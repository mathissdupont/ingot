//! Wire-level tests for the Google Gemini provider.
//!
//! The stub server lives in `support`; these assert what this protocol
//! specifically sends and understands. Two of its differences from the other
//! providers only exist on the wire and cannot be checked any other way: the
//! model is a path segment, and the credential is a header rather than a query
//! parameter.

#![cfg(feature = "google")]

mod support;

use std::collections::BTreeMap;

use ingot_runtime::google::GoogleProvider;
use ingot_runtime::provider::{CompletionRequest, ModelProvider, ModelSelection, ProviderError};
use ingot_runtime::schema;
use serde_json::{json, Value};
use support::{serve_once as serve_at, serve_stream};

/// The stub answers whatever arrives, so the URL it hands back is the API base
/// the provider appends `/models/{model}:{method}` to — exactly the arrangement
/// an operator has when they point `base-url` at a gateway.
fn serve_base(response: Value) -> (String, std::sync::mpsc::Receiver<support::Captured>) {
    serve_at("", 200, response)
}

fn request(response_type: &str, prompt: &str) -> CompletionRequest {
    let shape = schema::response_shape(response_type, &BTreeMap::new()).unwrap();
    CompletionRequest {
        node: "n0".into(),
        model: ModelSelection::Exact("google/gemini-2.5-pro".into()),
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
        "candidates": [{
            "content": { "role": "model", "parts": [{ "text": text }] },
            "finishReason": "STOP",
        }],
        "usageMetadata": { "promptTokenCount": 42, "candidatesTokenCount": 7 },
        "modelVersion": "gemini-2.5-pro",
    })
}

/// The same answer, framed as the event stream that would have produced it.
fn ok_stream(pieces: &[&str]) -> Vec<(String, String)> {
    let mut events: Vec<(String, String)> = pieces
        .iter()
        .map(|piece| {
            (
                String::new(),
                json!({
                    "candidates": [{ "content": { "parts": [{ "text": piece }] } }],
                    "modelVersion": "gemini-2.5-pro",
                })
                .to_string(),
            )
        })
        .collect();
    events.push((
        String::new(),
        json!({
            "candidates": [{ "content": { "parts": [] }, "finishReason": "STOP" }],
            "usageMetadata": { "promptTokenCount": 42, "candidatesTokenCount": 7 },
        })
        .to_string(),
    ));
    events
}

#[test]
fn the_model_and_the_method_travel_in_the_path() {
    let (url, captured) = serve_base(ok_response("# Summary\n\nShort."));
    let mut provider = GoogleProvider::with_key("test-key").with_base_url(url);

    let response = provider
        .complete(&request("markdown", "Summarise it"))
        .unwrap();
    assert_eq!(response.value, json!("# Summary\n\nShort."));
    assert_eq!(response.usage.input_tokens, 42);
    assert_eq!(response.model, "gemini-2.5-pro");

    let seen = captured
        .recv()
        .expect("the stub should have served a request");
    assert_eq!(seen.target, "/models/gemini-2.5-pro:generateContent");
    assert!(
        seen.body.get("model").is_none(),
        "the model belongs in the path for this protocol, not the body: {}",
        seen.body
    );
}

#[test]
fn the_credential_is_a_header_and_never_reaches_the_url() {
    // A URL travels through proxy logs, error messages and crash reports in a
    // way a header does not. The API accepts `?key=` and this provider will
    // not use it.
    let (url, captured) = serve_base(ok_response("ok"));
    let mut provider = GoogleProvider::with_key("test-key").with_base_url(url);
    provider.complete(&request("markdown", "Go")).unwrap();

    let seen = captured.recv().expect("served");
    assert_eq!(
        seen.headers.get("x-goog-api-key").map(String::as_str),
        Some("test-key")
    );
    assert!(!seen.target.contains("test-key"), "{}", seen.target);
    assert!(!seen.target.contains("key="), "{}", seen.target);
}

#[test]
fn the_prompt_and_the_system_instruction_are_sent_the_way_this_protocol_wants_them() {
    let (url, captured) = serve_base(ok_response("ok"));
    let mut provider = GoogleProvider::with_key("k").with_base_url(url);
    provider
        .complete(&request("markdown", "Summarise it"))
        .unwrap();

    let seen = captured.recv().expect("served");
    let text = seen.body["contents"][0]["parts"][0]["text"]
        .as_str()
        .unwrap();
    assert!(text.contains("<document>"), "{text}");
    assert!(text.ends_with("Summarise it"), "{text}");
    assert_eq!(
        seen.body["systemInstruction"]["parts"][0]["text"],
        "You are terse."
    );
    assert_eq!(seen.body["generationConfig"]["maxOutputTokens"], 2048);

    // Sampling parameters are rejected by current models, and an artifact that
    // behaves differently per provider is what portability is meant to prevent.
    let generation = &seen.body["generationConfig"];
    for rejected in ["temperature", "topP", "topK"] {
        assert!(
            generation.get(rejected).is_none(),
            "`{rejected}` must not be sent"
        );
    }
}

#[test]
fn a_typed_request_sends_an_openapi_schema_and_unwraps_the_answer() {
    let (url, captured) = serve_base(ok_response(r#"{"value":["one","two"]}"#));
    let mut provider = GoogleProvider::with_key("k").with_base_url(url);

    let response = provider.complete(&request("string[]", "List two")).unwrap();
    assert_eq!(response.value, json!(["one", "two"]));

    let seen = captured.recv().expect("served");
    let schema = &seen.body["generationConfig"]["responseSchema"];
    // Upper case: this is OpenAPI's Schema, not JSON Schema.
    assert_eq!(schema["type"], "OBJECT");
    assert_eq!(schema["properties"]["value"]["type"], "ARRAY");
    assert!(schema.get("additionalProperties").is_none(), "{schema}");
    assert_eq!(
        seen.body["generationConfig"]["responseMimeType"],
        "application/json"
    );
}

#[test]
fn a_blocked_prompt_is_a_refusal_rather_than_an_empty_answer() {
    let (url, _captured) = serve_base(json!({
        "promptFeedback": { "blockReason": "SAFETY" },
    }));
    let mut provider = GoogleProvider::with_key("k").with_base_url(url);
    let error = provider.complete(&request("markdown", "Go")).unwrap_err();
    assert!(matches!(error, ProviderError::Refused { .. }), "{error}");
}

#[test]
fn truncation_is_reported_the_way_it_is_everywhere_else() {
    let (url, _captured) = serve_base(json!({
        "candidates": [{
            "content": { "parts": [{ "text": "half an ans" }] },
            "finishReason": "MAX_TOKENS",
        }],
    }));
    let mut provider = GoogleProvider::with_key("k").with_base_url(url);
    let error = provider.complete(&request("markdown", "Go")).unwrap_err();
    assert!(
        matches!(error, ProviderError::Truncated { limit: 2048 }),
        "{error}"
    );
}

#[test]
fn a_streaming_request_asks_for_server_sent_events() {
    let (url, captured) = serve_stream("", ok_stream(&["Half ", "an ", "answer"]));
    let mut provider = GoogleProvider::with_key("k").with_base_url(url);

    let mut shown = Vec::new();
    let response = provider
        .complete_streaming(&request("markdown", "Go"), &mut |text| {
            shown.push(text.to_string())
        })
        .unwrap();

    assert_eq!(response.value, json!("Half an answer"));
    assert_eq!(shown, vec!["Half ", "an ", "answer"]);

    let seen = captured.recv().expect("served");
    // The default streaming form is a JSON array delivered as one document,
    // parseable only once complete — which is not streaming in any sense a
    // watcher benefits from.
    assert_eq!(
        seen.target,
        "/models/gemini-2.5-pro:streamGenerateContent?alt=sse"
    );
}

#[test]
fn a_streamed_answer_is_identical_to_the_same_answer_sent_at_once() {
    // One parser, two transports. The two paths must not be able to drift.
    let (url, _) = serve_base(ok_response("Half an answer"));
    let mut whole = GoogleProvider::with_key("k").with_base_url(url);
    let at_once = whole.complete(&request("markdown", "Go")).unwrap();

    let (url, _) = serve_stream("", ok_stream(&["Half ", "an ", "answer"]));
    let mut streamed_provider = GoogleProvider::with_key("k").with_base_url(url);
    let streamed = streamed_provider
        .complete_streaming(&request("markdown", "Go"), &mut |_| {})
        .unwrap();

    assert_eq!(streamed, at_once);
}

#[test]
fn a_stream_cut_before_the_answer_finishes_is_a_transport_failure() {
    let events = vec![(
        String::new(),
        json!({ "candidates": [{ "content": { "parts": [{ "text": "half" }] } }] }).to_string(),
    )];
    let (url, _) = serve_stream("", events);
    let mut provider = GoogleProvider::with_key("k").with_base_url(url);

    let error = provider
        .complete_streaming(&request("markdown", "Go"), &mut |_| {})
        .unwrap_err();
    assert!(matches!(error, ProviderError::Transport(_)), "{error}");
    assert!(error.to_string().contains("ended before"), "{error}");
}

#[test]
fn this_provider_says_it_streams_so_the_interpreter_can_raise_the_cap() {
    assert!(GoogleProvider::with_key("k").streams());
}
