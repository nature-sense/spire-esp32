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
pub fn fake_llm_logging(
    replies: Vec<String>,
    log: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
) -> String {
    use std::io::{Read, Write};

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        let mut queue: std::collections::VecDeque<String> = replies.into();
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            // Read the headers, then however much body the Content-Length promises.
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
            if let Ok(request) = String::from_utf8(buf.clone()) {
                log.lock().unwrap().push(request);
            }

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
    format!("http://{addr}/v1/chat/completions")
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
