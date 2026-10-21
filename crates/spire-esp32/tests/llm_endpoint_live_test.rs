// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2026 NatureSense

//! The configured endpoint answers **both** roles — the one thing the settings sheet cannot show.
//!
//! The LLM settings sheet hands the core a base URL and three model names, and every way that
//! pairing can be wrong looks the same from the UI: saved, no error. A URL one path component
//! short, a model alias the server does not serve, or a transport timeout shorter than the model's
//! own latency all surface only when a real run stalls. So this sends the two prompts the app's own
//! callers send — through `LlmActor::complete_prompt`, not a reimplementation — to whatever
//! `llm-config.json` names, which makes it exercise a LAN server and DeepSeek alike, and prints
//! what came back with how long it took.
//!
//! Gated like the other live tests: `#[ignore]` so a plain `cargo test` never spends a real call,
//! and a missing key skips rather than fails. A test binary is not named `spire-esp32`, so
//! `SPIRE_APP_NAME=spire-esp32` is what points this at the app's own config directory:
//!
//! ```text
//! SPIRE_APP_NAME=spire-esp32 cargo test -p spire-esp32 --test llm_endpoint_live_test \
//!     -- --ignored --nocapture
//! ```

use std::time::Instant;

use spire_core::subsystems::llm::llm::{LlmActor, LlmModelRole};

/// **One endpoint, both shapes.** Planning must come back as an object with a non-empty `steps`
/// array, and coding as the function that was asked for. A server whose alias is missing fails
/// before either assert is reached; one that answers structured prompts in `reasoning_content`
/// (leaving `content` empty) fails the first.
#[ignore = "live model: sends real completions — run explicitly with `--ignored`"]
#[tokio::test]
async fn the_configured_endpoint_answers_planning_and_coding() {
    // ── The gate: same as the other live tests, so `--ignored` on a machine with no key skips ──
    let config = spire_core::config::load_global_llm_config();
    if config.api_key.is_empty() {
        eprintln!(
            "skipped: no deepseek.api_key in {}\n             A test binary is not named `spire-esp32`, so `SPIRE_APP_NAME=spire-esp32` is what\n             points this at the app's own config dir.",
            spire_core::config::llm_config_path().display()
        );
        return;
    }

    eprintln!(
        "live endpoint: {}\n  planning={} coding={} base={}",
        config.api_url, config.planning_model, config.coding_model, config.model
    );

    let actor = LlmActor::new(config);

    // Planning: the plan has to land in `content` as JSON — the shape the plan parser reads.
    let started = Instant::now();
    let plan = actor
        .complete_prompt(
            "Return ONLY a JSON object with the keys \"title\" (a short string) and \"steps\" \
             (an array of exactly two short strings). No prose, no code fence.",
            LlmModelRole::Planning,
        )
        .await
        .expect("planning completion failed");
    eprintln!("planning ({} ms): {plan}", started.elapsed().as_millis());

    let plan_json: serde_json::Value =
        serde_json::from_str(strip_code_fence(&plan)).expect("planning reply is not JSON");
    assert!(
        plan_json
            .get("steps")
            .and_then(|steps| steps.as_array())
            .is_some_and(|steps| !steps.is_empty()),
        "planning reply has no non-empty `steps` array: {plan}"
    );

    // Coding: source text, not JSON — the shape the rewrite loop reads.
    let started = Instant::now();
    let code = actor
        .complete_prompt(
            "Write a single Rust function `fn add(a: i32, b: i32) -> i32` that returns a + b. \
             Output only the function, no prose and no code fence.",
            LlmModelRole::Coding,
        )
        .await
        .expect("coding completion failed");
    eprintln!("coding ({} ms): {code}", started.elapsed().as_millis());

    assert!(
        code.contains("fn add"),
        "coding reply has no `fn add`: {code}"
    );
}

/// Models wrap JSON in a ```` ```json ```` fence often enough to be worth tolerating here: the
/// app's own parser strips it, so a fence is not what this test is asking about.
fn strip_code_fence(text: &str) -> &str {
    let trimmed = text.trim();
    match trimmed.strip_prefix("```") {
        Some(rest) => {
            let body = rest.split_once('\n').map(|(_, body)| body).unwrap_or(rest);
            body.strip_suffix("```").unwrap_or(body).trim()
        }
        None => trimmed,
    }
}
