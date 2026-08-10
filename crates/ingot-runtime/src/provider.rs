//! The model provider interface.
//!
//! Everything vendor-specific lives behind [`ModelProvider`]. The interpreter
//! builds a [`CompletionRequest`] from the IR and never learns which provider
//! answered it.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::schema::ResponseShape;

/// Which model to use, as the artifact stated it.
#[derive(Debug, Clone, PartialEq)]
pub enum ModelSelection {
    /// A pinned `provider/model` reference.
    Exact(String),
    /// Capability requirements the provider must satisfy.
    Capabilities {
        capabilities: Vec<String>,
        min_context_tokens: Option<i64>,
    },
    /// The artifact stated no preference.
    Default,
}

/// One model call.
#[derive(Debug, Clone)]
pub struct CompletionRequest {
    /// Node id, for cassette matching and error messages.
    pub node: String,
    pub model: ModelSelection,
    pub system: Option<String>,
    pub prompt: String,
    /// Named context values rendered into the request, in declaration order.
    pub context: Vec<(String, Value)>,
    /// The Ingot type the caller declared.
    pub response_type: String,
    pub shape: ResponseShape,
    /// Upper bound on output tokens for this call.
    pub max_tokens: u32,
}

impl CompletionRequest {
    /// A stable digest of everything that determines the answer.
    ///
    /// Cassette replay compares this, so an edited prompt produces a loud
    /// mismatch instead of a stale answer from the previous recording.
    pub fn digest(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.node.as_bytes());
        hasher.update([0]);
        hasher.update(self.system.as_deref().unwrap_or("").as_bytes());
        hasher.update([0]);
        hasher.update(self.prompt.as_bytes());
        hasher.update([0]);
        hasher.update(self.response_type.as_bytes());
        for (name, value) in &self.context {
            hasher.update([0]);
            hasher.update(name.as_bytes());
            hasher.update([0]);
            // to_string on serde_json::Value sorts object keys, so this is
            // stable across runs.
            hasher.update(value.to_string().as_bytes());
        }
        format!("{:x}", hasher.finalize())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompletionResponse {
    /// The value, already unwrapped and typed as the caller declared.
    pub value: Value,
    pub usage: Usage,
    /// The model that actually answered, for the event stream.
    pub model: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub cache_read_tokens: u64,
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

impl Usage {
    pub fn total(&self) -> u64 {
        self.input_tokens + self.output_tokens
    }

    pub fn add(&mut self, other: Usage) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.cache_read_tokens += other.cache_read_tokens;
    }
}

#[derive(Debug)]
pub enum ProviderError {
    /// The provider could not be reached, or the transport failed.
    Transport(String),
    /// The provider rejected the request.
    Request { status: u16, message: String },
    /// The provider is rate limiting; retry after this many seconds if known.
    RateLimited { retry_after_seconds: Option<u64> },
    /// The provider declined to answer on safety grounds.
    Refused {
        category: Option<String>,
        explanation: Option<String>,
    },
    /// The answer did not match the declared response type.
    InvalidResponse(String),
    /// The response was cut off before it finished.
    Truncated { limit: u32 },
    /// Configuration is missing or wrong (no API key, unusable model).
    Configuration(String),
    /// Cassette replay could not serve this request.
    Cassette(String),
}

impl fmt::Display for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProviderError::Transport(message) => write!(f, "provider transport failed: {message}"),
            ProviderError::Request { status, message } => {
                write!(f, "provider rejected the request ({status}): {message}")
            }
            ProviderError::RateLimited {
                retry_after_seconds: Some(seconds),
            } => {
                write!(f, "provider is rate limiting; retry after {seconds}s")
            }
            ProviderError::RateLimited { .. } => write!(f, "provider is rate limiting"),
            ProviderError::Refused {
                category,
                explanation,
            } => {
                write!(f, "the provider declined to answer")?;
                if let Some(category) = category {
                    write!(f, " ({category})")?;
                }
                if let Some(explanation) = explanation {
                    write!(f, ": {explanation}")?;
                }
                Ok(())
            }
            ProviderError::InvalidResponse(message) => {
                write!(f, "the response did not match the declared type: {message}")
            }
            ProviderError::Truncated { limit } => {
                write!(f, "the response was cut off at the {limit} token limit")
            }
            ProviderError::Configuration(message) => {
                write!(f, "provider not configured: {message}")
            }
            ProviderError::Cassette(message) => write!(f, "cassette replay failed: {message}"),
        }
    }
}

impl std::error::Error for ProviderError {}

/// Text as it arrives, handed over before the answer is complete.
///
/// A delta is for watching, never for deciding. Nothing downstream may parse
/// one, validate one, or bind one to a name: the value a run uses is always
/// assembled from the finished response and validated whole. See
/// [Runtime 0.3 §2](../../../specs/runtime/v0.3.md).
pub type DeltaSink<'a> = &'a mut dyn FnMut(&str);

/// A source of model completions.
pub trait ModelProvider {
    /// Short name used in events and diagnostics, e.g. `anthropic` or `replay`.
    fn name(&self) -> &str;

    fn complete(
        &mut self,
        request: &CompletionRequest,
    ) -> Result<CompletionResponse, ProviderError>;

    /// Whether this provider delivers an answer incrementally.
    ///
    /// Read by the interpreter before it decides how many output tokens one
    /// call may ask for: a service that must compose a whole body before
    /// sending it holds the connection open for the length of the answer, and
    /// several refuse a large `max_tokens` outright unless the request streams.
    /// The ceiling is therefore a property of the transport, and a provider
    /// that says `false` here keeps the smaller one.
    fn streams(&self) -> bool {
        false
    }

    /// Complete a request, handing text to `on_delta` as it arrives.
    ///
    /// The default is [`ModelProvider::complete`] with the deltas dropped,
    /// which is the honest answer for a provider that has nothing live to
    /// show — a cassette replay produces its answer at once, and inventing
    /// deltas for it would make a replayed run look like a call that never
    /// happened.
    ///
    /// The returned response is what the run uses. Whatever reached `on_delta`
    /// is a display artifact: on any error it is discarded, including when the
    /// answer was cut off part-way through.
    fn complete_streaming(
        &mut self,
        request: &CompletionRequest,
        on_delta: DeltaSink<'_>,
    ) -> Result<CompletionResponse, ProviderError> {
        let _ = on_delta;
        self.complete(request)
    }
}

/// Lets a boxed provider be used wherever a provider is expected — including as
/// the inner provider of a [`crate::RecordingProvider`], which is how the CLI
/// wraps a recorder around a provider it chose at runtime.
impl<P: ModelProvider + ?Sized> ModelProvider for Box<P> {
    fn name(&self) -> &str {
        (**self).name()
    }

    fn complete(
        &mut self,
        request: &CompletionRequest,
    ) -> Result<CompletionResponse, ProviderError> {
        (**self).complete(request)
    }

    fn streams(&self) -> bool {
        (**self).streams()
    }

    fn complete_streaming(
        &mut self,
        request: &CompletionRequest,
        on_delta: DeltaSink<'_>,
    ) -> Result<CompletionResponse, ProviderError> {
        (**self).complete_streaming(request, on_delta)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn request() -> CompletionRequest {
        CompletionRequest {
            node: "n0".into(),
            model: ModelSelection::Default,
            system: None,
            prompt: "Summarise this".into(),
            context: vec![("document".into(), json!("hello"))],
            response_type: "markdown".into(),
            shape: ResponseShape::Prose,
            max_tokens: 4096,
        }
    }

    #[test]
    fn the_digest_is_stable() {
        assert_eq!(request().digest(), request().digest());
    }

    #[test]
    fn the_digest_changes_with_the_prompt() {
        let mut other = request();
        other.prompt = "Summarise this document".into();
        assert_ne!(request().digest(), other.digest());
    }

    #[test]
    fn the_digest_changes_with_the_context() {
        let mut other = request();
        other.context = vec![("document".into(), json!("goodbye"))];
        assert_ne!(request().digest(), other.digest());
    }

    #[test]
    fn the_digest_changes_with_the_response_type() {
        let mut other = request();
        other.response_type = "text".into();
        assert_ne!(request().digest(), other.digest());
    }

    #[test]
    fn a_provider_that_cannot_stream_answers_at_once_and_shows_nothing() {
        struct AtOnce;
        impl ModelProvider for AtOnce {
            fn name(&self) -> &str {
                "at-once"
            }
            fn complete(
                &mut self,
                _request: &CompletionRequest,
            ) -> Result<CompletionResponse, ProviderError> {
                Ok(CompletionResponse {
                    value: json!("the whole answer"),
                    usage: Usage::default(),
                    model: "at-once".into(),
                })
            }
        }

        let mut seen = Vec::new();
        let response = AtOnce
            .complete_streaming(&request(), &mut |text| seen.push(text.to_string()))
            .unwrap();
        assert_eq!(response.value, json!("the whole answer"));
        assert!(
            seen.is_empty(),
            "a provider with nothing live to show must not invent deltas: {seen:?}"
        );
        assert!(!AtOnce.streams());
    }

    #[test]
    fn usage_accumulates() {
        let mut total = Usage::default();
        total.add(Usage {
            input_tokens: 10,
            output_tokens: 5,
            cache_read_tokens: 0,
        });
        total.add(Usage {
            input_tokens: 1,
            output_tokens: 2,
            cache_read_tokens: 3,
        });
        assert_eq!(total.input_tokens, 11);
        assert_eq!(total.output_tokens, 7);
        assert_eq!(total.cache_read_tokens, 3);
        assert_eq!(total.total(), 18);
    }
}
