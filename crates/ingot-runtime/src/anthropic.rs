//! Anthropic Messages API provider.
//!
//! Feature-gated behind `anthropic`, so the interpreter and its tests carry no
//! HTTP or TLS dependency. Raw HTTP rather than an SDK: there is no official
//! Anthropic SDK for Rust.
//!
//! Two details are load-bearing and easy to get wrong:
//!
//! * **Structured output is how `ask<T>` becomes real.** A declared response
//!   type is sent as a JSON Schema in `output_config.format`, so the model is
//!   constrained rather than merely asked nicely. Prose types are exempt — see
//!   [`crate::schema`].
//! * **Sampling parameters are not sent.** `temperature`, `top_p` and `top_k`
//!   are rejected by current models and would fail the request outright.

use std::time::Duration;

use serde_json::{json, Map, Value};

use crate::provider::{
    CompletionRequest, CompletionResponse, ModelProvider, ModelSelection, ProviderError, Usage,
};
use crate::schema::ResponseShape;

const API_URL: &str = "https://api.anthropic.com/v1/messages";
const API_VERSION: &str = "2023-06-01";

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
        let api_key = std::env::var("ANTHROPIC_API_KEY").map_err(|_| {
            ProviderError::Configuration(
                "ANTHROPIC_API_KEY is not set. Export it, or run with \
                 `--provider replay` against a recorded cassette."
                    .to_string(),
            )
        })?;
        if api_key.trim().is_empty() {
            return Err(ProviderError::Configuration(
                "ANTHROPIC_API_KEY is set but empty".to_string(),
            ));
        }
        let mut provider = AnthropicProvider::with_key(api_key);
        if let Ok(url) = std::env::var("INGOT_ANTHROPIC_BASE_URL") {
            if !url.trim().is_empty() {
                provider = provider.with_base_url(url);
            }
        }
        Ok(provider)
    }

    pub fn with_key(api_key: impl Into<String>) -> AnthropicProvider {
        AnthropicProvider {
            api_key: api_key.into(),
            model_override: None,
            effort: None,
            max_retries: 3,
            timeout: Duration::from_secs(180),
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
        let mut attempt = 0;

        loop {
            attempt += 1;
            let response = ureq::post(&self.base_url)
                .config()
                .timeout_global(Some(self.timeout))
                .build()
                .header("content-type", "application/json")
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", API_VERSION)
                .send_json(&body);

            match response {
                Ok(mut ok) => {
                    let payload: Value = ok
                        .body_mut()
                        .read_json()
                        .map_err(|error| ProviderError::Transport(error.to_string()))?;
                    return parse_response(request, &payload);
                }
                Err(ureq::Error::StatusCode(status)) => {
                    let retryable = status == 429 || status >= 500;
                    if retryable && attempt <= self.max_retries {
                        // No jitter: runs should be reproducible, and the retry
                        // count is small enough that thundering herds are not
                        // the failure mode a reference interpreter guards against.
                        std::thread::sleep(Duration::from_millis(500 * u64::from(attempt)));
                        continue;
                    }
                    if status == 429 {
                        return Err(ProviderError::RateLimited {
                            retry_after_seconds: None,
                        });
                    }
                    return Err(ProviderError::Request {
                        status,
                        message: describe_status(status).to_string(),
                    });
                }
                Err(error) => {
                    if attempt <= self.max_retries {
                        std::thread::sleep(Duration::from_millis(500 * u64::from(attempt)));
                        continue;
                    }
                    return Err(ProviderError::Transport(error.to_string()));
                }
            }
        }
    }
}

fn describe_status(status: u16) -> &'static str {
    match status {
        400 => "the request was malformed or used an unsupported parameter",
        401 => "the API key is missing or invalid",
        403 => "the API key lacks permission for this model",
        404 => "no such model or endpoint",
        413 => "the request is too large",
        529 => "the service is temporarily overloaded",
        _ => "the provider rejected the request",
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
}
