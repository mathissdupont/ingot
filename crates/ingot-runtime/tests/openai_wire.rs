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
use support::serve_stream as stream_at;

fn serve_once(
    status: u16,
    response: Value,
) -> (String, std::sync::mpsc::Receiver<support::Captured>) {
    serve_at("/v1/chat/completions", status, response)
}

fn serve_stream(events: Vec<String>) -> (String, std::sync::mpsc::Receiver<support::Captured>) {
    // Every name is empty: an OpenAI-compatible stream frames its chunks with
    // `data:` alone and carries no `event:` line.
    stream_at(
        "/v1/chat/completions",
        events
            .into_iter()
            .map(|data| (String::new(), data))
            .collect(),
    )
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

/// The chunks a server sends for `content`, framed the way a real one frames
/// them: a role-only opener, one chunk per word, the stop, a usage-only chunk
/// with an empty `choices`, then the `[DONE]` terminator.
///
/// The usage matches [`ok_response`], so a streamed answer and a non-streamed
/// one can be compared whole.
fn streamed(content: &str) -> Vec<String> {
    let mut chunks = vec![json!({
        "model": "gpt-test",
        "choices": [{ "delta": { "role": "assistant", "content": "" } }],
    })];
    for fragment in content.split_inclusive(' ') {
        chunks.push(json!({
            "model": "gpt-test",
            "choices": [{ "delta": { "content": fragment } }],
        }));
    }
    chunks.push(json!({
        "model": "gpt-test",
        "choices": [{ "delta": {}, "finish_reason": "stop" }],
    }));
    chunks.push(json!({
        "model": "gpt-test",
        "choices": [],
        "usage": { "prompt_tokens": 42, "completion_tokens": 7 },
    }));

    let mut events: Vec<String> = chunks.iter().map(|chunk| chunk.to_string()).collect();
    events.push("[DONE]".to_string());
    events
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

#[test]
fn a_streamed_request_asks_the_server_to_stream_and_to_report_usage() {
    let (url, captured) = serve_stream(streamed("ok"));
    provider(&url)
        .complete_streaming(&request("markdown", "hello"), &mut |_| {})
        .expect("the stub streams");

    let seen = captured.recv().expect("the server saw a request");
    assert_eq!(seen.body["stream"], json!(true));
    // Without this most servers send no usage at all on a streamed call, and a
    // run that cannot see its token usage cannot enforce its budget.
    assert_eq!(seen.body["stream_options"]["include_usage"], json!(true));
    assert_eq!(
        seen.headers.get("authorization").map(String::as_str),
        Some("Bearer stub-key")
    );
    assert_eq!(
        seen.headers.get("accept").map(String::as_str),
        Some("text/event-stream")
    );
}

#[test]
fn a_streamed_prose_answer_is_identical_to_the_same_answer_returned_at_once() {
    // One parser, two transports: the point of reassembling the chunks into the
    // non-streamed shape is that neither the value nor the usage can drift.
    let (url, _captured) = serve_once(200, ok_response("# Summary of it"));
    let at_once = provider(&url)
        .complete(&request("markdown", "Summarise it"))
        .expect("the stub answers");

    let (url, _captured) = serve_stream(streamed("# Summary of it"));
    let mut shown = String::new();
    let in_pieces = provider(&url)
        .complete_streaming(&request("markdown", "Summarise it"), &mut |text| {
            shown.push_str(text)
        })
        .expect("the stub streams");

    assert_eq!(in_pieces, at_once);
    assert_eq!(shown, "# Summary of it");
}

#[test]
fn text_reaches_the_watcher_in_pieces_rather_than_all_at_once() {
    let (url, _captured) = serve_stream(streamed("one two three"));
    let mut shown: Vec<String> = Vec::new();
    let response = provider(&url)
        .complete_streaming(&request("markdown", "count"), &mut |text| {
            shown.push(text.to_string())
        })
        .expect("the stub streams");

    assert!(shown.len() > 1, "the answer arrived whole: {shown:?}");
    assert_eq!(shown.concat(), "one two three");
    assert_eq!(response.value, json!("one two three"));
}

#[test]
fn the_done_terminator_is_framing_rather_than_a_chunk() {
    let whole_answer = json!({
        "model": "gpt-test",
        "choices": [{ "delta": { "content": "ok" }, "finish_reason": "stop" }],
    });
    let (url, _captured) = serve_stream(vec![whole_answer.to_string(), "[DONE]".to_string()]);
    let response = provider(&url)
        .complete_streaming(&request("markdown", "hello"), &mut |_| {})
        .expect("[DONE] is not an error");

    assert_eq!(response.value, json!("ok"));
}

#[test]
fn a_stream_cut_before_the_completion_is_a_transport_failure() {
    // No finish reason ever arrived. That is a broken connection, not a short
    // answer, and calling it a success would hand the run half a document.
    let half = json!({
        "model": "gpt-test",
        "choices": [{ "delta": { "content": "half an ans" } }],
    });
    let (url, _captured) = serve_stream(vec![half.to_string()]);
    let error = provider(&url)
        .complete_streaming(&request("markdown", "write a lot"), &mut |_| {})
        .unwrap_err();

    assert!(matches!(error, ProviderError::Transport(_)), "{error}");
    assert!(
        error.to_string().contains("ended before the completion"),
        "{error}"
    );
}

#[test]
fn a_truncated_stream_is_reported_the_way_a_truncated_response_is() {
    let events = vec![
        json!({ "model": "gpt-test", "choices": [{ "delta": { "content": "half an ans" } }] })
            .to_string(),
        json!({ "model": "gpt-test", "choices": [{ "delta": {}, "finish_reason": "length" }] })
            .to_string(),
        "[DONE]".to_string(),
    ];
    let (url, _captured) = serve_stream(events);
    let error = provider(&url)
        .complete_streaming(&request("markdown", "write a lot"), &mut |_| {})
        .unwrap_err();

    assert!(
        matches!(error, ProviderError::Truncated { limit: 2048 }),
        "{error}"
    );
}

#[test]
fn this_provider_says_it_streams_so_the_interpreter_can_raise_the_cap() {
    assert!(provider("http://127.0.0.1:1/v1/chat/completions").streams());
}
