//! OpenAI Chat Completions provider.
//!
//! Feature-gated behind `openai`. Raw HTTP over the **Chat Completions** shape
//! rather than a newer, richer one, and that is the load-bearing choice: it is
//! the shape a dozen other services speak. Point [`OpenAiProvider::with_base_url`]
//! at Azure, a local vLLM or llama.cpp server, or any OpenAI-compatible gateway
//! and the same artifact runs against it.
//!
//! Three details are easy to get wrong:
//!
//! * **There is no default model.** Model names change often enough that
//!   guessing produces a 404 that reads like a bug in Ingot. The artifact pins
//!   one, or `--model` does, or the run stops and says so.
//! * **Structured output needs a strict schema.** `ask<T>` becomes
//!   `response_format: json_schema` with `strict: true`, which requires every
//!   property to be required and `additionalProperties: false` — which is what
//!   [`crate::schema`] already produces.
//! * **Sampling parameters are not sent**, for the same reason they are not
//!   sent to Anthropic: current reasoning models reject them, and an artifact
//!   that behaves differently per provider defeats the point of the artifact.

use std::time::Duration;

use serde_json::{json, Map, Value};

use crate::http::{self, DEFAULT_MAX_RETRIES, DEFAULT_TIMEOUT};
use crate::provider::{
    CompletionRequest, CompletionResponse, ModelProvider, ModelSelection, ProviderError, Usage,
};
use crate::schema::ResponseShape;

const API_URL: &str = "https://api.openai.com/v1/chat/completions";

/// The prefix an artifact uses to pin this provider: `openai/gpt-…`.
pub const PROVIDER: &str = "openai";

/// What `--effort` becomes. OpenAI takes three levels; the two above them in
/// Ingot's vocabulary saturate rather than being dropped silently.
fn reasoning_effort(effort: &str) -> &str {
    match effort {
        "low" => "low",
        "medium" => "medium",
        _ => "high",
    }
}

pub struct OpenAiProvider {
    /// Absent for a server that wants no authentication, which is the usual
    /// arrangement for one running on the same machine.
    api_key: Option<String>,
    /// Overrides whatever the artifact asks for. Set from `--model`.
    model_override: Option<String>,
    effort: Option<String>,
    max_retries: u32,
    timeout: Duration,
    base_url: String,
}

impl OpenAiProvider {
    /// Read the key from `OPENAI_API_KEY`.
    ///
    /// `INGOT_OPENAI_BASE_URL` overrides the endpoint — that is how a gateway,
    /// a self-hosted server, or the stub in the wire tests is reached.
    pub fn from_env() -> Result<OpenAiProvider, ProviderError> {
        let mut provider = OpenAiProvider::with_key(http::key_from_env("OPENAI_API_KEY")?);
        if let Some(url) = http::base_url_from_env("INGOT_OPENAI_BASE_URL") {
            provider = provider.with_base_url(url);
        }
        Ok(provider)
    }

    pub fn with_key(api_key: impl Into<String>) -> OpenAiProvider {
        OpenAiProvider {
            api_key: Some(api_key.into()),
            ..OpenAiProvider::without_key()
        }
    }

    /// For a server that authenticates no requests — a local one, usually.
    pub fn without_key() -> OpenAiProvider {
        OpenAiProvider {
            api_key: None,
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

    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }

    /// Which model to ask, or why we will not guess.
    fn resolve_model(&self, selection: &ModelSelection) -> Result<String, ProviderError> {
        if let Some(model) = &self.model_override {
            return Ok(model.clone());
        }
        match selection {
            ModelSelection::Exact(reference) => Ok(reference
                .split_once('/')
                .map(|(_, model)| model.to_string())
                .unwrap_or_else(|| reference.clone())),
            ModelSelection::Default => Err(ProviderError::Configuration(
                "this artifact names no model, and there is no sensible default for OpenAI: \
                 the catalogue changes often enough that guessing produces a `404 no such model` \
                 that reads like a bug here.\n  \
                 pin one in the source with `model exact \"openai/<model>\"`, or pass --model"
                    .to_string(),
            )),
            ModelSelection::Capabilities { capabilities, .. } => {
                Err(ProviderError::Configuration(format!(
                    "this artifact requires capabilities ({}) rather than naming a model, and \
                     this provider has no catalogue to match them against.\n  \
                     pin one with `model exact \"openai/<model>\"`, or pass --model",
                    if capabilities.is_empty() {
                        "none listed".to_string()
                    } else {
                        capabilities.join(", ")
                    }
                )))
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

        let mut messages = Vec::new();
        if let Some(system) = &request.system {
            messages.push(json!({ "role": "system", "content": system }));
        }
        messages.push(json!({ "role": "user", "content": user_content }));

        let mut body = Map::new();
        body.insert("model".into(), json!(model));
        body.insert("messages".into(), Value::Array(messages));
        // `max_completion_tokens`, not `max_tokens`: current models reject the
        // older name outright. An OpenAI-compatible server that only knows the
        // old one will not cap the response, and the budget check catches that.
        body.insert("max_completion_tokens".into(), json!(request.max_tokens));

        if let Some(effort) = &self.effort {
            body.insert("reasoning_effort".into(), json!(reasoning_effort(effort)));
        }

        if let ResponseShape::Schema { schema, .. } = &request.shape {
            body.insert(
                "response_format".into(),
                json!({
                    "type": "json_schema",
                    "json_schema": {
                        // Named after the Ingot type, so a provider-side log
                        // says which declaration constrained the call.
                        "name": schema_name(&request.response_type),
                        "strict": true,
                        "schema": schema,
                    }
                }),
            );
        }

        Ok(Value::Object(body))
    }
}

/// A schema name OpenAI accepts: letters, digits, underscores and dashes.
fn schema_name(response_type: &str) -> String {
    let cleaned: String = response_type
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect();
    let trimmed = cleaned.trim_matches('_');
    if trimmed.is_empty() {
        "ingot_response".to_string()
    } else {
        format!("ingot_{trimmed}")
    }
}

impl ModelProvider for OpenAiProvider {
    fn name(&self) -> &str {
        PROVIDER
    }

    fn complete(
        &mut self,
        request: &CompletionRequest,
    ) -> Result<CompletionResponse, ProviderError> {
        let body = self.build_body(request)?;
        let authorization = self.api_key.as_ref().map(|key| format!("Bearer {key}"));
        let headers: Vec<(&str, &str)> = match &authorization {
            Some(value) => vec![("authorization", value.as_str())],
            None => Vec::new(),
        };
        let payload = http::post_json(
            &self.base_url,
            &headers,
            &body,
            self.timeout,
            self.max_retries,
        )?;
        parse_response(request, &payload)
    }
}

/// Turn a Chat Completions response into a typed value.
pub(crate) fn parse_response(
    request: &CompletionRequest,
    payload: &Value,
) -> Result<CompletionResponse, ProviderError> {
    // A service-level error arrives with HTTP 200 on some compatible gateways.
    if let Some(error) = payload.get("error") {
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("the provider reported an error without a message");
        return Err(ProviderError::InvalidResponse(message.to_string()));
    }

    let choice = payload
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .ok_or_else(|| {
            ProviderError::InvalidResponse("the response carries no choices".to_string())
        })?;
    let message = choice.get("message");

    // Before the content, for the same reason as everywhere else: on a refusal
    // the content is null, and reading it first turns a refusal into a
    // confusing parse error.
    if let Some(refusal) = message
        .and_then(|message| message.get("refusal"))
        .and_then(Value::as_str)
    {
        if !refusal.trim().is_empty() {
            return Err(ProviderError::Refused {
                category: None,
                explanation: Some(refusal.to_string()),
            });
        }
    }

    let finish = choice
        .get("finish_reason")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if finish == "length" {
        return Err(ProviderError::Truncated {
            limit: request.max_tokens,
        });
    }
    if finish == "content_filter" {
        return Err(ProviderError::Refused {
            category: Some("content_filter".to_string()),
            explanation: None,
        });
    }

    let text = message
        .and_then(|message| message.get("content"))
        .and_then(Value::as_str)
        .unwrap_or_default();

    if text.trim().is_empty() {
        return Err(ProviderError::InvalidResponse(format!(
            "the provider returned no text (finish reason: `{finish}`)"
        )));
    }

    let value = match &request.shape {
        ResponseShape::Prose => Value::String(text.to_string()),
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
                .and_then(|usage| usage.get("prompt_tokens"))
                .and_then(Value::as_u64)
                .unwrap_or(0),
            output_tokens: usage
                .and_then(|usage| usage.get("completion_tokens"))
                .and_then(Value::as_u64)
                .unwrap_or(0),
            cache_read_tokens: usage
                .and_then(|usage| usage.get("prompt_tokens_details"))
                .and_then(|details| details.get("cached_tokens"))
                .and_then(Value::as_u64)
                .unwrap_or(0),
        },
        model: payload
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema;
    use std::collections::BTreeMap;

    fn request(response_type: &str, model: ModelSelection) -> CompletionRequest {
        let types = BTreeMap::new();
        CompletionRequest {
            node: "n0".to_string(),
            model,
            system: Some("You are precise.".to_string()),
            prompt: "Summarise it".to_string(),
            context: vec![("hits".to_string(), json!([{"title": "t"}]))],
            response_type: response_type.to_string(),
            shape: schema::response_shape(response_type, &types).expect("a requestable type"),
            max_tokens: 4096,
        }
    }

    fn provider() -> OpenAiProvider {
        OpenAiProvider::with_key("stub-key")
    }

    fn pinned() -> ModelSelection {
        ModelSelection::Exact("openai/gpt-test".to_string())
    }

    #[test]
    fn sampling_parameters_are_never_sent() {
        let body = provider()
            .build_body(&request("markdown", pinned()))
            .unwrap();
        for absent in ["temperature", "top_p", "top_k", "frequency_penalty"] {
            assert!(body.get(absent).is_none(), "{absent} must not be sent");
        }
    }

    #[test]
    fn the_output_cap_uses_the_name_current_models_accept() {
        let body = provider()
            .build_body(&request("markdown", pinned()))
            .unwrap();
        assert_eq!(body["max_completion_tokens"], json!(4096));
        assert!(body.get("max_tokens").is_none(), "{body}");
    }

    #[test]
    fn prose_requests_carry_no_schema() {
        let body = provider()
            .build_body(&request("markdown", pinned()))
            .unwrap();
        assert!(body.get("response_format").is_none(), "{body}");
    }

    #[test]
    fn structured_requests_carry_a_strict_schema_named_after_the_type() {
        let body = provider()
            .build_body(&request("string[]", pinned()))
            .unwrap();
        let format = &body["response_format"];
        assert_eq!(format["type"], "json_schema");
        assert_eq!(format["json_schema"]["strict"], json!(true));
        assert_eq!(format["json_schema"]["name"], json!("ingot_string"));
        assert_eq!(
            format["json_schema"]["schema"]["properties"]["value"]["type"],
            "array"
        );
    }

    #[test]
    fn a_system_prompt_becomes_the_first_message() {
        let body = provider()
            .build_body(&request("markdown", pinned()))
            .unwrap();
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[1]["role"], "user");
        assert!(
            messages[1]["content"].as_str().unwrap().contains("<hits>"),
            "context is wrapped in named tags: {messages:?}"
        );
    }

    #[test]
    fn an_exact_reference_drops_the_provider_prefix() {
        let body = provider()
            .build_body(&request("markdown", pinned()))
            .unwrap();
        assert_eq!(body["model"], json!("gpt-test"));
    }

    #[test]
    fn no_model_is_refused_rather_than_guessed() {
        // A wrong guess produces `404 no such model`, which reads like a bug in
        // Ingot rather than a missing line in the artifact.
        let error = provider()
            .build_body(&request("markdown", ModelSelection::Default))
            .unwrap_err();
        let text = error.to_string();
        assert!(text.contains("model exact"), "{text}");
        assert!(text.contains("--model"), "{text}");
    }

    #[test]
    fn capability_requirements_are_refused_without_a_catalogue_to_match() {
        let selection = ModelSelection::Capabilities {
            capabilities: vec!["structured_output".to_string()],
            min_context_tokens: Some(128_000),
        };
        let error = provider()
            .build_body(&request("markdown", selection))
            .unwrap_err();
        assert!(error.to_string().contains("structured_output"), "{error}");
    }

    #[test]
    fn effort_saturates_rather_than_being_dropped() {
        assert_eq!(reasoning_effort("low"), "low");
        assert_eq!(reasoning_effort("medium"), "medium");
        assert_eq!(reasoning_effort("high"), "high");
        assert_eq!(reasoning_effort("xhigh"), "high");
        assert_eq!(reasoning_effort("max"), "high");

        let body = provider()
            .with_effort(Some("max".to_string()))
            .build_body(&request("markdown", pinned()))
            .unwrap();
        assert_eq!(body["reasoning_effort"], json!("high"));
    }

    #[test]
    fn a_refusal_is_reported_before_content_is_read() {
        let payload = json!({
            "choices": [{ "message": { "content": null, "refusal": "I cannot help with that." },
                          "finish_reason": "stop" }]
        });
        let error = parse_response(&request("markdown", pinned()), &payload).unwrap_err();
        assert!(matches!(error, ProviderError::Refused { .. }), "{error}");
        assert!(error.to_string().contains("cannot help"), "{error}");
    }

    #[test]
    fn truncation_is_reported_rather_than_returned_as_a_result() {
        let payload = json!({
            "choices": [{ "message": { "content": "half an ans" }, "finish_reason": "length" }]
        });
        let error = parse_response(&request("markdown", pinned()), &payload).unwrap_err();
        assert!(
            matches!(error, ProviderError::Truncated { limit: 4096 }),
            "{error}"
        );
    }

    #[test]
    fn a_content_filter_stop_is_a_refusal() {
        let payload = json!({
            "choices": [{ "message": { "content": "" }, "finish_reason": "content_filter" }]
        });
        let error = parse_response(&request("markdown", pinned()), &payload).unwrap_err();
        assert!(matches!(error, ProviderError::Refused { .. }), "{error}");
    }

    #[test]
    fn prose_responses_come_back_as_text_with_usage() {
        let payload = json!({
            "model": "gpt-test",
            "choices": [{ "message": { "content": "# Title" }, "finish_reason": "stop" }],
            "usage": { "prompt_tokens": 120, "completion_tokens": 40,
                       "prompt_tokens_details": { "cached_tokens": 100 } }
        });
        let response = parse_response(&request("markdown", pinned()), &payload).unwrap();
        assert_eq!(response.value, json!("# Title"));
        assert_eq!(response.usage.input_tokens, 120);
        assert_eq!(response.usage.output_tokens, 40);
        assert_eq!(response.usage.cache_read_tokens, 100);
        assert_eq!(response.model, "gpt-test");
    }

    #[test]
    fn wrapped_structured_responses_are_unwrapped() {
        let payload = json!({
            "choices": [{ "message": { "content": "{\"value\":[\"a\",\"b\"]}" },
                          "finish_reason": "stop" }]
        });
        let response = parse_response(&request("string[]", pinned()), &payload).unwrap();
        assert_eq!(response.value, json!(["a", "b"]));
    }

    #[test]
    fn an_error_body_returned_with_a_success_status_is_still_an_error() {
        // Several OpenAI-compatible gateways do this.
        let payload = json!({ "error": { "message": "model not found" } });
        let error = parse_response(&request("markdown", pinned()), &payload).unwrap_err();
        assert!(error.to_string().contains("model not found"), "{error}");
    }

    #[test]
    fn an_empty_response_is_an_error_rather_than_an_empty_artifact() {
        let payload = json!({
            "choices": [{ "message": { "content": "   " }, "finish_reason": "stop" }]
        });
        assert!(parse_response(&request("markdown", pinned()), &payload).is_err());
    }

    #[test]
    fn a_schema_name_is_always_acceptable_to_the_service() {
        assert_eq!(schema_name("search_result[]"), "ingot_search_result");
        assert_eq!(schema_name("string"), "ingot_string");
        assert_eq!(schema_name("[]"), "ingot_response");
    }
}
