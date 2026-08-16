//! How long a model call may take, proved against a socket.
//!
//! The three providers share `http.rs`, so the ceiling is tested once here
//! rather than three times in the per-protocol wire tests. What has to be true
//! is small and easy to get subtly wrong: the number a declaration states is
//! the number the transport waits, "no ceiling" is not a ceiling of zero, and a
//! request that ran out of time is not asked again.
//!
//! Closes [GAP-040](../../../docs/gaps.md#gap-040).

#![cfg(feature = "openai")]

mod support;

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use ingot_runtime::catalogue::{ModelConfig, ProviderConfig, ProviderKind, DEFAULT_TIMEOUT};
use ingot_runtime::http;
use ingot_runtime::provider::{CompletionRequest, ModelProvider, ModelSelection, ProviderError};
use ingot_runtime::schema;
use serde_json::json;
use support::{serve_once, serve_silently};

/// Short enough that the tests are not a wait, long enough that a loaded CI
/// machine does not lose the race to open the connection at all.
const BRIEF: Duration = Duration::from_millis(250);

fn request() -> CompletionRequest {
    CompletionRequest {
        node: "n0".into(),
        model: ModelSelection::Exact("local/any".into()),
        system: None,
        prompt: "anything".into(),
        context: Vec::new(),
        response_type: "markdown".into(),
        shape: schema::response_shape("markdown", &BTreeMap::new()).unwrap(),
        max_tokens: 256,
    }
}

#[test]
fn a_stated_ceiling_is_the_one_the_transport_waits_for() {
    let url = serve_silently("/v1/chat/completions");

    let started = Instant::now();
    let error = http::post_json(&url, &[], &json!({ "model": "m" }), Some(BRIEF), 0)
        .expect_err("nobody is going to answer");
    let waited = started.elapsed();

    assert!(
        matches!(error, ProviderError::Transport(_)),
        "a silent endpoint is a transport failure, not a rejection: {error}"
    );
    // The assertion is that it waited *this* long rather than the default. A
    // ceiling that is accepted and then ignored is the bug this guards.
    assert!(
        waited < DEFAULT_TIMEOUT,
        "waited {waited:?}, which is the default rather than the ceiling asked for"
    );
}

#[test]
fn no_ceiling_is_not_a_ceiling_of_zero() {
    // `timeout-seconds = 0` means wait indefinitely. Reading the absent
    // duration as `Duration::ZERO` would instead fail every call before it left
    // the machine, which is the one reading nobody could have wanted -- and it
    // is one character away in the transport.
    let (url, _captured) = serve_once(
        "/v1/chat/completions",
        200,
        json!({ "id": "x", "model": "m", "choices": [] }),
    );

    let answer = http::post_json(&url, &[], &json!({ "model": "m" }), None, 0)
        .expect("an unbounded wait still returns when the answer arrives");
    assert_eq!(answer["id"], "x");
}

#[test]
fn a_request_that_ran_out_of_time_is_not_asked_again() {
    // Three retries would make a stated ceiling mean four times itself, so the
    // number an operator wrote would not be the wait they got. Measured rather
    // than asserted on a counter: four attempts at `BRIEF` plus the backoff
    // between them cannot fit inside two of them.
    let url = serve_silently("/v1/chat/completions");

    let started = Instant::now();
    let _ = http::post_json(&url, &[], &json!({ "model": "m" }), Some(BRIEF), 3)
        .expect_err("nobody is going to answer");
    let waited = started.elapsed();

    assert!(
        waited < BRIEF * 2,
        "waited {waited:?} for a ceiling of {BRIEF:?}: the attempt was repeated"
    );
}

#[test]
fn a_declared_wait_reaches_the_provider_built_from_it() {
    // The wiring end to end: a manifest field, through `catalogue::build`, to
    // the socket. Tested through the declaration rather than by calling
    // `with_timeout` directly, because the step that can silently go missing is
    // the one inside `build`.
    let url = serve_silently("/v1/chat/completions");
    let declaration = ProviderConfig {
        name: "local".to_string(),
        kind: ProviderKind::Openai,
        base_url: url,
        api_key_env: None,
        timeout_seconds: Some(1),
    };

    let mut provider =
        ingot_runtime::catalogue::build(&declaration, &ModelConfig::default(), None, None)
            .expect("an openai-compatible endpoint needs no key");

    let started = Instant::now();
    let error = provider
        .complete(&request())
        .expect_err("nobody is going to answer");
    let waited = started.elapsed();

    assert!(
        matches!(error, ProviderError::Transport(_)),
        "a silent endpoint is a transport failure: {error}"
    );
    assert!(
        waited < DEFAULT_TIMEOUT,
        "waited {waited:?}: `timeout-seconds` never reached the transport"
    );
}

#[test]
fn a_declaration_that_says_nothing_still_reaches_a_service_that_answers() {
    // The default path, so that giving the field a home did not make the
    // absence of it mean something new.
    let (url, _captured) = serve_once(
        "/v1/chat/completions",
        200,
        json!({
            "id": "chatcmpl-test",
            "model": "m",
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": "# Fine" },
                "finish_reason": "stop",
            }],
            "usage": { "prompt_tokens": 1, "completion_tokens": 1 },
        }),
    );
    let declaration = ProviderConfig {
        name: "local".to_string(),
        kind: ProviderKind::Openai,
        base_url: url,
        api_key_env: None,
        timeout_seconds: None,
    };

    let mut provider =
        ingot_runtime::catalogue::build(&declaration, &ModelConfig::default(), None, None)
            .expect("an openai-compatible endpoint needs no key");

    let response = provider.complete(&request()).expect("the stub answers");
    assert_eq!(response.value, json!("# Fine"));
}
