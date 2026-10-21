// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2026 NatureSense

//! Shared harness for the system-level tests.
//!
//! `#![allow(dead_code)]` because every test binary compiles this module separately and
//! uses a different subset of it.

#![allow(dead_code)]

use tokio::sync::mpsc;

/// A sender that accepts everything and answers nothing.
pub fn mock_sender<T: Send + 'static>() -> mpsc::Sender<T> {
    let (tx, mut rx) = mpsc::channel(64);
    tokio::spawn(async move { while rx.recv().await.is_some() {} });
    tx
}

/// A fake OpenAI-compatible endpoint, serving one scripted reply per request.
///
/// This is what makes the model the *only* thing replaced: the real `LlmActor` posts to
/// it over HTTP, parses the reply, and everything downstream is production code. Blocking
/// std IO on its own thread on purpose — the test runtime is busy driving actors.
pub fn fake_llm(replies: Vec<String>) -> String {
    fake_llm_logging(
        replies,
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
    )
}

/// The same endpoint, keeping every request body it was sent.
///
/// For the tests whose subject is the *prompt* rather than the answer: what a model is asked is not
/// observable from a reply, so the one place it can be checked is the wire.
/// The same endpoint, but **hanging up** when its script runs out.
///
/// `fake_llm` answers `NONE` forever once the script is spent, which is how a model that says
/// nothing useful is scripted. An endpoint that *goes away* mid-run cannot be scripted as a
/// reply at all: the request has to die, so this closes the socket without answering. That is
/// the only way to reach the transport-failure branches from a test.
pub fn fake_llm_then_hang_up(replies: Vec<String>) -> String {
    serve(
        replies,
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        true,
    )
}

pub fn fake_llm_logging(
    replies: Vec<String>,
    log: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
) -> String {
    serve(replies, log, false)
}

fn serve(
    replies: Vec<String>,
    log: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    hang_up_when_spent: bool,
) -> String {
    use std::io::Write;

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        let mut queue: std::collections::VecDeque<String> = replies.into();
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            if let Some(request) = read_request(&mut stream) {
                log.lock().unwrap().push(request);
            }

            let content = match queue.pop_front() {
                Some(content) => content,
                // Nothing left to say and this endpoint was asked to leave: dropping the stream
                // without a reply is what a transport failure looks like from the client side.
                None if hang_up_when_spent => continue,
                None => "NONE".to_string(),
            };
            let body = serde_json::json!({
                "choices": [{
                    "message": { "role": "assistant", "content": content },
                    "finish_reason": "stop"
                }]
            })
            .to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        }
    });
    format!("http://{addr}/v1/chat/completions")
}

/// Read one HTTP request off `stream`: the headers, then however much body the
/// `Content-Length` promises.
///
/// Shared by every fake endpoint here so "what the actor sent" is read the same way
/// whether the endpoint answers, refuses, or hangs up.
fn read_request(stream: &mut std::net::TcpStream) -> Option<String> {
    use std::io::Read;

    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let mut wanted: Option<usize> = None;
    loop {
        let Ok(n) = stream.read(&mut chunk) else {
            break;
        };
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(head_end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let length = *wanted.get_or_insert_with(|| {
                String::from_utf8_lossy(&buf[..head_end])
                    .to_lowercase()
                    .lines()
                    .find_map(|line| {
                        line.strip_prefix("content-length:")
                            .and_then(|v| v.trim().parse::<usize>().ok())
                    })
                    .unwrap_or(0)
            });
            if buf.len() >= head_end + 4 + length {
                break;
            }
        }
    }
    String::from_utf8(buf).ok()
}

/// A fake endpoint that **refuses** its first `refuse` connections, then serves `replies`.
///
/// A refusal here is accepted-and-dropped without a word — the transport shape of a server
/// that is not answering *yet*, which is what a Mac Mini coming out of sleep presents to the
/// first request. The returned counter is incremented on every connection, so a test can
/// prove the actor resent the request rather than the first one having quietly worked.
pub fn flaky_llm(
    refuse: usize,
    replies: Vec<String>,
) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    use std::io::Write;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let connections = std::sync::Arc::new(AtomicUsize::new(0));
    let seen = connections.clone();
    std::thread::spawn(move || {
        let mut queue: std::collections::VecDeque<String> = replies.into();
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            if seen.fetch_add(1, Ordering::SeqCst) < refuse {
                // Dropped without reading: the client sees a reset while sending, before
                // any byte of a completion could exist.
                continue;
            }
            let _ = read_request(&mut stream);
            let content = queue.pop_front().unwrap_or_else(|| "NONE".to_string());
            let body = serde_json::json!({
                "choices": [{
                    "message": { "role": "assistant", "content": content },
                    "finish_reason": "stop"
                }]
            })
            .to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        }
    });
    (format!("http://{addr}/v1/chat/completions"), connections)
}

/// A fake endpoint that answers **every** request with `status`, counting them.
///
/// For the half of the retry policy that is a refusal to retry: a 4xx is the server's
/// answer, so the count must stay at one however long the actor was prepared to wait.
pub fn refusing_llm(
    status: &'static str,
) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    use std::io::Write;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let connections = std::sync::Arc::new(AtomicUsize::new(0));
    let seen = connections.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            seen.fetch_add(1, Ordering::SeqCst);
            let _ = read_request(&mut stream);
            let body = r#"{"error":{"message":"refused by the test endpoint"}}"#;
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        }
    });
    (format!("http://{addr}/v1/chat/completions"), connections)
}

/// The bundled `idf/` project, as this crate's tests see it.
///
/// `CARGO_MANIFEST_DIR` is `crates/spire-esp32`, so the project is two levels up. Canonicalized where
/// possible so a path handed to CMake names the copy it is building rather than a `..` route back to
/// the original — the difference matters in [`copy_component`]'s callers, where building the
/// *original* while asserting about the copy would make a mutation test pass for the wrong reason.
pub fn idf_root() -> std::path::PathBuf {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("idf");
    root.canonicalize().unwrap_or(root)
}

/// Does this machine have the gate's own tools?
///
/// `build_host_test` needs `cmake` and the test leg needs `ctest`. Without them the component gate
/// refuses before anything can be asserted, so a test that needs the gate says so and returns rather
/// than reporting a pass it did not earn.
pub fn gate_is_runnable() -> bool {
    ["cmake", "ctest"].iter().all(|tool| {
        std::process::Command::new(tool)
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    })
}

/// Copy one bundled component into `<root>/components/<name>`, minus its build output.
///
/// A **copy**, not the original: these tests write broken source on purpose, and a component in the
/// library is not theirs to break. `build/` is skipped for a sharper reason — `test/build` holds an
/// absolute CMake cache and a previously linked test binary, so a copy that kept it would configure
/// against the original's paths and run the original's code. That is the shared-provenance mistake
/// in miniature: the thing under test would no longer be the copy.
pub fn copy_component(root: &std::path::Path, name: &str) -> std::path::PathBuf {
    let from = idf_root().join("components").join(name);
    let to = root.join("components").join(name);
    copy_tree(&from, &to);
    to
}

/// A recursive copy that leaves every directory named `build` behind.
fn copy_tree(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).expect("create the copy's directory");
    for entry in std::fs::read_dir(from).expect("read the component's directory") {
        let entry = entry.expect("read a component entry");
        let name = entry.file_name();
        let target = to.join(&name);
        if entry.file_type().expect("entry type").is_dir() {
            if name == "build" {
                continue;
            }
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).expect("copy a component file");
        }
    }
}
