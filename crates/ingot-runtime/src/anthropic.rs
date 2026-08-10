//! Anthropic Messages API provider.
//!
//! Feature-gated behind `anthropic`, so the interpreter and its tests carry no
//! HTTP or TLS dependency. Raw HTTP rather than an SDK: there is no official
//! Anthropic SDK for Rust.
//!
//! Three details are load-bearing and easy to get wrong:
//!
//! * **Structured output is how `ask<T>` becomes real.** A declared response
//!   type is sent as a JSON Schema in `output_config.format`, so the model is
//!   constrained rather than merely asked nicely. Prose types are exempt — see
//!   [`crate::schema`].
//! * **Sampling parameters are not sent.** `temperature`, `top_p` and `top_k`
//!   are rejected by current models and would fail the request outright.
//! * **Streaming reuses the non-streaming parser.** The streamed events are
//!   assembled back into the payload a single reply would have carried, and
//!   handed to the same `parse_response`. One parser, two transports: the way a
//!   call was made cannot change the answer it produces.

use std::time::Duration;

use serde_json::{json, Map, Value};

use crate::http::{self, DEFAULT_MAX_RETRIES, DEFAULT_TIMEOUT};
use crate::provider::{
    CompletionRequest, CompletionResponse, DeltaSink, ModelProvider, ModelSelection, ProviderError,
    Usage,
};
use crate::schema::ResponseShape;

const API_URL: &str = "https://api.anthropic.com/v1/messages";
const API_VERSION: &str = "2023-06-01";

/// The prefix an artifact uses to pin this provider: `anthropic/claude-…`.
pub const PROVIDER: &str = "anthropic";

/// Default model. Capability-based requirements resolve to this unless the
/// artifact pins one.
pub const DEFAULT_MODEL: &str = "claude-opus-5";

/// Context window of [`DEFAULT_MODEL`], for checking `context >= N` requirements.
const DEFAULT_MODEL_CONTEXT_TOKENS: i64 = 1_000_000;

/// Capabilities [`DEFAULT_MODEL`] provides, for checking `model requires { ... }`.
const DEFAULT_MODEL_CAPABILITIES: &[&str] = &[
    "tool_calling",
    "structured_output",
    "streaming",
    "vision",
    "reasoning",
    "parallel_tool_calls",
];

pub struct AnthropicProvider {
    api_key: String,
    /// Overrides whatever the artifact asks for. Set from `--model`.
    model_override: Option<String>,
    /// `low` | `medium` | `high` | `xhigh` | `max`. Omitted when `None`.
    effort: Option<String>,
    max_retries: u32,
    timeout: Duration,
    base_url: String,
}

impl AnthropicProvider {
    /// Read the key from `ANTHROPIC_API_KEY`.
    ///
    /// `INGOT_ANTHROPIC_BASE_URL` overrides the endpoint, for a gateway, a
    /// proxy, or the stub server the wire tests run against.
    pub fn from_env() -> Result<AnthropicProvider, ProviderError> {
        let mut provider = AnthropicProvider::with_key(http::key_from_env("ANTHROPIC_API_KEY")?);
        if let Some(url) = http::base_url_from_env("INGOT_ANTHROPIC_BASE_URL") {
            provider = provider.with_base_url(url);
        }
        Ok(provider)
    }

    pub fn with_key(api_key: impl Into<String>) -> AnthropicProvider {
        AnthropicProvider {
            api_key: api_key.into(),
            model_override: None,
            effort: None,
            max_retries: DEFAULT_MAX_RETRIES,
            timeout: DEFAULT_TIMEOUT,
            base_url: API_URL.to_string(),
        }
    }

    pub fn with_model(mut self, model: Option<String>) -> Self {
        self.model_override = model;
        self
    }

    pub fn with_effort(mut self, effort: Option<String>) -> Self {
        self.effort = effort;
        self
    }

    /// Point at a different endpoint. Used by tests against a local stub.
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }

    /// Which model to ask, and why.
    fn resolve_model(&self, selection: &ModelSelection) -> Result<String, ProviderError> {
        if let Some(model) = &self.model_override {
            return Ok(model.clone());
        }
        match selection {
            // `provider/model` in the source; the provider half is ours already.
            ModelSelection::Exact(reference) => Ok(reference
                .split_once('/')
                .map(|(_, model)| model.to_string())
                .unwrap_or_else(|| reference.clone())),
            ModelSelection::Default => Ok(DEFAULT_MODEL.to_string()),
            ModelSelection::Capabilities {
                capabilities,
                min_context_tokens,
            } => {
                for capability in capabilities {
                    if !DEFAULT_MODEL_CAPABILITIES.contains(&capability.as_str()) {
                        return Err(ProviderError::Configuration(format!(
                            "the artifact requires the `{capability}` capability, which \
                             {DEFAULT_MODEL} does not advertise; pin a model with `--model`"
                        )));
                    }
                }
                if let Some(required) = min_context_tokens {
                    if *required > DEFAULT_MODEL_CONTEXT_TOKENS {
                        return Err(ProviderError::Configuration(format!(
                            "the artifact requires a {required}-token context window, but \
                             {DEFAULT_MODEL} provides {DEFAULT_MODEL_CONTEXT_TOKENS}"
                        )));
                    }
                }
                Ok(DEFAULT_MODEL.to_string())
            }
        }
    }

    fn build_body(&self, request: &CompletionRequest) -> Result<Value, ProviderError> {
        let model = self.resolve_model(&request.model)?;

        let mut user_content = String::new();
        for (name, value) in &request.context {
            user_content.push_str(&format!("<{name}>\n"));
            match value {
                Value::String(text) => user_content.push_str(text),
                other => user_content.push_str(
                    &serde_json::to_string_pretty(other).unwrap_or_else(|_| other.to_string()),
                ),
            }
            user_content.push_str(&format!("\n</{name}>\n\n"));
        }
        user_content.push_str(&request.prompt);

        let mut body = Map::new();
        body.insert("model".into(), json!(model));
        body.insert("max_tokens".into(), json!(request.max_tokens));
        body.insert(
            "messages".into(),
            json!([{ "role": "user", "content": user_content }]),
        );
        if let Some(system) = &request.system {
            body.insert("system".into(), json!(system));
        }

        let mut output_config = Map::new();
        if let Some(effort) = &self.effort {
            output_config.insert("effort".into(), json!(effort));
        }
        if let ResponseShape::Schema { schema, .. } = &request.shape {
            output_config.insert(
                "format".into(),
                json!({ "type": "json_schema", "schema": schema }),
            );
        }
        if !output_config.is_empty() {
            body.insert("output_config".into(), Value::Object(output_config));
        }

        // Deliberately absent: temperature, top_p, top_k. Current models reject
        // them, and an artifact that behaves differently per provider is exactly
        // what portability is meant to prevent.
        Ok(Value::Object(body))
    }
}

impl ModelProvider for AnthropicProvider {
    fn name(&self) -> &str {
        "anthropic"
    }

    fn complete(
        &mut self,
        request: &CompletionRequest,
    ) -> Result<CompletionResponse, ProviderError> {
        let body = self.build_body(request)?;
        let payload = http::post_json(
            &self.base_url,
            &[
                ("x-api-key", self.api_key.as_str()),
                ("anthropic-version", API_VERSION),
            ],
            &body,
            self.timeout,
            self.max_retries,
        )?;
        parse_response(request, &payload)
    }

    fn streams(&self) -> bool {
        true
    }

    fn complete_streaming(
        &mut self,
        request: &CompletionRequest,
        on_delta: DeltaSink<'_>,
    ) -> Result<CompletionResponse, ProviderError> {
        let mut body = self.build_body(request)?;
        // Added here rather than inside `build_body`, so the non-streaming body
        // stays exactly what it was: the two transports must differ by this one
        // field and nothing else.
        body["stream"] = json!(true);

        let mut accumulator = StreamAccumulator::default();
        let mut failure: Option<ProviderError> = None;
        let outcome = {
            let mut on_event = |name: &str, data: &Value| {
                // The first failure wins: every event after it describes a
                // message that is already known not to have arrived.
                if failure.is_some() {
                    return;
                }
                if let Err(error) = accumulator.event(name, data, on_delta) {
                    failure = Some(error);
                }
            };
            http::post_sse(
                &self.base_url,
                &[
                    ("x-api-key", self.api_key.as_str()),
                    ("anthropic-version", API_VERSION),
                ],
                &body,
                self.timeout,
                self.max_retries,
                &mut on_event,
            )
        };

        // What the provider said went wrong outranks how the stream ended: an
        // `error` event explains the truncated stream that follows it.
        if let Some(error) = failure {
            return Err(error);
        }
        outcome?;

        parse_response(request, &accumulator.finish()?)
    }
}

/// Turn a Messages API response into a typed value.
pub(crate) fn parse_response(
    request: &CompletionRequest,
    payload: &Value,
) -> Result<CompletionResponse, ProviderError> {
    let stop_reason = payload
        .get("stop_reason")
        .and_then(Value::as_str)
        .unwrap_or_default();

    // Check the stop reason before touching content: on a refusal the content
    // array is empty or partial, and indexing it would panic or lie.
    if stop_reason == "refusal" {
        let details = payload.get("stop_details");
        return Err(ProviderError::Refused {
            category: details
                .and_then(|d| d.get("category"))
                .and_then(Value::as_str)
                .map(str::to_string),
            explanation: details
                .and_then(|d| d.get("explanation"))
                .and_then(Value::as_str)
                .map(str::to_string),
        });
    }
    if stop_reason == "max_tokens" {
        return Err(ProviderError::Truncated {
            limit: request.max_tokens,
        });
    }

    let text = payload
        .get("content")
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|block| block.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default();

    if text.trim().is_empty() {
        return Err(ProviderError::InvalidResponse(format!(
            "the provider returned no text (stop reason: `{stop_reason}`)"
        )));
    }

    let value = match &request.shape {
        ResponseShape::Prose => Value::String(text),
        ResponseShape::FreeJson => serde_json::from_str(text.trim()).map_err(|error| {
            ProviderError::InvalidResponse(format!("expected JSON, got: {error}"))
        })?,
        ResponseShape::Schema { wrapped, .. } => {
            let parsed: Value = serde_json::from_str(text.trim()).map_err(|error| {
                ProviderError::InvalidResponse(format!(
                    "the schema-constrained response was not valid JSON: {error}"
                ))
            })?;
            if *wrapped {
                parsed.get("value").cloned().ok_or_else(|| {
                    ProviderError::InvalidResponse(
                        "the wrapped response has no `value` field".to_string(),
                    )
                })?
            } else {
                parsed
            }
        }
    };

    let usage = payload.get("usage");
    Ok(CompletionResponse {
        value,
        usage: Usage {
            input_tokens: usage
                .and_then(|u| u.get("input_tokens"))
                .and_then(Value::as_u64)
                .unwrap_or(0),
            output_tokens: usage
                .and_then(|u| u.get("output_tokens"))
                .and_then(Value::as_u64)
                .unwrap_or(0),
            cache_read_tokens: usage
                .and_then(|u| u.get("cache_read_input_tokens"))
                .and_then(Value::as_u64)
                .unwrap_or(0),
        },
        model: payload
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or(DEFAULT_MODEL)
            .to_string(),
    })
}

/// The pieces of a streamed reply, assembled back into the reply itself.
///
/// **One parser, two transports.** This type's only job is to rebuild the exact
/// payload a non-streaming call would have returned and hand it to
/// [`parse_response`]. There is deliberately no second parser, and that absence
/// is the guarantee: a streamed call and a non-streamed call produce identical
/// values *and* identical errors — truncation, refusal, invalid JSON, an empty
/// answer — for the same content. Two parsers would drift, and the drift would
/// surface as an artifact behaving differently depending on how it was run.
///
/// Kept separate from the HTTP call so the whole of it is testable without a
/// socket.
#[derive(Default)]
struct StreamAccumulator {
    id: Option<String>,
    text: String,
    model: Option<String>,
    stop_reason: Option<String>,
    stop_details: Option<Value>,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
}

impl StreamAccumulator {
    /// Fold one event in, showing a watcher whatever is worth seeing.
    ///
    /// Unknown event names are ignored rather than refused: a provider that adds
    /// an event type must not break a run that already works.
    fn event(
        &mut self,
        name: &str,
        data: &Value,
        on_delta: DeltaSink<'_>,
    ) -> Result<(), ProviderError> {
        match name {
            "message_start" => {
                let message = data.get("message");
                if let Some(id) = message.and_then(|m| m.get("id")).and_then(Value::as_str) {
                    self.id = Some(id.to_string());
                }
                if let Some(model) = message.and_then(|m| m.get("model")).and_then(Value::as_str) {
                    self.model = Some(model.to_string());
                }
                let usage = message.and_then(|m| m.get("usage"));
                if let Some(tokens) = usage
                    .and_then(|u| u.get("input_tokens"))
                    .and_then(Value::as_u64)
                {
                    self.input_tokens = tokens;
                }
                if let Some(tokens) = usage
                    .and_then(|u| u.get("cache_read_input_tokens"))
                    .and_then(Value::as_u64)
                {
                    self.cache_read_tokens = tokens;
                }
            }
            "content_block_delta" => {
                // Only `text_delta` is shown. Thinking deltas and tool-input
                // deltas are dropped, because `parse_response` drops the blocks
                // they belong to when it collects text. The property that buys:
                // what a watcher sees live is exactly the text that becomes the
                // answer — never a sentence the run then throws away.
                let text = data
                    .get("delta")
                    .filter(|d| d.get("type").and_then(Value::as_str) == Some("text_delta"))
                    .and_then(|d| d.get("text"))
                    .and_then(Value::as_str);
                if let Some(text) = text {
                    self.text.push_str(text);
                    on_delta(text);
                }
            }
            "message_delta" => {
                let delta = data.get("delta");
                if let Some(reason) = delta
                    .and_then(|d| d.get("stop_reason"))
                    .and_then(Value::as_str)
                {
                    self.stop_reason = Some(reason.to_string());
                }
                if let Some(details) = delta
                    .and_then(|d| d.get("stop_details"))
                    .filter(|details| !details.is_null())
                {
                    self.stop_details = Some(details.clone());
                }
                if let Some(tokens) = data
                    .get("usage")
                    .and_then(|u| u.get("output_tokens"))
                    .and_then(Value::as_u64)
                {
                    self.output_tokens = tokens;
                }
            }
            "error" => {
                // `Request` rather than `Transport`: the connection did exactly
                // what it was asked and delivered this event intact, so calling
                // it a transport failure would send an operator looking at the
                // wrong thing. This is the service reporting the same class of
                // failure a 5xx would have carried had it arrived before the
                // stream opened, so it is reported the same way.
                let message = data
                    .get("error")
                    .and_then(|error| error.get("message"))
                    .and_then(Value::as_str)
                    .unwrap_or("the provider reported an error mid-stream");
                return Err(ProviderError::Request {
                    status: 500,
                    message: message.to_string(),
                });
            }
            // `content_block_start`, `content_block_stop`, `message_stop` and
            // `ping` carry nothing the answer depends on, and neither does an
            // event type that did not exist when this was written.
            _ => {}
        }
        Ok(())
    }

    /// The finished payload, or the reason there is not one.
    ///
    /// A stream that stops without ever reporting a stop reason is a connection
    /// cut mid-answer, not a successful empty answer. The text collected so far
    /// may be a fragment, and a fragment that parses is worse than no answer:
    /// it passes silently.
    fn finish(self) -> Result<Value, ProviderError> {
        if self.stop_reason.is_none() {
            return Err(ProviderError::Transport(
                "the stream ended before the message did: no stop reason arrived, so the \
                 answer is incomplete"
                    .to_string(),
            ));
        }
        Ok(self.into_payload())
    }

    /// The same shape a non-streaming Messages reply has, down to the field
    /// names, so [`parse_response`] cannot tell where it came from.
    fn into_payload(self) -> Value {
        json!({
            "id": self.id,
            "model": self.model,
            "stop_reason": self.stop_reason,
            "stop_details": self.stop_details,
            "content": [{ "type": "text", "text": self.text }],
            "usage": {
                "input_tokens": self.input_tokens,
                "output_tokens": self.output_tokens,
                "cache_read_input_tokens": self.cache_read_tokens,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema;
    use std::collections::BTreeMap;

    fn request(response_type: &str) -> CompletionRequest {
        let shape = schema::response_shape(response_type, &BTreeMap::new()).unwrap();
        CompletionRequest {
            node: "n0".into(),
            model: ModelSelection::Default,
            system: None,
            prompt: "Write something".into(),
            context: vec![("document".into(), json!("the source text"))],
            response_type: response_type.into(),
            shape,
            max_tokens: 4096,
        }
    }

    fn provider() -> AnthropicProvider {
        AnthropicProvider::with_key("test-key")
    }

    #[test]
    fn sampling_parameters_are_never_sent() {
        let body = provider().build_body(&request("markdown")).unwrap();
        for rejected in ["temperature", "top_p", "top_k"] {
            assert!(
                body.get(rejected).is_none(),
                "`{rejected}` must not be sent"
            );
        }
    }

    #[test]
    fn prose_requests_carry_no_schema() {
        let body = provider().build_body(&request("markdown")).unwrap();
        assert!(
            body.get("output_config").is_none(),
            "asking for markdown must not constrain the model to JSON"
        );
    }

    #[test]
    fn structured_requests_carry_a_schema() {
        let body = provider().build_body(&request("string[]")).unwrap();
        let format = &body["output_config"]["format"];
        assert_eq!(format["type"], "json_schema");
        assert_eq!(format["schema"]["properties"]["value"]["type"], "array");
    }

    #[test]
    fn context_is_wrapped_in_named_tags() {
        let body = provider().build_body(&request("markdown")).unwrap();
        let content = body["messages"][0]["content"].as_str().unwrap();
        assert!(content.contains("<document>"), "{content}");
        assert!(content.contains("the source text"), "{content}");
        assert!(content.ends_with("Write something"), "{content}");
    }

    #[test]
    fn an_exact_reference_drops_the_provider_prefix() {
        let provider = provider();
        let model = provider
            .resolve_model(&ModelSelection::Exact("anthropic/claude-opus-5".into()))
            .unwrap();
        assert_eq!(model, "claude-opus-5");
    }

    #[test]
    fn an_unknown_capability_is_refused_rather_than_guessed() {
        let error = provider()
            .resolve_model(&ModelSelection::Capabilities {
                capabilities: vec!["telepathy".into()],
                min_context_tokens: None,
            })
            .unwrap_err();
        assert!(error.to_string().contains("telepathy"), "{error}");
    }

    #[test]
    fn a_context_requirement_beyond_the_model_is_refused() {
        let error = provider()
            .resolve_model(&ModelSelection::Capabilities {
                capabilities: vec![],
                min_context_tokens: Some(DEFAULT_MODEL_CONTEXT_TOKENS + 1),
            })
            .unwrap_err();
        assert!(error.to_string().contains("context window"), "{error}");
    }

    #[test]
    fn a_refusal_is_reported_before_content_is_read() {
        let payload = json!({
            "stop_reason": "refusal",
            "stop_details": { "category": "cyber", "explanation": "declined" },
            "content": [],
        });
        let error = parse_response(&request("markdown"), &payload).unwrap_err();
        assert!(matches!(error, ProviderError::Refused { .. }));
        assert!(error.to_string().contains("cyber"), "{error}");
    }

    #[test]
    fn truncation_is_reported_rather_than_returned_as_a_result() {
        let payload = json!({
            "stop_reason": "max_tokens",
            "content": [{"type": "text", "text": "half an ans"}],
        });
        let error = parse_response(&request("markdown"), &payload).unwrap_err();
        assert!(matches!(error, ProviderError::Truncated { .. }));
    }

    #[test]
    fn prose_responses_come_back_as_text() {
        let payload = json!({
            "stop_reason": "end_turn",
            "model": "claude-opus-5",
            "content": [{"type": "text", "text": "# Title\n\nBody"}],
            "usage": {"input_tokens": 12, "output_tokens": 5},
        });
        let response = parse_response(&request("markdown"), &payload).unwrap();
        assert_eq!(response.value, json!("# Title\n\nBody"));
        assert_eq!(response.usage.input_tokens, 12);
        assert_eq!(response.model, "claude-opus-5");
    }

    #[test]
    fn wrapped_structured_responses_are_unwrapped() {
        let payload = json!({
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "{\"value\": [\"a\", \"b\"]}"}],
            "usage": {"input_tokens": 1, "output_tokens": 1},
        });
        let response = parse_response(&request("string[]"), &payload).unwrap();
        assert_eq!(response.value, json!(["a", "b"]));
    }

    #[test]
    fn a_non_json_structured_response_is_an_error() {
        let payload = json!({
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "not json at all"}],
        });
        let error = parse_response(&request("string[]"), &payload).unwrap_err();
        assert!(matches!(error, ProviderError::InvalidResponse(_)));
    }

    #[test]
    fn an_empty_response_is_an_error() {
        let payload = json!({"stop_reason": "end_turn", "content": []});
        let error = parse_response(&request("markdown"), &payload).unwrap_err();
        assert!(error.to_string().contains("no text"), "{error}");
    }

    #[test]
    fn thinking_blocks_are_ignored_when_collecting_text() {
        let payload = json!({
            "stop_reason": "end_turn",
            "content": [
                {"type": "thinking", "thinking": ""},
                {"type": "text", "text": "the answer"},
            ],
        });
        let response = parse_response(&request("markdown"), &payload).unwrap();
        assert_eq!(response.value, json!("the answer"));
    }

    /// Drive the accumulator over a whole stream, exactly as
    /// `complete_streaming` does, and report both what it built and what a
    /// watcher would have seen.
    fn accumulate(events: &[(&str, Value)]) -> (Result<Value, ProviderError>, Vec<String>) {
        let mut seen: Vec<String> = Vec::new();
        let mut accumulator = StreamAccumulator::default();
        let mut failure = None;
        {
            let mut on_delta = |text: &str| seen.push(text.to_string());
            for (name, data) in events {
                if let Err(error) = accumulator.event(name, data, &mut on_delta) {
                    failure = Some(error);
                    break;
                }
            }
        }
        match failure {
            Some(error) => (Err(error), seen),
            None => (accumulator.finish(), seen),
        }
    }

    fn started() -> (&'static str, Value) {
        (
            "message_start",
            json!({"message": {
                "id": "msg_test",
                "model": "claude-opus-5",
                "usage": {"input_tokens": 12, "cache_read_input_tokens": 3},
            }}),
        )
    }

    fn text_delta(text: &str) -> (&'static str, Value) {
        (
            "content_block_delta",
            json!({"index": 0, "delta": {"type": "text_delta", "text": text}}),
        )
    }

    fn stopped(reason: &str) -> (&'static str, Value) {
        (
            "message_delta",
            json!({"delta": {"stop_reason": reason}, "usage": {"output_tokens": 5}}),
        )
    }

    #[test]
    fn the_provider_advertises_that_it_streams() {
        assert!(provider().streams());
    }

    #[test]
    fn the_non_streaming_body_never_asks_for_a_stream() {
        let body = provider().build_body(&request("markdown")).unwrap();
        assert!(
            body.get("stream").is_none(),
            "`complete` must not send `stream`"
        );
    }

    #[test]
    fn streamed_text_is_accumulated_in_arrival_order() {
        let (payload, _) = accumulate(&[
            started(),
            text_delta("# Ti"),
            text_delta("tle\n\nBo"),
            text_delta("dy"),
            stopped("end_turn"),
        ]);
        let response = parse_response(&request("markdown"), &payload.unwrap()).unwrap();
        assert_eq!(response.value, json!("# Title\n\nBody"));
    }

    #[test]
    fn deltas_are_handed_over_in_arrival_order() {
        let (_, seen) = accumulate(&[
            started(),
            text_delta("one "),
            text_delta("two "),
            text_delta("three"),
            stopped("end_turn"),
        ]);
        assert_eq!(seen, ["one ", "two ", "three"]);
    }

    #[test]
    fn only_the_text_that_becomes_the_answer_is_shown_live() {
        let (payload, seen) = accumulate(&[
            started(),
            (
                "content_block_start",
                json!({"index": 0, "content_block": {"type": "thinking"}}),
            ),
            (
                "content_block_delta",
                json!({"index": 0, "delta": {
                    "type": "thinking_delta",
                    "thinking": "weighing it up",
                }}),
            ),
            ("content_block_stop", json!({"index": 0})),
            text_delta("the answer"),
            (
                "content_block_delta",
                json!({"index": 1, "delta": {
                    "type": "input_json_delta",
                    "partial_json": "{\"query\":",
                }}),
            ),
            stopped("end_turn"),
        ]);
        // No reasoning and no half-built tool arguments: what the watcher saw is
        // character-for-character the value the run uses.
        assert_eq!(seen.concat(), "the answer");
        let response = parse_response(&request("markdown"), &payload.unwrap()).unwrap();
        assert_eq!(response.value, json!("the answer"));
    }

    #[test]
    fn a_truncated_stream_produces_the_same_error_as_a_truncated_response() {
        let (payload, _) =
            accumulate(&[started(), text_delta("half an ans"), stopped("max_tokens")]);
        let error = parse_response(&request("markdown"), &payload.unwrap()).unwrap_err();
        assert!(matches!(error, ProviderError::Truncated { .. }), "{error}");
    }

    #[test]
    fn a_refused_stream_produces_the_same_error_as_a_refused_response() {
        let (payload, _) = accumulate(&[
            started(),
            (
                "message_delta",
                json!({
                    "delta": {
                        "stop_reason": "refusal",
                        "stop_details": {"category": "cyber", "explanation": "declined"},
                    },
                    "usage": {"output_tokens": 0},
                }),
            ),
        ]);
        let error = parse_response(&request("markdown"), &payload.unwrap()).unwrap_err();
        assert!(matches!(error, ProviderError::Refused { .. }), "{error}");
        assert!(error.to_string().contains("cyber"), "{error}");
    }

    #[test]
    fn usage_is_carried_through_the_stream() {
        let (payload, _) = accumulate(&[started(), text_delta("done"), stopped("end_turn")]);
        let response = parse_response(&request("markdown"), &payload.unwrap()).unwrap();
        assert_eq!(response.usage.input_tokens, 12);
        assert_eq!(response.usage.output_tokens, 5);
        assert_eq!(response.usage.cache_read_tokens, 3);
        assert_eq!(response.model, "claude-opus-5");
    }

    #[test]
    fn an_unknown_event_is_ignored_rather_than_failing_the_run() {
        let (payload, _) = accumulate(&[
            started(),
            ("ping", json!({})),
            ("message_reconsidered", json!({"invented": true})),
            text_delta("still fine"),
            stopped("end_turn"),
        ]);
        let response = parse_response(&request("markdown"), &payload.unwrap()).unwrap();
        assert_eq!(response.value, json!("still fine"));
    }

    #[test]
    fn an_error_event_is_reported_rather_than_swallowed() {
        let (payload, _) = accumulate(&[
            started(),
            text_delta("partial"),
            (
                "error",
                json!({"error": {"type": "overloaded_error", "message": "overloaded"}}),
            ),
        ]);
        let error = payload.unwrap_err();
        assert!(
            matches!(error, ProviderError::Request { status: 500, .. }),
            "{error}"
        );
        assert!(error.to_string().contains("overloaded"), "{error}");
    }

    #[test]
    fn a_stream_that_ends_before_the_message_does_is_not_an_answer() {
        let (payload, _) = accumulate(&[started(), text_delta("half an ans")]);
        let error = payload.unwrap_err();
        assert!(matches!(error, ProviderError::Transport(_)), "{error}");
        assert!(error.to_string().contains("ended before"), "{error}");
    }

    #[test]
    fn the_same_answer_streamed_or_at_once_produces_the_same_response() {
        let at_once = json!({
            "id": "msg_test",
            "model": "claude-opus-5",
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "{\"value\": [\"a\", \"b\"]}"}],
            "usage": {"input_tokens": 12, "output_tokens": 5, "cache_read_input_tokens": 3},
        });
        let (streamed, _) = accumulate(&[
            started(),
            text_delta("{\"value\": "),
            text_delta("[\"a\", \"b\"]}"),
            stopped("end_turn"),
        ]);
        assert_eq!(
            parse_response(&request("string[]"), &streamed.unwrap()).unwrap(),
            parse_response(&request("string[]"), &at_once).unwrap()
        );
    }
}
