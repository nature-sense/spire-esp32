// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2026 NatureSense

//! `modify/code` against a real LLM path — with the *model* replaced, not the code.
//!
//! The flow's unit tests script a backend, so the measured seam and the measure share the
//! author's assumptions. These tests instead point the **real** `LlmActor` at a fake
//! OpenAI-compatible endpoint: the HTTP request, the response parse, the fenced-code
//! stripping, the tree-sitter check and its retry, `select_files`, the apply, the verify —
//! all real. Only the model's *text* is scripted, which is the one thing you want to
//! control anyway.
//!
//! What this cannot tell you is whether a real model gives *good* answers. That is a
//! quality question and needs a live endpoint; this is the plumbing, where the bugs are.

use spire_actor::ActorSystem;
use spire_core::subsystems::graph::memory_graph::MemoryGraphMessage;
use spire_esp32::actors::{
    ChatActor, CoordinatorActor, CoordinatorMessage, LlmActor, LlmConfig, McpClientActor,
    SystemActor, ToolsActor,
};
use spire_esp32::subsystems::build::build_manager::{BuildEventLogActor, BuildEventLogMessage};
use tokio::sync::mpsc;

mod common;

use common::{fake_llm, fake_llm_then_hang_up, mock_sender};

/// The coordinator's memory-graph channel with no actor behind it: every read comes back
/// empty, which is what makes "the build reported no errors" the baseline here.
fn mock_memory_graph() -> mpsc::Sender<MemoryGraphMessage> {
    let (tx, _rx) = mpsc::channel(64);
    tx
}

/// A coordinator wired to a real LLM actor pointed at `url`.
///
/// Everything else is a mock: the build and test tools answer nothing, so a run's
/// baseline is "nothing known to be broken" and the verdict is decided by the code under
/// test rather than by a stub toolchain.
async fn app(url: &str) -> mpsc::Sender<CoordinatorMessage> {
    app_with_activity(url).await.0
}

/// As [`app`], with the pane's activity sink attached — the same wiring `ffi.rs` does at
/// startup, and the same ordered queue the build events use.
async fn app_with_activity(
    url: &str,
) -> (
    mpsc::Sender<CoordinatorMessage>,
    mpsc::Sender<BuildEventLogMessage>,
) {
    let system = ActorSystem::new();
    let (chat_tx, _) = system.spawn(ChatActor::new());
    let (tools_tx, _) = system.spawn(ToolsActor::new(mock_sender()));
    let (mcp_tx, _) = system.spawn(McpClientActor::new());
    let (llm_tx, _) = system.spawn(LlmActor::new(LlmConfig {
        api_url: url.to_string(),
        // The actor refuses a request with no key, but never checks it against anything:
        // our endpoint does not care what it is.
        api_key: "test-key".to_string(),
        ..LlmConfig::default()
    }));
    let (system_tx, _) = system.spawn(SystemActor::new());

    let (coord_tx, _handle) = system.spawn(CoordinatorActor::new(
        chat_tx,
        tools_tx,
        mcp_tx,
        llm_tx,
        system_tx,
        mock_memory_graph(),
        mock_sender(),
        mock_sender(),
        mock_sender(),
        mock_sender(),
        mock_sender(),
    ));
    let notify = std::sync::Arc::new(tokio::sync::Notify::new());
    let (log_tx, _) = system.spawn(BuildEventLogActor::new(notify));
    // Same sender as the request below, so the sink is attached before the run is routed.
    coord_tx
        .send(CoordinatorMessage::SetActivitySink { tx: log_tx.clone() })
        .await
        .unwrap();
    (coord_tx, log_tx)
}

/// Send one request and wait for the coordinator's answer.
async fn call(
    coord: &mpsc::Sender<CoordinatorMessage>,
    method: &str,
    params: serde_json::Value,
) -> serde_json::Value {
    let (tx, rx) = tokio::sync::oneshot::channel();
    coord
        .send(CoordinatorMessage::HandleRequest {
            method: method.to_string(),
            params,
            response_tx: tx,
        })
        .await
        .unwrap();
    rx.await.unwrap()
}

/// A project with one source file and a build directory, so a platform resolves.
fn project() -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("a.cpp");
    std::fs::write(&file, "int value = 1;\n").unwrap();
    std::fs::create_dir_all(tmp.path().join("build-fake")).unwrap();
    (tmp, file)
}

/// What `modify/code` is asked to do, shaped as the UI sends it.
fn request(root: &std::path::Path, scope: &[&std::path::Path]) -> serde_json::Value {
    serde_json::json!({
        "path": root.to_string_lossy(),
        "prompt": "make value 2",
        "platform": "fake",
        "scope": scope
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect::<Vec<_>>(),
    })
}

/// The whole path with a model behind it: the listing call picks the file, the rewrite
/// call returns fenced C++, and the change lands and is kept.
#[tokio::test]
async fn rewrites_the_file_the_model_names() {
    let (tmp, file) = project();
    let url = fake_llm(vec![
        file.to_string_lossy().to_string(),
        "```cpp\nint value = 2;\n```\n".to_string(),
    ]);
    let coord = app(&url).await;

    let result = call(&coord, "modify/code", request(tmp.path(), &[&file])).await;

    assert_eq!(result["success"], serde_json::json!(true), "{result}");
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "int value = 2;\n",
        "the fenced block was unwrapped and written as a whole file: {result}"
    );
}

/// Models decorate. The path list is read through prose, bullets and backticks — this is
/// the parsing step that would otherwise write to a file nobody named.
#[tokio::test]
async fn reads_a_path_list_through_prose_and_bullets() {
    let (tmp, file) = project();
    let url = fake_llm(vec![
        format!(
            "Here is the file that needs changing:\n\n- `{}`\n",
            file.display()
        ),
        "```cpp\nint value = 3;\n```".to_string(),
    ]);
    let coord = app(&url).await;

    let result = call(&coord, "modify/code", request(tmp.path(), &[&file])).await;

    assert_eq!(result["success"], serde_json::json!(true), "{result}");
    // One trailing newline: unwrapping the fence trims the block, and what is written is a whole
    // file — so the newline the block ended in is put back rather than dropped.
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "int value = 3;\n");
}

/// A model naming a file that was never offered must get nothing. The scope is a
/// guardrail, not a hint.
#[tokio::test]
async fn will_not_touch_a_file_outside_the_scope() {
    let (tmp, file) = project();
    let outside = tmp.path().join("b.cpp");
    std::fs::write(&outside, "int untouched = 7;\n").unwrap();

    let url = fake_llm(vec![outside.to_string_lossy().to_string()]);
    let coord = app(&url).await;

    let result = call(&coord, "modify/code", request(tmp.path(), &[&file])).await;

    assert_eq!(result["success"], serde_json::json!(false), "{result}");
    assert_eq!(
        std::fs::read_to_string(&outside).unwrap(),
        "int untouched = 7;\n",
        "the file it named was not offered, so it was not touched: {result}"
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "int value = 1;\n");
}

/// A rewrite that does not parse is retried once (feeding the syntax errors back), and if
/// it still does not parse it is dropped. Nothing garbled is written, and the run says so
/// rather than reporting a success with no changes.
#[tokio::test]
async fn drops_a_rewrite_that_does_not_parse() {
    let (tmp, file) = project();
    let url = fake_llm(vec![
        file.to_string_lossy().to_string(),
        "int broken( ;\n".to_string(),
        // The retry's answer, just as broken.
        "int still_broken( ;\n".to_string(),
    ]);
    let coord = app(&url).await;

    let result = call(&coord, "modify/code", request(tmp.path(), &[&file])).await;

    assert_eq!(result["success"], serde_json::json!(false), "{result}");
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "int value = 1;\n",
        "the unparseable rewrite was never written: {result}"
    );
}

/// Everything the activity log has recorded so far, oldest first.
async fn drain(log: &mpsc::Sender<BuildEventLogMessage>) -> Vec<serde_json::Value> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    log.send(BuildEventLogMessage::Drain { reply_to: tx })
        .await
        .unwrap();
    rx.await.unwrap()
}

/// Everything the pane shows in one run, in the order it happened.
///
/// The activity log is a *view of the run*, so what it must carry is the run's own facts: what was
/// asked, which file was written, whether the reply parsed, and what the gate said. This is the
/// only test that reads the queue `ffi.rs` wires at startup, so it is also what proves the sink
/// is actually attached rather than traced and forgotten.
#[tokio::test]
async fn the_activity_log_carries_the_rewrite_and_the_gate() {
    let (tmp, file) = project();
    let url = fake_llm(vec![
        file.to_string_lossy().to_string(),
        "```cpp\nint value = 2;\n```\n".to_string(),
    ]);
    let (coord, log) = app_with_activity(&url).await;

    let result = call(&coord, "modify/code", request(tmp.path(), &[&file])).await;
    assert_eq!(result["success"], serde_json::json!(true), "{result}");

    let events = drain(&log).await;
    let lines: Vec<&str> = events
        .iter()
        .filter_map(|event| event["line"].as_str())
        .collect();

    assert!(
        lines.iter().any(|line| line.contains("Edit: make value 2")),
        "what was asked is the one thing no report carries: {lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("Writing ") && line.contains("a.cpp")),
        "the file being written is named before the model is asked: {lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("← ") && line.contains("parses ✓")),
        "the rewrite reports its size, its syntax check, and how long it took: {lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.contains("Compiling the project")),
        "the compile that judges the rewrite is part of the same timeline: {lines:?}"
    );
    assert!(
        lines.iter().any(|line| line.contains("Host tests")),
        "the gate is reported even when it could not run: {lines:?}"
    );
    assert!(
        lines.iter().any(|line| line.starts_with("✓ Edit:")),
        "the run ends with what was kept and how it was verified: {lines:?}"
    );

    // Every line names its producer: the pane's icon and colour key on this pair, and a build line
    // that silently lost it would render as an anonymous circle.
    assert!(
        events
            .iter()
            .all(|event| event["source"].is_string() && event["phase"].is_string()),
        "a line without its source and phase is not usable by the pane: {events:?}"
    );
}

/// A rewrite that dies at the transport is reported **once**, not once per caller.
///
/// It is `llm_rewrite` that sees the failure — it has the file, the time it spent, and the error —
/// so it is the only thing that may say so. The planning loop that called it must not repeat the
/// line, or one moment reads as two problems in a pane whose whole premise is that each line is a
/// moment. The error is still carried into the run's closing line, so nothing is lost by it.
#[tokio::test]
async fn a_failed_rewrite_is_reported_once() {
    let (tmp, file) = project();
    // The listing is answered with the file; the rewrite's request finds the endpoint gone. A
    // transport failure cannot be scripted as a reply, which is why the harness hangs up.
    let url = fake_llm_then_hang_up(vec![file.to_string_lossy().to_string()]);
    let (coord, log) = app_with_activity(&url).await;

    let result = call(&coord, "modify/code", request(tmp.path(), &[&file])).await;
    assert_eq!(
        result["success"],
        serde_json::json!(false),
        "the model was never reached: {result}"
    );

    let events = drain(&log).await;
    let lines: Vec<&str> = events
        .iter()
        .filter_map(|event| event["line"].as_str())
        .collect();
    let about_the_file: Vec<&&str> = lines
        .iter()
        .filter(|line| line.starts_with("✗ ") && line.contains("a.cpp"))
        .collect();
    assert_eq!(
        about_the_file.len(),
        1,
        "one failed rewrite, one line about it: {lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("✗ Edit:") && line.contains("could not be reached")),
        "the run still ends by saying the model was never reached: {lines:?}"
    );
}
