//! Wire-level tests for the OpenAI-compatible provider.
//!
//! The endpoint is deliberately reachable through `INGOT_OPENAI_BASE_URL`, so
//! these run against localhost — and so an operator can point the same provider
//! at Azure, a gateway, or a local server. That is the whole reason this speaks
//! Chat Completions rather than a richer, vendor-only shape.

#![cfg(feature = "openai")]

mod support;

use std::collections::BTreeMap;

use ingot_runtime::openai::OpenAiProvider;
use ingot_runtime::provider::{CompletionRequest, ModelProvider, ModelSelection, ProviderError};
use ingot_runtime::schema;
use serde_json::{json, Value};
use support::serve_once as serve_at;

fn serve_once(
    status: u16,
    response: Value,
) -> (String, std::sync::mpsc::Receiver<support::Captured>) {
    serve_at("/v1/chat/completions", status, response)
}

fn request(response_type: &str, prompt: &str) -> CompletionRequest {
    let shape = schema::response_shape(response_type, &BTreeMap::new()).unwrap();
    CompletionRequest {
        node: "n0".into(),
        model: ModelSelection::Exact("openai/gpt-test".into()),
        system: Some("You are terse.".into()),
        prompt: prompt.into(),
        context: vec![("document".into(), json!("the source text"))],
        response_type: response_type.into(),
        shape,
        max_tokens: 2048,
    }
}

fn ok_response(content: &str) -> Value {
    json!({
        "id": "chatcmpl-test",
        "model": "gpt-test",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": content },
            "finish_reason": "stop",
        }],
        "usage": { "prompt_tokens": 42, "completion_tokens": 7 },
    })
}

fn provider(url: &str) -> OpenAiProvider {
    OpenAiProvider::with_key("stub-key").with_base_url(url)
}

#[test]
fn a_prose_request_carries_bearer_auth_and_the_expected_body() {
    let (url, captured) = serve_once(200, ok_response("# Summary"));
    let response = provider(&url)
        .complete(&request("markdown", "Summarise it"))
        .expect("the stub answers");

    assert_eq!(response.value, json!("# Summary"));
    assert_eq!(response.usage.input_tokens, 42);
    assert_eq!(response.usage.output_tokens, 7);
    assert_eq!(response.model, "gpt-test");

    let seen = captured.recv().expect("the server saw a request");
    assert_eq!(
        seen.headers.get("authorization").map(String::as_str),
        Some("Bearer stub-key")
    );
    assert_eq!(
        seen.headers.get("content-type").map(String::as_str),
        Some("application/json")
    );
    assert_eq!(seen.body["model"], json!("gpt-test"));
    assert_eq!(seen.body["max_completion_tokens"], json!(2048));
    assert!(
        seen.body.get("response_format").is_none(),
        "prose is not constrained: {}",
        seen.body
    );
    assert_eq!(seen.body["messages"][0]["role"], "system");
    assert_eq!(seen.body["messages"][1]["role"], "user");
}

#[test]
fn a_typed_request_sends_a_strict_schema_and_unwraps_the_answer() {
    let (url, captured) = serve_once(200, ok_response("{\"value\":[\"a\",\"b\"]}"));
    let response = provider(&url)
        .complete(&request("string[]", "List two"))
        .expect("the stub answers");

    assert_eq!(response.value, json!(["a", "b"]));

    let seen = captured.recv().unwrap();
    let format = &seen.body["response_format"];
    assert_eq!(format["type"], "json_schema");
    assert_eq!(format["json_schema"]["strict"], json!(true));
    assert_eq!(
        format["json_schema"]["schema"]["properties"]["value"]["items"]["type"],
        "string"
    );
    assert_eq!(
        format["json_schema"]["schema"]["additionalProperties"],
        json!(false),
        "a strict schema requires this: {}",
        seen.body
    );
}

#[test]
fn context_values_reach_the_model_inside_named_tags() {
    let (url, captured) = serve_once(200, ok_response("ok"));
    provider(&url)
        .complete(&request("markdown", "Use the document"))
        .unwrap();

    let seen = captured.recv().unwrap();
    let content = seen.body["messages"][1]["content"].as_str().unwrap();
    assert!(content.contains("<document>"), "{content}");
    assert!(content.contains("the source text"), "{content}");
    assert!(content.contains("</document>"), "{content}");
    assert!(
        content.ends_with("Use the document"),
        "the prompt comes last: {content}"
    );
}

#[test]
fn an_effort_setting_becomes_reasoning_effort() {
    let (url, captured) = serve_once(200, ok_response("ok"));
    provider(&url)
        .with_effort(Some("max".into()))
        .complete(&request("markdown", "think"))
        .unwrap();

    let seen = captured.recv().unwrap();
    assert_eq!(seen.body["reasoning_effort"], json!("high"));
}

#[test]
fn a_model_override_wins_over_the_artifact() {
    let (url, captured) = serve_once(200, ok_response("ok"));
    provider(&url)
        .with_model(Some("gpt-override".into()))
        .complete(&request("markdown", "hello"))
        .unwrap();

    assert_eq!(
        captured.recv().unwrap().body["model"],
        json!("gpt-override")
    );
}

#[test]
fn an_artifact_that_names_no_model_stops_before_any_request() {
    // A guessed model is a 404 that reads like a bug in Ingot.
    let (url, captured) = serve_once(200, ok_response("never sent"));
    let mut call = request("markdown", "hello");
    call.model = ModelSelection::Default;

    let error = provider(&url).complete(&call).unwrap_err();
    assert!(error.to_string().contains("model exact"), "{error}");
    assert!(
        captured.try_recv().is_err(),
        "nothing should have been sent"
    );
}

#[test]
fn a_refusal_is_surfaced_rather_than_returned_as_content() {
    let payload = json!({
        "choices": [{
            "message": { "role": "assistant", "content": null, "refusal": "I can't help with that." },
            "finish_reason": "stop",
        }]
    });
    let (url, _captured) = serve_once(200, payload);
    let error = provider(&url)
        .complete(&request("markdown", "do something"))
        .unwrap_err();

    assert!(matches!(error, ProviderError::Refused { .. }), "{error}");
    assert!(error.to_string().contains("can't help"), "{error}");
}

#[test]
fn a_truncated_answer_is_an_error_rather_than_a_short_artifact() {
    let payload = json!({
        "choices": [{ "message": { "content": "half an ans" }, "finish_reason": "length" }]
    });
    let (url, _captured) = serve_once(200, payload);
    let error = provider(&url)
        .complete(&request("markdown", "write a lot"))
        .unwrap_err();

    assert!(
        matches!(error, ProviderError::Truncated { limit: 2048 }),
        "{error}"
    );
}

#[test]
fn an_authentication_failure_is_reported_without_retrying() {
    let (url, captured) = serve_once(401, json!({ "error": { "message": "bad key" } }));
    let error = provider(&url)
        .complete(&request("markdown", "hello"))
        .unwrap_err();

    assert!(
        matches!(error, ProviderError::Request { status: 401, .. }),
        "{error}"
    );
    assert!(error.to_string().contains("API key"), "{error}");
    // The stub serves exactly one request; a retry would have found nothing.
    assert!(captured.recv().is_ok());
}

#[test]
fn a_gateway_that_reports_an_error_with_a_success_status_is_still_an_error() {
    // Several OpenAI-compatible services do this, and treating it as a
    // successful empty answer would produce a confusing artifact.
    let (url, _captured) = serve_once(200, json!({ "error": { "message": "model not found" } }));
    let error = provider(&url)
        .complete(&request("markdown", "hello"))
        .unwrap_err();

    assert!(error.to_string().contains("model not found"), "{error}");
}

#[test]
fn the_endpoint_can_be_pointed_at_something_that_is_not_openai() {
    // The whole argument for Chat Completions: the same provider reaches a
    // gateway, Azure, or a local server, and the artifact does not change.
    let (url, captured) = serve_once(200, ok_response("from elsewhere"));
    let response = provider(&url)
        .complete(&request("markdown", "hello"))
        .unwrap();

    assert_eq!(response.value, json!("from elsewhere"));
    assert!(captured.recv().is_ok());
}
