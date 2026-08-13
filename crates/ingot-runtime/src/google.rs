//! Google Gemini provider.
//!
//! Feature-gated behind `google`. The third wire protocol Ingot speaks, and it
//! is here because it is the one a service cannot reach by pretending to be
//! something else: Gemini is neither Chat Completions nor Messages.
//!
//! Four details differ from the other two, and each is easy to get wrong:
//!
//! * **The model is in the URL, not the body.** The endpoint is
//!   `{base}/models/{model}:generateContent`, so `base-url` for this protocol
//!   is the API base rather than a complete endpoint. A base that already names
//!   a method is refused rather than concatenated into a 404.
//! * **The key goes in a header.** The API also accepts `?key=`, and this
//!   provider deliberately does not use it: a URL travels through proxy logs,
//!   error messages and crash reports in a way a header does not.
//! * **Structured output takes an OpenAPI subset, not JSON Schema.** The schema
//!   [`crate::schema`] produces is translated, and a construct with no faithful
//!   translation is refused rather than quietly dropped — a silently weakened
//!   schema is a constraint the caller believes in and does not have.
//! * **There is no default model.** Gemini model names are versioned and
//!   short-lived, and guessing one produces a 404 that reads like a bug in
//!   Ingot. The artifact pins one, or `--model` does, or the run stops.
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

/// The Generative Language API base. Not an endpoint: the model and the method
/// are path segments, appended per call.
const API_BASE: &str = "https://generativelanguage.googleapis.com/v1beta";

/// The prefix an artifact uses to pin this provider: `google/gemini-…`.
pub const PROVIDER: &str = "google";

pub struct GoogleProvider {
    api_key: String,
    /// Overrides whatever the artifact asks for. Set from `--model`.
    model_override: Option<String>,
    /// Held so the refusal in [`GoogleProvider::check_effort`] can name it.
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
}

impl GoogleProvider {
    /// Read the key from `GEMINI_API_KEY`, falling back to `GOOGLE_API_KEY`.
    ///
    /// Both names are in wide use and neither is wrong, so trying one and
    /// failing would be a configuration error the operator did not make.
    ///
    /// `INGOT_GOOGLE_BASE_URL` overrides the API base, for a gateway, a proxy,
    /// or the stub server the wire tests run against.
    pub fn from_env() -> Result<GoogleProvider, ProviderError> {
        let key = match http::key_from_env("GEMINI_API_KEY") {
            Ok(key) => key,
            Err(first) => match http::key_from_env("GOOGLE_API_KEY") {
                Ok(key) => key,
                // The first name is the one the message advises exporting,
                // because naming both in a failure reads as indecision.
                Err(_) => return Err(first),
            },
        };
        let mut provider = GoogleProvider::with_key(key);
        if let Some(url) = http::base_url_from_env("INGOT_GOOGLE_BASE_URL") {
            provider = provider.with_base_url(url);
        }
        Ok(provider)
    }

    pub fn with_key(api_key: impl Into<String>) -> GoogleProvider {
        GoogleProvider {
            api_key: api_key.into(),
            model_override: None,
            catalogue: ModelConfig::default(),
            effort: None,
            max_retries: DEFAULT_MAX_RETRIES,
            timeout: DEFAULT_TIMEOUT,
            base_url: API_BASE.to_string(),
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

    pub fn with_effort(mut self, effort: Option<String>) -> Self {
        self.effort = effort;
        self
    }

    /// Point at a different API base. Used by tests against a local stub.
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }

    /// Which model to ask.
    ///
    /// No fallback. A wrong model name here is a 404 from the service, and a
    /// 404 that Ingot caused by guessing is indistinguishable from a 404 the
    /// artifact caused by naming a model that was retired.
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
                "this provider has no default model, because Gemini model names are versioned \
                 and a guess becomes a 404 that looks like a bug in Ingot.\n  \
                 pin one in the source with `model exact \"google/<model>\"`, or pass --model"
                    .to_string(),
            )),
            ModelSelection::Capabilities {
                capabilities,
                min_context_tokens,
            } => self
                .catalogue
                .resolve_capabilities(PROVIDER, capabilities, *min_context_tokens)
                .map_err(ProviderError::Configuration),
        }
    }

    /// Refuse `--effort` rather than pick a control that may not exist.
    ///
    /// Gemini's thinking control is model-dependent — a token `thinkingBudget`
    /// on one generation, a named level on the next — and the two are not
    /// interchangeable. Sending the wrong one is a rejected request; sending
    /// neither and saying nothing would let an operator believe a flag took
    /// effect when it did not. Refusing is the only option that is true.
    ///
    /// Checked per request rather than at construction, so exporting a Gemini
    /// key does not break `--effort` for an artifact pinned to another vendor.
    fn check_effort(&self) -> Result<(), ProviderError> {
        let Some(effort) = &self.effort else {
            return Ok(());
        };
        Err(ProviderError::Configuration(format!(
            "`--effort {effort}` cannot be honoured by the Gemini protocol: its thinking control \
             differs per model generation, and Ingot will not guess which one a given model \
             takes.\n  \
             drop --effort for this run, or route the call to a provider that has one"
        )))
    }

    /// `{base}/models/{model}:{method}`, with the base checked first.
    fn endpoint(&self, model: &str, method: &str) -> Result<String, ProviderError> {
        let base = self.base_url.trim_end_matches('/');
        if base.contains(":generateContent") || base.contains(":streamGenerateContent") {
            return Err(ProviderError::Configuration(format!(
                "`base-url` for the google protocol is the API base, not a complete endpoint, \
                 because the model and the method are path segments.\n  \
                 got:  {base}\n  \
                 want: {API_BASE}"
            )));
        }
        Ok(format!("{base}/models/{model}:{method}"))
    }

    fn build_body(&self, request: &CompletionRequest) -> Result<Value, ProviderError> {
        self.check_effort()?;

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
        body.insert(
            "contents".into(),
            json!([{ "role": "user", "parts": [{ "text": user_content }] }]),
        );
        if let Some(system) = &request.system {
            body.insert(
                "systemInstruction".into(),
                json!({ "parts": [{ "text": system }] }),
            );
        }

        let mut generation = Map::new();
        generation.insert("maxOutputTokens".into(), json!(request.max_tokens));
        match &request.shape {
            ResponseShape::Prose => {}
            ResponseShape::FreeJson => {
                generation.insert("responseMimeType".into(), json!("application/json"));
            }
            ResponseShape::Schema { schema, .. } => {
                generation.insert("responseMimeType".into(), json!("application/json"));
                generation.insert("responseSchema".into(), response_schema(schema)?);
            }
        }
        body.insert("generationConfig".into(), Value::Object(generation));

        // Deliberately absent: temperature, topP, topK. Same reason as the
        // other two providers — an artifact that behaves differently per
        // provider is what portability is meant to prevent.
        Ok(Value::Object(body))
    }

    fn headers(&self) -> Vec<(&str, &str)> {
        vec![("x-goog-api-key", self.api_key.as_str())]
    }
}

/// Translate the JSON Schema [`crate::schema`] produces into Gemini's subset.
///
/// The subset is OpenAPI 3.0's `Schema`, which overlaps with JSON Schema but is
/// not it: types are named in upper case, and `additionalProperties` is not a
/// field it has. Anything with no faithful translation is refused, because a
/// schema quietly stripped of a constraint is worse than no schema — the caller
/// believes the model was constrained, and it was not.
///
/// Dropping `additionalProperties: false` is the one deliberate loosening, and
/// it costs nothing: [`crate::schema::validate`] checks that every declared
/// field is present and well typed and has never rejected an extra one, so the
/// value a run accepts is the same either way.
fn response_schema(schema: &Value) -> Result<Value, ProviderError> {
    let Some(object) = schema.as_object() else {
        return Err(unrepresentable("a schema that is not an object"));
    };

    let Some(ty) = object.get("type").and_then(Value::as_str) else {
        // Reached by a record with a `json` field: its schema is `{}`, which
        // permits anything and therefore has no OpenAPI type to name.
        return Err(unrepresentable(
            "a `json` value inside a typed response, which has no shape to constrain",
        ));
    };

    let mut out = Map::new();
    match ty {
        "object" => {
            out.insert("type".into(), json!("OBJECT"));
            let mut properties = Map::new();
            if let Some(declared) = object.get("properties").and_then(Value::as_object) {
                for (name, field) in declared {
                    properties.insert(name.clone(), response_schema(field)?);
                }
            }
            out.insert("properties".into(), Value::Object(properties));
            if let Some(required) = object.get("required").and_then(Value::as_array) {
                out.insert("required".into(), Value::Array(required.clone()));
                // The order the fields were declared in. Gemini emits
                // properties in this order, so a record comes back reading the
                // way it reads in the source rather than alphabetically.
                out.insert("propertyOrdering".into(), Value::Array(required.clone()));
            }
        }
        "array" => {
            out.insert("type".into(), json!("ARRAY"));
            let Some(items) = object.get("items") else {
                return Err(unrepresentable("an array with no element type"));
            };
            out.insert("items".into(), response_schema(items)?);
        }
        "string" => {
            out.insert("type".into(), json!("STRING"));
        }
        "integer" => {
            out.insert("type".into(), json!("INTEGER"));
        }
        "number" => {
            out.insert("type".into(), json!("NUMBER"));
        }
        "boolean" => {
            out.insert("type".into(), json!("BOOLEAN"));
        }
        other => return Err(unrepresentable(&format!("the schema type `{other}`"))),
    }
    Ok(Value::Object(out))
}

fn unrepresentable(what: &str) -> ProviderError {
    ProviderError::Configuration(format!(
        "the declared response type cannot be expressed as a Gemini response schema: {what}.\n  \
         ask for `json` and validate it yourself, or route this call to another provider"
    ))
}

impl ModelProvider for GoogleProvider {
    fn name(&self) -> &str {
        PROVIDER
    }

    fn complete(
        &mut self,
        request: &CompletionRequest,
    ) -> Result<CompletionResponse, ProviderError> {
        let model = self.resolve_model(&request.model)?;
        let body = self.build_body(request)?;
        let payload = http::post_json(
            &self.endpoint(&model, "generateContent")?,
            &self.headers(),
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
        let model = self.resolve_model(&request.model)?;
        let body = self.build_body(request)?;
        // `alt=sse` rather than the default, which is a JSON array streamed as
        // one document — parseable only once it is complete, which is not
        // streaming in any sense a watcher benefits from.
        let url = format!(
            "{}?alt=sse",
            self.endpoint(&model, "streamGenerateContent")?
        );

        // Gemini's chunks carry no event name, so the name is ignored rather
        // than switched on.
        let mut accumulator = StreamAccumulator::default();
        http::post_sse(
            &url,
            &self.headers(),
            &body,
            self.timeout,
            self.max_retries,
            &mut |_name, data| accumulator.chunk(data, &mut *on_delta),
        )?;

        parse_response(request, &accumulator.into_payload()?)
    }
}

/// Rebuilds a streamed answer into the shape a whole-body answer has.
///
/// One parser, two transports: everything that decides what a response means —
/// blocked prompts, safety stops, truncation, schema mismatches — is decided
/// once, in [`parse_response`], from a payload that looks the same whichever
/// way it arrived. A second parser here would be a second answer to "did this
/// response validate", and the two would drift on the inputs nobody tested.
#[derive(Default)]
struct StreamAccumulator {
    text: String,
    model: Option<String>,
    finish_reason: Option<String>,
    prompt_feedback: Option<Value>,
    usage: Option<Value>,
    /// A chunk carrying `error` rather than a candidate. Some gateways report a
    /// mid-stream failure this way, with the HTTP status long since sent.
    error: Option<Value>,
    /// Whether any chunk at all arrived, so a connection that opened and closed
    /// is not mistaken for a model that answered with nothing.
    saw_chunk: bool,
}

impl StreamAccumulator {
    fn chunk(&mut self, data: &Value, on_delta: DeltaSink<'_>) {
        self.saw_chunk = true;

        if let Some(error) = data.get("error") {
            self.error = Some(error.clone());
            return;
        }
        if let Some(feedback) = data.get("promptFeedback") {
            self.prompt_feedback = Some(feedback.clone());
        }
        if let Some(version) = data.get("modelVersion").and_then(Value::as_str) {
            if self.model.is_none() {
                self.model = Some(version.to_string());
            }
        }
        if let Some(usage) = data.get("usageMetadata") {
            self.usage = Some(usage.clone());
        }

        let Some(candidate) = data
            .get("candidates")
            .and_then(Value::as_array)
            .and_then(|candidates| candidates.first())
        else {
            return;
        };
        if let Some(reason) = candidate.get("finishReason").and_then(Value::as_str) {
            self.finish_reason = Some(reason.to_string());
        }
        for text in answer_parts(candidate) {
            self.text.push_str(&text);
            on_delta(&text);
        }
    }

    fn into_payload(self) -> Result<Value, ProviderError> {
        if !self.saw_chunk {
            return Err(ProviderError::Transport(
                "the stream carried no chunks at all, so nothing was answered".to_string(),
            ));
        }
        // A stream that stops before the model says why is a cut connection,
        // not an answer that happens to be short. Reporting it as an answer
        // would hand a truncated value to a caller that has no way to tell.
        if self.error.is_none() && self.finish_reason.is_none() {
            return Err(ProviderError::Transport(
                "the stream ended before the answer did: no finish reason arrived".to_string(),
            ));
        }

        let mut payload = Map::new();
        if let Some(error) = self.error {
            payload.insert("error".into(), error);
            return Ok(Value::Object(payload));
        }
        payload.insert(
            "candidates".into(),
            json!([{
                "content": { "role": "model", "parts": [{ "text": self.text }] },
                "finishReason": self.finish_reason,
            }]),
        );
        if let Some(feedback) = self.prompt_feedback {
            payload.insert("promptFeedback".into(), feedback);
        }
        if let Some(usage) = self.usage {
            payload.insert("usageMetadata".into(), usage);
        }
        if let Some(model) = self.model {
            payload.insert("modelVersion".into(), json!(model));
        }
        Ok(Value::Object(payload))
    }
}

/// The text a candidate actually answered with.
///
/// A part marked `thought` is the model reasoning, not the answer, and is
/// skipped here and nowhere else — which is what keeps the promise that what a
/// watcher sees live is exactly the text that becomes the answer.
fn answer_parts(candidate: &Value) -> Vec<String> {
    candidate
        .get("content")
        .and_then(|content| content.get("parts"))
        .and_then(Value::as_array)
        .map(|parts| {
            parts
                .iter()
                .filter(|part| part.get("thought").and_then(Value::as_bool) != Some(true))
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Why a candidate stopped, in the terms the rest of the runtime uses.
///
/// Everything that is not `STOP` and not a truncation is a refusal. Listing the
/// refusal reasons rather than matching them would mean a reason added later
/// arriving as "the model returned no text", which sends an operator looking at
/// the prompt instead of at the policy that blocked it.
fn finish_failure(reason: &str, limit: u32) -> Option<ProviderError> {
    match reason {
        "STOP" | "" => None,
        "MAX_TOKENS" => Some(ProviderError::Truncated { limit }),
        other => Some(ProviderError::Refused {
            category: Some(other.to_string()),
            explanation: None,
        }),
    }
}

/// Turn a `generateContent` response into a typed value.
pub(crate) fn parse_response(
    request: &CompletionRequest,
    payload: &Value,
) -> Result<CompletionResponse, ProviderError> {
    if let Some(error) = payload.get("error") {
        let status = error
            .get("code")
            .and_then(Value::as_u64)
            .and_then(|code| u16::try_from(code).ok())
            .unwrap_or(500);
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("the provider reported an error without a message");
        return Err(ProviderError::Request {
            status,
            message: message.to_string(),
        });
    }

    // Before the candidates, because a blocked prompt produces none at all and
    // reading them first turns a refusal into a confusing parse error.
    if let Some(reason) = payload
        .get("promptFeedback")
        .and_then(|feedback| feedback.get("blockReason"))
        .and_then(Value::as_str)
    {
        return Err(ProviderError::Refused {
            category: Some(reason.to_string()),
            explanation: Some("the prompt was blocked before the model saw it".to_string()),
        });
    }

    let candidate = payload
        .get("candidates")
        .and_then(Value::as_array)
        .and_then(|candidates| candidates.first())
        .ok_or_else(|| {
            ProviderError::InvalidResponse("the response carries no candidates".to_string())
        })?;

    let finish = candidate
        .get("finishReason")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if let Some(error) = finish_failure(finish, request.max_tokens) {
        return Err(error);
    }

    let text = answer_parts(candidate).join("");
    if text.trim().is_empty() {
        return Err(ProviderError::InvalidResponse(format!(
            "the provider returned no text (finish reason: `{finish}`)"
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

    let usage = payload.get("usageMetadata");
    Ok(CompletionResponse {
        value,
        usage: Usage {
            input_tokens: usage
                .and_then(|u| u.get("promptTokenCount"))
                .and_then(Value::as_u64)
                .unwrap_or(0),
            output_tokens: usage
                .and_then(|u| u.get("candidatesTokenCount"))
                .and_then(Value::as_u64)
                .unwrap_or(0),
            cache_read_tokens: usage
                .and_then(|u| u.get("cachedContentTokenCount"))
                .and_then(Value::as_u64)
                .unwrap_or(0),
        },
        model: payload
            .get("modelVersion")
            .and_then(Value::as_str)
            .unwrap_or("gemini")
            .to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema;
    use ingot_ir::{FieldType, RecordType};
    use std::collections::BTreeMap;

    fn request(response_type: &str) -> CompletionRequest {
        request_with(response_type, &BTreeMap::new())
    }

    fn request_with(
        response_type: &str,
        types: &BTreeMap<String, RecordType>,
    ) -> CompletionRequest {
        let shape = schema::response_shape(response_type, types).unwrap();
        CompletionRequest {
            node: "n0".into(),
            model: ModelSelection::Exact("google/gemini-test".into()),
            system: None,
            prompt: "Write something".into(),
            context: vec![("document".into(), json!("the source text"))],
            response_type: response_type.into(),
            shape,
            max_tokens: 4096,
        }
    }

    fn provider() -> GoogleProvider {
        GoogleProvider::with_key("test-key")
    }

    fn hit(response_type: &str, payload: Value) -> Result<CompletionResponse, ProviderError> {
        parse_response(&request(response_type), &payload)
    }

    fn answered(text: &str, finish: &str) -> Value {
        json!({
            "candidates": [{
                "content": { "role": "model", "parts": [{ "text": text }] },
                "finishReason": finish,
            }],
            "usageMetadata": { "promptTokenCount": 12, "candidatesTokenCount": 5 },
            "modelVersion": "gemini-test",
        })
    }

    /// Drive the accumulator the way `complete_streaming` does.
    fn accumulate(chunks: &[Value]) -> (Vec<String>, Result<Value, ProviderError>) {
        let mut shown = Vec::new();
        let mut accumulator = StreamAccumulator::default();
        for chunk in chunks {
            accumulator.chunk(chunk, &mut |text| shown.push(text.to_string()));
        }
        (shown, accumulator.into_payload())
    }

    fn chunk(text: &str, finish: Option<&str>) -> Value {
        json!({
            "candidates": [{
                "content": { "role": "model", "parts": [{ "text": text }] },
                "finishReason": finish,
            }],
            "modelVersion": "gemini-test",
        })
    }

    // --- the request ------------------------------------------------------

    #[test]
    fn the_key_travels_in_a_header_rather_than_the_url() {
        // A URL reaches proxy logs, error messages and crash reports. A header
        // does not, and the API accepts both.
        let provider = provider();
        let url = provider.endpoint("gemini-test", "generateContent").unwrap();
        assert!(!url.contains("test-key"), "{url}");
        assert_eq!(provider.headers()[0].0, "x-goog-api-key");
    }

    #[test]
    fn the_model_and_the_method_are_path_segments() {
        let url = provider()
            .endpoint("gemini-test", "generateContent")
            .unwrap();
        assert!(
            url.ends_with("/models/gemini-test:generateContent"),
            "{url}"
        );
    }

    #[test]
    fn a_base_url_that_is_already_an_endpoint_is_refused_rather_than_concatenated() {
        let provider =
            provider().with_base_url("https://example.test/v1beta/models/x:generateContent");
        let error = provider
            .endpoint("gemini-test", "generateContent")
            .unwrap_err();
        assert!(error.to_string().contains("API base"), "{error}");
    }

    #[test]
    fn sampling_parameters_are_never_sent() {
        let body = provider().build_body(&request("markdown")).unwrap();
        let generation = &body["generationConfig"];
        for rejected in ["temperature", "topP", "topK"] {
            assert!(
                generation.get(rejected).is_none(),
                "`{rejected}` must not be sent"
            );
        }
    }

    #[test]
    fn prose_requests_are_not_constrained_to_json() {
        let body = provider().build_body(&request("markdown")).unwrap();
        let generation = &body["generationConfig"];
        assert!(generation.get("responseSchema").is_none());
        assert!(generation.get("responseMimeType").is_none());
    }

    #[test]
    fn context_is_wrapped_in_named_tags() {
        let body = provider().build_body(&request("markdown")).unwrap();
        let text = body["contents"][0]["parts"][0]["text"].as_str().unwrap();
        assert!(text.contains("<document>"), "{text}");
        assert!(text.contains("the source text"), "{text}");
        assert!(text.ends_with("Write something"), "{text}");
    }

    #[test]
    fn a_system_prompt_becomes_a_system_instruction() {
        let mut asked = request("markdown");
        asked.system = Some("You are terse.".into());
        let body = provider().build_body(&asked).unwrap();
        assert_eq!(
            body["systemInstruction"]["parts"][0]["text"],
            "You are terse."
        );
    }

    #[test]
    fn effort_is_refused_rather_than_guessed_at() {
        // Gemini's thinking control differs per model generation. Sending the
        // wrong one is a rejected request; sending neither and saying nothing
        // would let an operator believe the flag took effect.
        let provider = provider().with_effort(Some("high".into()));
        let error = provider.build_body(&request("markdown")).unwrap_err();
        assert!(error.to_string().contains("--effort high"), "{error}");
    }

    #[test]
    fn effort_only_bites_when_this_provider_answers() {
        // Checked per request, so exporting a Gemini key does not break
        // `--effort` for an artifact pinned to another vendor.
        let provider = provider().with_effort(None);
        assert!(provider.build_body(&request("markdown")).is_ok());
    }

    #[test]
    fn a_model_is_never_guessed() {
        let mut asked = request("markdown");
        asked.model = ModelSelection::Default;
        let error = provider().resolve_model(&asked.model).unwrap_err();
        assert!(error.to_string().contains("no default model"), "{error}");
    }

    // --- the schema translation ------------------------------------------

    #[test]
    fn a_wrapped_scalar_schema_becomes_an_openapi_object() {
        let body = provider().build_body(&request("string[]")).unwrap();
        let schema = &body["generationConfig"]["responseSchema"];
        assert_eq!(schema["type"], "OBJECT");
        assert_eq!(schema["properties"]["value"]["type"], "ARRAY");
        assert_eq!(schema["properties"]["value"]["items"]["type"], "STRING");
        assert_eq!(
            body["generationConfig"]["responseMimeType"],
            "application/json"
        );
    }

    #[test]
    fn additional_properties_is_dropped_because_the_subset_has_no_such_field() {
        // The one deliberate loosening, and it costs nothing: `schema::validate`
        // has never rejected an extra field, so the accepted value is the same.
        let body = provider().build_body(&request("string[]")).unwrap();
        let schema = &body["generationConfig"]["responseSchema"];
        assert!(schema.get("additionalProperties").is_none(), "{schema}");
    }

    #[test]
    fn a_record_keeps_the_order_its_fields_were_declared_in() {
        let types: BTreeMap<String, RecordType> = [(
            "hit".to_string(),
            RecordType {
                fields: vec![
                    FieldType {
                        name: "title".into(),
                        ty: "string".into(),
                    },
                    FieldType {
                        name: "score".into(),
                        ty: "int".into(),
                    },
                ],
            },
        )]
        .into_iter()
        .collect();

        let body = provider().build_body(&request_with("hit", &types)).unwrap();
        let schema = &body["generationConfig"]["responseSchema"];
        assert_eq!(schema["type"], "OBJECT");
        assert_eq!(schema["propertyOrdering"], json!(["title", "score"]));
        assert_eq!(schema["properties"]["score"]["type"], "INTEGER");
    }

    #[test]
    fn a_type_with_no_faithful_translation_is_refused_rather_than_weakened() {
        // A `json` field has no shape to constrain, and a schema quietly
        // stripped of it is a constraint the caller believes in and lacks.
        let types: BTreeMap<String, RecordType> = [(
            "envelope".to_string(),
            RecordType {
                fields: vec![FieldType {
                    name: "payload".into(),
                    ty: "json".into(),
                }],
            },
        )]
        .into_iter()
        .collect();

        let error = provider()
            .build_body(&request_with("envelope", &types))
            .unwrap_err();
        assert!(error.to_string().contains("`json`"), "{error}");
        assert!(error.to_string().contains("another provider"), "{error}");
    }

    #[test]
    fn free_json_asks_for_json_without_a_schema() {
        let body = provider().build_body(&request("json")).unwrap();
        let generation = &body["generationConfig"];
        assert_eq!(generation["responseMimeType"], "application/json");
        assert!(generation.get("responseSchema").is_none());
    }

    // --- the response -----------------------------------------------------

    #[test]
    fn prose_responses_come_back_as_text() {
        let response = hit("markdown", answered("# Title\n\nBody", "STOP")).unwrap();
        assert_eq!(response.value, json!("# Title\n\nBody"));
        assert_eq!(response.usage.input_tokens, 12);
        assert_eq!(response.usage.output_tokens, 5);
        assert_eq!(response.model, "gemini-test");
    }

    #[test]
    fn wrapped_structured_responses_are_unwrapped() {
        let response = hit("string[]", answered(r#"{"value":["a","b"]}"#, "STOP")).unwrap();
        assert_eq!(response.value, json!(["a", "b"]));
    }

    #[test]
    fn truncation_is_reported_rather_than_returned_as_a_result() {
        let error = hit("markdown", answered("half an ans", "MAX_TOKENS")).unwrap_err();
        assert!(
            matches!(error, ProviderError::Truncated { limit: 4096 }),
            "{error}"
        );
    }

    #[test]
    fn a_safety_stop_is_a_refusal() {
        let error = hit("markdown", answered("", "SAFETY")).unwrap_err();
        assert!(matches!(error, ProviderError::Refused { .. }), "{error}");
        assert!(error.to_string().contains("SAFETY"), "{error}");
    }

    #[test]
    fn an_unfamiliar_stop_reason_is_a_refusal_rather_than_an_empty_answer() {
        // A reason added later must not arrive as "the model returned no text",
        // which sends an operator looking at the prompt instead of the policy.
        let error = hit("markdown", answered("", "SOMETHING_NEW")).unwrap_err();
        assert!(matches!(error, ProviderError::Refused { .. }), "{error}");
    }

    #[test]
    fn a_blocked_prompt_is_reported_before_the_candidates_are_read() {
        let payload = json!({ "promptFeedback": { "blockReason": "SAFETY" } });
        let error = hit("markdown", payload).unwrap_err();
        assert!(matches!(error, ProviderError::Refused { .. }), "{error}");
        assert!(
            error.to_string().contains("before the model saw it"),
            "{error}"
        );
    }

    #[test]
    fn an_error_payload_carries_its_status_through() {
        let payload = json!({ "error": { "code": 429, "message": "quota exhausted" } });
        let error = hit("markdown", payload).unwrap_err();
        assert!(
            matches!(error, ProviderError::Request { status: 429, .. }),
            "{error}"
        );
    }

    #[test]
    fn a_non_json_structured_response_is_an_error() {
        let error = hit("string[]", answered("not json at all", "STOP")).unwrap_err();
        assert!(
            matches!(error, ProviderError::InvalidResponse(_)),
            "{error}"
        );
    }

    #[test]
    fn thought_parts_are_ignored_when_collecting_text() {
        let payload = json!({
            "candidates": [{
                "content": { "parts": [
                    { "text": "let me think", "thought": true },
                    { "text": "the answer" },
                ]},
                "finishReason": "STOP",
            }],
        });
        let response = hit("markdown", payload).unwrap();
        assert_eq!(response.value, json!("the answer"));
    }

    // --- streaming --------------------------------------------------------

    #[test]
    fn the_provider_advertises_that_it_streams() {
        assert!(provider().streams());
    }

    #[test]
    fn streamed_text_accumulates_in_arrival_order_and_is_shown_as_it_arrives() {
        let (shown, payload) = accumulate(&[
            chunk("Half ", None),
            chunk("an ", None),
            chunk("answer", Some("STOP")),
        ]);
        assert_eq!(shown, vec!["Half ", "an ", "answer"]);
        let response = parse_response(&request("markdown"), &payload.unwrap()).unwrap();
        assert_eq!(response.value, json!("Half an answer"));
    }

    #[test]
    fn only_the_text_that_becomes_the_answer_is_shown_live() {
        // The promise a watcher relies on: never a sentence the run throws away.
        let thinking = json!({
            "candidates": [{
                "content": { "parts": [{ "text": "reasoning", "thought": true }] },
            }],
        });
        let (shown, _) = accumulate(&[thinking, chunk("the answer", Some("STOP"))]);
        assert_eq!(shown, vec!["the answer"]);
    }

    #[test]
    fn the_same_answer_streamed_or_at_once_produces_the_same_response() {
        // The point of rebuilding the whole-body shape: one parser, two
        // transports, so the two paths cannot drift.
        let (_, payload) = accumulate(&[
            chunk("# Title\n\n", None),
            json!({
                "candidates": [{
                    "content": { "parts": [{ "text": "Body" }] },
                    "finishReason": "STOP",
                }],
                "usageMetadata": { "promptTokenCount": 12, "candidatesTokenCount": 5 },
            }),
        ]);
        let streamed = parse_response(&request("markdown"), &payload.unwrap()).unwrap();
        let at_once = hit("markdown", answered("# Title\n\nBody", "STOP")).unwrap();
        assert_eq!(streamed, at_once);
    }

    #[test]
    fn a_streamed_truncation_produces_the_same_error_as_a_truncated_response() {
        let (_, payload) = accumulate(&[chunk("half an ans", Some("MAX_TOKENS"))]);
        let error = parse_response(&request("markdown"), &payload.unwrap()).unwrap_err();
        assert!(matches!(error, ProviderError::Truncated { .. }), "{error}");
    }

    #[test]
    fn a_stream_that_ends_before_the_answer_does_is_not_an_answer() {
        let (_, payload) = accumulate(&[chunk("half an ans", None)]);
        let error = payload.unwrap_err();
        assert!(error.to_string().contains("ended before"), "{error}");
    }

    #[test]
    fn a_stream_that_carried_nothing_says_so() {
        let (_, payload) = accumulate(&[]);
        let error = payload.unwrap_err();
        assert!(error.to_string().contains("no chunks"), "{error}");
    }

    #[test]
    fn an_error_reported_mid_stream_stops_the_answer_rather_than_completing_it() {
        let (_, payload) = accumulate(&[
            chunk("partial", None),
            json!({ "error": { "code": 500, "message": "backend died" } }),
        ]);
        let error = parse_response(&request("markdown"), &payload.unwrap()).unwrap_err();
        assert!(
            matches!(error, ProviderError::Request { status: 500, .. }),
            "{error}"
        );
    }

    #[test]
    fn usage_from_the_final_chunk_reaches_the_response() {
        let (_, payload) = accumulate(&[
            chunk("ok", None),
            json!({
                "candidates": [{ "finishReason": "STOP" }],
                "usageMetadata": {
                    "promptTokenCount": 30,
                    "candidatesTokenCount": 7,
                    "cachedContentTokenCount": 3,
                },
            }),
        ]);
        let response = parse_response(&request("markdown"), &payload.unwrap()).unwrap();
        assert_eq!(response.usage.input_tokens, 30);
        assert_eq!(response.usage.output_tokens, 7);
        assert_eq!(response.usage.cache_read_tokens, 3);
    }
}
