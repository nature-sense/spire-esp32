// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2026 NatureSense

//! The retry that covers a server waking from sleep.
//!
//! The subject is the seam between `LlmActor` and the socket: a request that never
//! reached the model must be resent, and one the server answered must not be. Both
//! are assertions about *connections*, not about the reply text — which is exactly
//! what the fake endpoints here count.
//!
//! What this cannot tell you is whether a real server takes this long to wake. It
//! tells you the actor keeps asking through the window instead of reporting the
//! first refusal as the final answer.

use std::sync::atomic::Ordering;

use spire_core::subsystems::llm::llm::LlmModelRole;
use spire_esp32::actors::{LlmActor, LlmConfig};

mod common;

use common::{flaky_llm, refusing_llm};

/// An actor pointed at `url`, with the key the endpoint never checks.
fn actor(url: &str) -> LlmActor {
    LlmActor::new(LlmConfig {
        api_url: url.to_string(),
        api_key: "test-key".to_string(),
        ..LlmConfig::default()
    })
}

/// The wake-from-sleep case: the first connections are refused, the retry lands.
#[tokio::test]
async fn rides_out_the_first_refused_connections() {
    let (url, connections) = flaky_llm(2, vec!["the answer".to_string()]);
    let actor = actor(&url);

    let answer = actor
        .complete_prompt("hello", LlmModelRole::Default)
        .await
        .expect("the retry should absorb two refusals");

    assert_eq!(answer, "the answer");
    assert!(
        connections.load(Ordering::SeqCst) >= 3,
        "the actor should have connected at least three times (two refusals, one \
         answer); it saw {}",
        connections.load(Ordering::SeqCst)
    );
}

/// A 4xx is the server's answer, not a transport accident: resending a prompt the
/// server already read would cost a second request for the same refusal.
#[tokio::test]
async fn a_definitive_refusal_is_not_resent() {
    let (url, connections) = refusing_llm("401 Unauthorized");
    let actor = actor(&url);

    let error = actor
        .complete_prompt("hello", LlmModelRole::Default)
        .await
        .expect_err("a 401 is final");

    assert!(
        format!("{error}").contains("401"),
        "the caller should see the server's status, got: {error}"
    );
    assert_eq!(
        connections.load(Ordering::SeqCst),
        1,
        "a refused prompt must reach the server exactly once"
    );
}

/// 503 is "not ready yet": every attempt is made, and the failure is reported only
/// after the last one — with the attempt count, so the log says how long it tried.
#[tokio::test]
async fn a_server_that_never_comes_up_is_tried_the_full_number_of_times() {
    let (url, connections) = refusing_llm("503 Service Unavailable");
    let actor = actor(&url);

    let error = actor
        .complete_prompt("hello", LlmModelRole::Default)
        .await
        .expect_err("four 503s exhaust the retries");

    assert_eq!(
        connections.load(Ordering::SeqCst),
        spire_core::subsystems::llm::llm::LLM_RETRY_ATTEMPTS as usize,
        "every allowed attempt should have been made"
    );
    assert!(
        format!("{error}").contains("after 4 attempts"),
        "the error should say it gave up rather than that it never tried, got: {error}"
    );
}
