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
//!
//! Streaming adds a transport, not a second understanding of the response: the
//! chunks are reassembled into the shape a non-streamed call returns and handed
//! to the same `parse_response`. Both are private: the reassembly is an
//! implementation detail of this module, and nothing outside it may depend on
//! a streamed response being built differently from a whole-body one.

use std::time::Duration;

use serde_json::{json, Map, Value};

use crate::catalogue::ModelConfig;
use crate::http::{self, DEFAULT_MAX_RETRIES, DEFAULT_TIMEOUT};
use crate::provider::{
    CompletionRequest, CompletionResponse, DeltaSink, ModelProvider, ModelSelection, ProviderError,
    Usage,
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
    /// What each model provides, so a capability requirement can be matched.
    ///
    /// Held by the provider rather than passed per call because it is
    /// deployment configuration: it does not change between two calls of one
    /// run, and threading it through every request would put it in the cassette
    /// digest, where a manifest edit would invalidate a recording that has
    /// nothing to do with it.
    catalogue: ModelConfig,
    /// The name this deployment gave this provider, when it declared one.
    ///
    /// A built-in provider is its protocol: `openai/…` reaches OpenAI. A
    /// provider out of `[[model.provider]]` is whatever the operator called it,
    /// and `model exact "local/…"` already resolves against that name. Matching
    /// `model requires { … }` against the protocol instead would mean one
    /// provider answering to two names in one manifest, with only the pinned
    /// half working.
    vendor: Option<String>,
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
            catalogue: ModelConfig::default(),
            vendor: None,
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

    /// The models this deployment knows about.
    ///
    /// Without one, only the built-in entries apply — which is what a
    /// test or an embedder that never reads a manifest gets, and it is
    /// enough for a pinned model.
    pub fn with_catalogue(mut self, catalogue: ModelConfig) -> Self {
        self.catalogue = catalogue;
        self
    }

    /// The vendor half of a model reference, for a declared provider.
    ///
    /// `None` leaves it as the protocol's own name, which is correct for the
    /// built-in provider and for a test.
    pub fn with_vendor(mut self, vendor: Option<String>) -> Self {
        self.vendor = vendor;
        self
    }

    /// The name a model reference must carry to reach this provider.
    fn vendor(&self) -> &str {
        self.vendor.as_deref().unwrap_or(PROVIDER)
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
            ModelSelection::Capabilities {
                capabilities,
                min_context_tokens,
            } => self
                .catalogue
                .resolve_capabilities(self.vendor(), capabilities, *min_context_tokens)
                .map_err(ProviderError::Configuration),
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

    /// The same body, with the two keys a streamed call needs.
    fn build_streaming_body(&self, request: &CompletionRequest) -> Result<Value, ProviderError> {
        let mut body = self.build_body(request)?;
        let object = body
            .as_object_mut()
            .expect("build_body always produces an object");
        object.insert("stream".into(), json!(true));
        // Without `include_usage` most servers report no usage at all on a
        // streamed call, and a run that cannot see its token usage cannot
        // enforce its budget — so the budget would silently stop being a bound.
        object.insert("stream_options".into(), json!({ "include_usage": true }));
        Ok(body)
    }

    /// Absent for a server that wants no authentication.
    fn authorization(&self) -> Option<String> {
        self.api_key.as_ref().map(|key| format!("Bearer {key}"))
    }
}

/// Chunks reassembled into the shape a non-streamed response arrives in.
///
/// This is the load-bearing choice in the streaming path: **one parser, two
/// transports.** Nothing here decides what an answer means. It collects the
/// pieces, rebuilds the ordinary Chat Completions payload, and lets the same
/// [`parse_response`] read it — which is what guarantees that a streamed call
/// and a non-streamed call over the same content produce identical values *and*
/// identical errors: truncation, content filter, refusal, invalid JSON, an
/// empty answer. A second parser here would be a second set of rules, and the
/// two would drift apart on exactly the cases that matter least often and hurt
/// most.
///
/// Kept separate from the socket so the reassembly is testable with a list of
/// chunk values.
#[derive(Default)]
struct StreamAccumulator {
    /// The first one named; a stream does not change model mid-answer.
    model: Option<String>,
    content: String,
    refusal: String,
    /// The last non-null one, which is the one that ended the completion.
    finish_reason: Option<String>,
    /// The last non-null one: it arrives on the final chunk.
    usage: Option<Value>,
    error: Option<Value>,
}

impl StreamAccumulator {
    fn chunk(&mut self, data: &Value, on_delta: DeltaSink<'_>) {
        // Once a failure has been reported there is nothing left worth
        // accumulating: whatever follows describes a call that already failed.
        if self.error.is_some() {
            return;
        }
        // Some OpenAI-compatible gateways report a failure mid-stream this way
        // rather than with a status code.
        if let Some(error) = data.get("error") {
            self.error = Some(error.clone());
            return;
        }

        if self.model.is_none() {
            if let Some(model) = data.get("model").and_then(Value::as_str) {
                self.model = Some(model.to_string());
            }
        }
        // Read before the choices, because the chunk carrying usage carries an
        // empty `choices` array.
        if let Some(usage) = data.get("usage").filter(|usage| !usage.is_null()) {
            self.usage = Some(usage.clone());
        }

        let Some(choice) = data
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
        else {
            return;
        };

        if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
            self.finish_reason = Some(reason.to_string());
        }

        let delta = choice.get("delta");
        if let Some(text) = delta
            .and_then(|delta| delta.get("content"))
            .and_then(Value::as_str)
        {
            // The opening chunk of an OpenAI stream carries `content: ""`
            // beside the role. Handing that on would be noise, not text
            // arriving.
            if !text.is_empty() {
                self.content.push_str(text);
                on_delta(text);
            }
        }
        // A refusal is accumulated but never shown: a watcher reading it live
        // would read it as the answer arriving, and a refusal is not an answer.
        // It reaches the caller through `parse_response`, as a refusal.
        if let Some(text) = delta
            .and_then(|delta| delta.get("refusal"))
            .and_then(Value::as_str)
        {
            self.refusal.push_str(text);
        }

        // Every other field is ignored on purpose. A service that adds one must
        // not break a run.
    }

    /// Whether the stream stopped before the completion did.
    ///
    /// No `finish_reason` ever arrived, so the connection was cut part-way
    /// through the answer. That is a transport failure, not a successful empty
    /// answer, and the difference decides whether a run continues on nothing.
    fn ended_early(&self) -> bool {
        self.error.is_none() && self.finish_reason.is_none()
    }

    fn into_payload(self) -> Value {
        // Carried in the payload rather than raised here, so a mid-stream error
        // and a gateway that answers 200 with an error body produce the very
        // same `ProviderError`.
        if let Some(error) = self.error {
            return json!({ "error": error });
        }

        let refusal = if self.refusal.is_empty() {
            Value::Null
        } else {
            Value::String(self.refusal)
        };
        json!({
            "model": self.model,
            "choices": [{
                "message": { "content": self.content, "refusal": refusal },
                "finish_reason": self.finish_reason,
            }],
            "usage": self.usage,
        })
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
        let authorization = self.authorization();
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

    fn streams(&self) -> bool {
        true
    }

    fn complete_streaming(
        &mut self,
        request: &CompletionRequest,
        on_delta: DeltaSink<'_>,
    ) -> Result<CompletionResponse, ProviderError> {
        let body = self.build_streaming_body(request)?;
        let authorization = self.authorization();
        let headers: Vec<(&str, &str)> = match &authorization {
            Some(value) => vec![("authorization", value.as_str())],
            None => Vec::new(),
        };

        let mut accumulator = StreamAccumulator::default();
        http::post_sse(
            &self.base_url,
            &headers,
            &body,
            self.timeout,
            self.max_retries,
            // The event name is empty for every chunk: an OpenAI-compatible
            // stream carries no `event:` line, only `data:`.
            &mut |_name, data| accumulator.chunk(data, &mut *on_delta),
        )?;

        if accumulator.ended_early() {
            return Err(ProviderError::Transport(
                "the stream ended before the completion did: no finish reason arrived, so the \
                 connection was cut part-way through the answer"
                    .to_string(),
            ));
        }
        // The same parser the non-streaming path uses, so the two agree on
        // every value and every error.
        parse_response(request, &accumulator.into_payload())
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
    fn a_declared_provider_matches_a_requirement_against_its_own_name() {
        // The bug this pins: `[[model.provider]] name = "local"` used to resolve
        // `model requires { … }` against `openai`, the protocol's own name, so
        // an operator's `local/…` entry was invisible and the refusal named a
        // vendor they had not written. `model exact "local/…"` already uses the
        // declared name, and one provider must not answer to two.
        let catalogue = ModelConfig {
            catalogue: vec![crate::catalogue::ModelEntry {
                model: "local/qwen3".to_string(),
                context: Some(32_768),
                capabilities: vec!["structured_output".to_string()],
            }],
            ..ModelConfig::default()
        };
        let selection = ModelSelection::Capabilities {
            capabilities: vec!["structured_output".to_string()],
            min_context_tokens: Some(32_768),
        };

        let body = provider()
            .with_catalogue(catalogue.clone())
            .with_vendor(Some("local".to_string()))
            .build_body(&request("markdown", selection.clone()))
            .expect("the declared vendor's entry answers");
        assert_eq!(body["model"], json!("qwen3"));

        // And without the declared name the same catalogue does not apply, so
        // the two halves are genuinely coupled rather than both being loose.
        let error = provider()
            .with_catalogue(catalogue)
            .build_body(&request("markdown", selection))
            .expect_err("`openai` has no such entry");
        assert!(error.to_string().contains("openai"), "{error}");
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

    #[test]
    fn a_non_streaming_request_never_asks_the_server_to_stream() {
        let body = provider()
            .build_body(&request("markdown", pinned()))
            .unwrap();
        assert!(body.get("stream").is_none(), "{body}");
        assert!(body.get("stream_options").is_none(), "{body}");
    }

    #[test]
    fn a_streamed_request_asks_for_usage_because_the_budget_depends_on_it() {
        let body = provider()
            .build_streaming_body(&request("markdown", pinned()))
            .unwrap();
        assert_eq!(body["stream"], json!(true));
        assert_eq!(body["stream_options"]["include_usage"], json!(true));
        // The rest of the body is the non-streamed one, cap included.
        assert_eq!(body["max_completion_tokens"], json!(4096));
        assert!(body.get("max_tokens").is_none(), "{body}");
    }

    /// Drive the accumulator the way a stream would, and report what a watcher
    /// would have seen while it ran.
    fn accumulate(chunks: Vec<Value>) -> (StreamAccumulator, Vec<String>) {
        let mut accumulator = StreamAccumulator::default();
        let mut shown = Vec::new();
        for chunk in &chunks {
            accumulator.chunk(chunk, &mut |text| shown.push(text.to_string()));
        }
        (accumulator, shown)
    }

    fn content_chunk(text: &str) -> Value {
        json!({ "model": "gpt-test", "choices": [{ "delta": { "content": text } }] })
    }

    fn stop_chunk(reason: &str) -> Value {
        json!({ "choices": [{ "delta": {}, "finish_reason": reason }] })
    }

    #[test]
    fn content_chunks_accumulate_in_the_order_they_arrive() {
        let (accumulator, _) = accumulate(vec![
            content_chunk("Once "),
            content_chunk("upon "),
            content_chunk("a time"),
            stop_chunk("stop"),
        ]);
        let response =
            parse_response(&request("markdown", pinned()), &accumulator.into_payload()).unwrap();
        assert_eq!(response.value, json!("Once upon a time"));
        assert_eq!(response.model, "gpt-test");
    }

    #[test]
    fn every_content_delta_is_handed_over_as_it_arrives() {
        let (_, shown) = accumulate(vec![
            // The opening chunk of a real stream: a role and no text yet.
            json!({ "choices": [{ "delta": { "role": "assistant", "content": "" } }] }),
            content_chunk("one "),
            content_chunk("two"),
            stop_chunk("stop"),
        ]);
        assert_eq!(shown, vec!["one ".to_string(), "two".to_string()]);
    }

    #[test]
    fn a_watcher_sees_exactly_the_text_that_becomes_the_answer() {
        // The property the streaming path exists to keep: nothing is shown that
        // does not end up in the answer, and nothing in the answer went unshown.
        let (accumulator, shown) = accumulate(vec![
            content_chunk("the "),
            json!({ "choices": [{ "delta": { "refusal": "I will not" } }] }),
            content_chunk("answer"),
            stop_chunk("stop"),
        ]);
        assert_eq!(shown.concat(), accumulator.content);
        assert_eq!(shown.concat(), "the answer");
    }

    #[test]
    fn refusal_deltas_are_collected_but_never_shown_to_a_watcher() {
        // A refusal is not an answer, so showing it live would be a lie about
        // what the run is producing — but it is still what the caller is told.
        let (accumulator, shown) = accumulate(vec![
            json!({ "choices": [{ "delta": { "refusal": "I cannot " } }] }),
            json!({ "choices": [{ "delta": { "refusal": "help with that." } }] }),
            stop_chunk("stop"),
        ]);
        assert!(shown.is_empty(), "{shown:?}");

        let error = parse_response(&request("markdown", pinned()), &accumulator.into_payload())
            .unwrap_err();
        assert!(matches!(error, ProviderError::Refused { .. }), "{error}");
        assert!(error.to_string().contains("cannot help"), "{error}");
    }

    #[test]
    fn a_streamed_truncation_is_reported_rather_than_returned_as_a_result() {
        let (accumulator, _) = accumulate(vec![content_chunk("half an ans"), stop_chunk("length")]);
        let error = parse_response(&request("markdown", pinned()), &accumulator.into_payload())
            .unwrap_err();
        assert!(
            matches!(error, ProviderError::Truncated { limit: 4096 }),
            "{error}"
        );
    }

    #[test]
    fn a_streamed_content_filter_stop_is_a_refusal() {
        let (accumulator, _) =
            accumulate(vec![content_chunk("half"), stop_chunk("content_filter")]);
        let error = parse_response(&request("markdown", pinned()), &accumulator.into_payload())
            .unwrap_err();
        assert!(matches!(error, ProviderError::Refused { .. }), "{error}");
    }

    #[test]
    fn usage_from_the_final_chunk_reaches_the_response() {
        let (accumulator, _) = accumulate(vec![
            content_chunk("done"),
            // Every chunk before the last carries a null here.
            json!({ "choices": [{ "delta": {} }], "usage": Value::Null }),
            stop_chunk("stop"),
            // The final chunk: usage, and no choice to go with it.
            json!({ "choices": [],
                    "usage": { "prompt_tokens": 120, "completion_tokens": 40,
                               "prompt_tokens_details": { "cached_tokens": 100 } } }),
        ]);
        let response =
            parse_response(&request("markdown", pinned()), &accumulator.into_payload()).unwrap();
        assert_eq!(response.usage.input_tokens, 120);
        assert_eq!(response.usage.output_tokens, 40);
        assert_eq!(response.usage.cache_read_tokens, 100);
    }

    #[test]
    fn an_unrecognised_chunk_field_is_ignored_rather_than_failing_the_run() {
        // A service adding a field must not break an artifact that predates it.
        let (accumulator, _) = accumulate(vec![
            json!({ "model": "gpt-test", "system_fingerprint": "fp_1",
                    "choices": [{ "delta": { "content": "fine", "reasoning": "hidden" },
                                  "logprobs": null }],
                    "obviously_new": { "nested": true } }),
            stop_chunk("stop"),
        ]);
        let response =
            parse_response(&request("markdown", pinned()), &accumulator.into_payload()).unwrap();
        assert_eq!(response.value, json!("fine"));
    }

    #[test]
    fn an_error_reported_mid_stream_stops_the_answer_rather_than_completing_it() {
        let (accumulator, shown) = accumulate(vec![
            content_chunk("start"),
            json!({ "error": { "message": "upstream capacity exhausted" } }),
            content_chunk("never shown"),
            stop_chunk("stop"),
        ]);
        assert_eq!(shown, vec!["start".to_string()]);
        assert!(!accumulator.ended_early(), "the failure is the outcome");

        let error = parse_response(&request("markdown", pinned()), &accumulator.into_payload())
            .unwrap_err();
        assert!(error.to_string().contains("capacity exhausted"), "{error}");
    }

    #[test]
    fn a_stream_that_ends_without_a_finish_reason_is_not_a_finished_answer() {
        // The connection was cut mid-answer. Treating that as a successful
        // short answer would hand the run half a document.
        let (accumulator, _) = accumulate(vec![content_chunk("half an ans")]);
        assert!(accumulator.ended_early());

        let (accumulator, _) = accumulate(vec![content_chunk("all of it"), stop_chunk("stop")]);
        assert!(!accumulator.ended_early());
    }
}
