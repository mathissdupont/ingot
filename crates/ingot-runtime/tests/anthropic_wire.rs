//! Wire-level tests for the Anthropic provider.
//!
//! The stub server lives in `support`; these assert what Anthropic specifically
//! sends and understands.

#![cfg(feature = "anthropic")]

mod support;

use std::collections::BTreeMap;

use ingot_runtime::anthropic::AnthropicProvider;
use ingot_runtime::provider::{CompletionRequest, ModelProvider, ModelSelection, ProviderError};
use ingot_runtime::schema;
use serde_json::{json, Value};
use support::serve_once as serve_at;

/// Serve one request at the Messages endpoint.
fn serve_once(
    status: u16,
    response: Value,
) -> (String, std::sync::mpsc::Receiver<support::Captured>) {
    serve_at("/v1/messages", status, response)
}

fn request(response_type: &str, prompt: &str) -> CompletionRequest {
    let shape = schema::response_shape(response_type, &BTreeMap::new()).unwrap();
    CompletionRequest {
        node: "n0".into(),
        model: ModelSelection::Default,
        system: Some("You are terse.".into()),
        prompt: prompt.into(),
        context: vec![("document".into(), json!("the source text"))],
        response_type: response_type.into(),
        shape,
        max_tokens: 2048,
    }
}

fn ok_response(text: &str) -> Value {
    json!({
        "id": "msg_test",
        "model": "claude-opus-5",
        "stop_reason": "end_turn",
        "content": [{ "type": "text", "text": text }],
        "usage": { "input_tokens": 42, "output_tokens": 7 },
    })
}

#[test]
fn a_prose_request_carries_the_expected_headers_and_body() {
    let (url, captured) = serve_once(200, ok_response("# Summary\n\nShort."));
    let mut provider = AnthropicProvider::with_key("test-key").with_base_url(url);

    let response = provider
        .complete(&request("markdown", "Summarise it"))
        .unwrap();
    assert_eq!(response.value, json!("# Summary\n\nShort."));
    assert_eq!(response.usage.input_tokens, 42);
    assert_eq!(response.model, "claude-opus-5");

    let seen = captured
        .recv()
        .expect("the stub should have served a request");
    assert_eq!(
        seen.headers.get("x-api-key").map(String::as_str),
        Some("test-key")
    );
    assert_eq!(
        seen.headers.get("anthropic-version").map(String::as_str),
        Some("2023-06-01")
    );
    assert_eq!(
        seen.headers.get("content-type").map(String::as_str),
        Some("application/json")
    );

    assert_eq!(seen.body["model"], "claude-opus-5");
    assert_eq!(seen.body["max_tokens"], 2048);
    assert_eq!(seen.body["system"], "You are terse.");
    assert_eq!(seen.body["messages"][0]["role"], "user");

    // Sampling parameters are rejected by current models; sending one would
    // fail the request outright.
    for rejected in ["temperature", "top_p", "top_k"] {
        assert!(
            seen.body.get(rejected).is_none(),
            "`{rejected}` must not be sent"
        );
    }
    // Asking for markdown must not constrain the model to JSON.
    assert!(seen.body.get("output_config").is_none());
}

#[test]
fn a_typed_request_sends_a_json_schema_and_unwraps_the_answer() {
    let (url, captured) = serve_once(200, ok_response(r#"{"value":["one","two"]}"#));
    let mut provider = AnthropicProvider::with_key("k").with_base_url(url);

    let response = provider
        .complete(&request("string[]", "List two things"))
        .unwrap();
    assert_eq!(response.value, json!(["one", "two"]));

    let seen = captured.recv().unwrap();
    let format = &seen.body["output_config"]["format"];
    assert_eq!(format["type"], "json_schema");
    assert_eq!(format["schema"]["properties"]["value"]["type"], "array");
    assert_eq!(format["schema"]["additionalProperties"], false);
}

#[test]
fn context_values_reach_the_model_inside_named_tags() {
    let (url, captured) = serve_once(200, ok_response("done"));
    let mut provider = AnthropicProvider::with_key("k").with_base_url(url);
    provider
        .complete(&request("markdown", "Use the document"))
        .unwrap();

    let seen = captured.recv().unwrap();
    let content = seen.body["messages"][0]["content"].as_str().unwrap();
    assert!(content.contains("<document>"), "{content}");
    assert!(content.contains("the source text"), "{content}");
    assert!(
        content.trim_end().ends_with("Use the document"),
        "{content}"
    );
}

#[test]
fn an_effort_setting_is_sent_inside_output_config() {
    let (url, captured) = serve_once(200, ok_response("done"));
    let mut provider = AnthropicProvider::with_key("k")
        .with_base_url(url)
        .with_effort(Some("low".into()));
    provider.complete(&request("markdown", "Be quick")).unwrap();

    let seen = captured.recv().unwrap();
    assert_eq!(seen.body["output_config"]["effort"], "low");
}

#[test]
fn a_model_override_wins_over_the_artifact() {
    let (url, captured) = serve_once(200, ok_response("done"));
    let mut provider = AnthropicProvider::with_key("k")
        .with_base_url(url)
        .with_model(Some("claude-haiku-4-5".into()));
    provider.complete(&request("markdown", "hi")).unwrap();

    assert_eq!(captured.recv().unwrap().body["model"], "claude-haiku-4-5");
}

#[test]
fn a_refusal_is_surfaced_rather_than_returned_as_content() {
    let (url, _captured) = serve_once(
        200,
        json!({
            "stop_reason": "refusal",
            "stop_details": { "category": "cyber", "explanation": "declined" },
            "content": [],
        }),
    );
    let mut provider = AnthropicProvider::with_key("k").with_base_url(url);
    let error = provider.complete(&request("markdown", "x")).unwrap_err();
    assert!(matches!(error, ProviderError::Refused { .. }), "{error}");
    assert!(error.to_string().contains("cyber"), "{error}");
}

#[test]
fn an_authentication_failure_is_reported_without_retrying() {
    let (url, _captured) = serve_once(401, json!({"error": {"message": "bad key"}}));
    let mut provider = AnthropicProvider::with_key("wrong").with_base_url(url);
    let error = provider.complete(&request("markdown", "x")).unwrap_err();
    match error {
        ProviderError::Request { status, message } => {
            assert_eq!(status, 401);
            assert!(message.contains("API key"), "{message}");
        }
        other => panic!("expected a request error, got {other}"),
    }
}

#[test]
fn a_bad_request_names_the_likely_cause() {
    let (url, _captured) = serve_once(400, json!({"error": {"message": "nope"}}));
    let mut provider = AnthropicProvider::with_key("k").with_base_url(url);
    let error = provider.complete(&request("markdown", "x")).unwrap_err();
    assert!(
        error.to_string().contains("unsupported parameter"),
        "{error}"
    );
}

#[test]
fn a_truncated_answer_is_an_error_not_a_partial_result() {
    let (url, _captured) = serve_once(
        200,
        json!({
            "stop_reason": "max_tokens",
            "content": [{"type": "text", "text": "half an answ"}],
        }),
    );
    let mut provider = AnthropicProvider::with_key("k").with_base_url(url);
    let error = provider.complete(&request("markdown", "x")).unwrap_err();
    assert!(matches!(error, ProviderError::Truncated { .. }), "{error}");
}

#[test]
fn a_typed_answer_that_is_not_json_is_an_error() {
    let (url, _captured) = serve_once(200, ok_response("I decided to write prose instead"));
    let mut provider = AnthropicProvider::with_key("k").with_base_url(url);
    let error = provider
        .complete(&request("string[]", "List things"))
        .unwrap_err();
    assert!(
        matches!(error, ProviderError::InvalidResponse(_)),
        "{error}"
    );
}
