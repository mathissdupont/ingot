//! Recorded model exchanges, for deterministic offline runs.
//!
//! A cassette is what makes `ingot test` runnable in CI: no API key, no network,
//! and the same answers every time. Replay matches interactions by order and
//! verifies the request digest, so an edited prompt fails loudly instead of
//! quietly reusing the previous recording.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::provider::{CompletionRequest, CompletionResponse, ModelProvider, ProviderError, Usage};

pub const CASSETTE_VERSION: &str = "0.1";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cassette {
    pub cassette_version: String,
    /// Fully qualified name of the agent this was recorded against.
    pub agent: String,
    /// The inputs the recording was made with.
    ///
    /// Kept in the cassette so it is self-contained: replaying needs no
    /// side-car file, and there is no way to pair a recording with the wrong
    /// inputs and get a confusing digest mismatch instead of a clear one.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub inputs: BTreeMap<String, Value>,
    pub interactions: Vec<Interaction>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Interaction {
    pub index: usize,
    /// The IR node that made the call.
    pub node: String,
    /// Digest of everything that determined the answer.
    pub request_digest: String,
    pub response_type: String,
    /// The value returned, already typed as the caller declared.
    pub value: Value,
    pub usage: Usage,
    /// The model that answered, recorded for provenance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

impl Cassette {
    pub fn new(agent: impl Into<String>) -> Cassette {
        Cassette {
            cassette_version: CASSETTE_VERSION.to_string(),
            agent: agent.into(),
            inputs: BTreeMap::new(),
            interactions: Vec::new(),
        }
    }

    /// The canonical encoding: two-space indentation, trailing newline.
    ///
    /// Cassettes are checked in and reviewed, so they get the same stability
    /// treatment as golden IR.
    pub fn to_canonical_json(&self) -> String {
        let mut json =
            serde_json::to_string_pretty(self).expect("a cassette is always serializable");
        json.push('\n');
        json
    }

    pub fn from_json(text: &str) -> Result<Cassette, String> {
        let cassette: Cassette =
            serde_json::from_str(text).map_err(|error| format!("invalid cassette: {error}"))?;
        if cassette.cassette_version != CASSETTE_VERSION {
            return Err(format!(
                "cassette version `{}` is not supported by this compiler (expected `{CASSETTE_VERSION}`)",
                cassette.cassette_version
            ));
        }
        Ok(cassette)
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Cassette, String> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        Cassette::from_json(&text)
    }

    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), String> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
            }
        }
        std::fs::write(path, self.to_canonical_json())
            .map_err(|error| format!("cannot write {}: {error}", path.display()))
    }
}

/// Serves recorded answers in order.
pub struct ReplayProvider {
    cassette: Cassette,
    position: usize,
    /// When false, a digest mismatch is a warning rather than an error. Only
    /// used by tooling that deliberately replays against edited sources.
    strict: bool,
}

impl ReplayProvider {
    pub fn new(cassette: Cassette) -> ReplayProvider {
        ReplayProvider {
            cassette,
            position: 0,
            strict: true,
        }
    }

    pub fn lenient(mut self) -> ReplayProvider {
        self.strict = false;
        self
    }

    /// Interactions recorded but never played back.
    pub fn remaining(&self) -> usize {
        self.cassette
            .interactions
            .len()
            .saturating_sub(self.position)
    }
}

impl ModelProvider for ReplayProvider {
    fn name(&self) -> &str {
        "replay"
    }

    fn complete(
        &mut self,
        request: &CompletionRequest,
    ) -> Result<CompletionResponse, ProviderError> {
        let Some(interaction) = self.cassette.interactions.get(self.position) else {
            return Err(ProviderError::Cassette(format!(
                "the cassette has {} interaction(s) but the run asked for another at node `{}`; \
                 re-record it",
                self.cassette.interactions.len(),
                request.node
            )));
        };
        self.position += 1;

        // Checked before the digest: a changed response type is a specific,
        // nameable difference, and saying so beats the generic "something in
        // the request changed" that the digest can offer.
        if interaction.response_type != request.response_type {
            return Err(ProviderError::Cassette(format!(
                "interaction {} recorded a `{}` response but node `{}` now asks for `{}`; \
                 re-record the cassette",
                interaction.index, interaction.response_type, request.node, request.response_type
            )));
        }

        if self.strict && interaction.request_digest != request.digest() {
            return Err(ProviderError::Cassette(format!(
                "interaction {} was recorded for a different request at node `{}`. \
                 The prompt or its context changed since recording — \
                 re-record the cassette and review the diff.",
                interaction.index, request.node
            )));
        }

        Ok(CompletionResponse {
            value: interaction.value.clone(),
            usage: interaction.usage,
            model: interaction
                .model
                .clone()
                .unwrap_or_else(|| "replay".to_string()),
        })
    }
}

/// Wraps another provider and records everything it answers.
pub struct RecordingProvider<P: ModelProvider> {
    inner: P,
    cassette: Cassette,
}

impl<P: ModelProvider> RecordingProvider<P> {
    pub fn new(inner: P, agent: impl Into<String>) -> RecordingProvider<P> {
        RecordingProvider {
            inner,
            cassette: Cassette::new(agent),
        }
    }

    /// Record the inputs alongside the interactions.
    pub fn with_inputs(mut self, inputs: BTreeMap<String, Value>) -> RecordingProvider<P> {
        self.cassette.inputs = inputs;
        self
    }

    pub fn finish(self) -> Cassette {
        self.cassette
    }
}

impl<P: ModelProvider> ModelProvider for RecordingProvider<P> {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn complete(
        &mut self,
        request: &CompletionRequest,
    ) -> Result<CompletionResponse, ProviderError> {
        let response = self.inner.complete(request)?;
        self.cassette.interactions.push(Interaction {
            index: self.cassette.interactions.len(),
            node: request.node.clone(),
            request_digest: request.digest(),
            response_type: request.response_type.clone(),
            value: response.value.clone(),
            usage: response.usage,
            model: Some(response.model.clone()),
        });
        Ok(response)
    }
}

/// Answers from a fixed script, ignoring the request. Test scaffolding.
pub struct ScriptedProvider {
    answers: Vec<Value>,
    position: usize,
    usage: Usage,
}

impl ScriptedProvider {
    pub fn new(answers: Vec<Value>) -> ScriptedProvider {
        ScriptedProvider {
            answers,
            position: 0,
            usage: Usage {
                input_tokens: 10,
                output_tokens: 5,
                cache_read_tokens: 0,
            },
        }
    }

    pub fn with_usage(mut self, usage: Usage) -> ScriptedProvider {
        self.usage = usage;
        self
    }

    /// Requests served so far.
    pub fn calls(&self) -> usize {
        self.position
    }
}

impl ModelProvider for ScriptedProvider {
    fn name(&self) -> &str {
        "scripted"
    }

    fn complete(
        &mut self,
        request: &CompletionRequest,
    ) -> Result<CompletionResponse, ProviderError> {
        let Some(value) = self.answers.get(self.position).cloned() else {
            return Err(ProviderError::Cassette(format!(
                "the script has {} answer(s) but node `{}` asked for another",
                self.answers.len(),
                request.node
            )));
        };
        self.position += 1;
        Ok(CompletionResponse {
            value,
            usage: self.usage,
            model: "scripted".to_string(),
        })
    }
}

/// Loads every cassette in a directory, keyed by file stem.
pub fn load_directory(dir: impl AsRef<Path>) -> Result<BTreeMap<String, Cassette>, String> {
    let dir = dir.as_ref();
    let mut cassettes = BTreeMap::new();
    let entries = std::fs::read_dir(dir)
        .map_err(|error| format!("cannot read {}: {error}", dir.display()))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let name = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or_default()
            .to_string();
        cassettes.insert(name, Cassette::load(&path)?);
    }
    Ok(cassettes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::ResponseShape;
    use serde_json::json;

    fn request(node: &str, prompt: &str) -> CompletionRequest {
        CompletionRequest {
            node: node.into(),
            model: crate::provider::ModelSelection::Default,
            system: None,
            prompt: prompt.into(),
            context: Vec::new(),
            response_type: "markdown".into(),
            shape: ResponseShape::Prose,
            max_tokens: 1024,
        }
    }

    fn recorded() -> Cassette {
        let mut provider = RecordingProvider::new(
            ScriptedProvider::new(vec![json!("first"), json!("second")]),
            "test.Agent",
        );
        provider.complete(&request("n0", "one")).unwrap();
        provider.complete(&request("n1", "two")).unwrap();
        provider.finish()
    }

    #[test]
    fn a_cassette_round_trips_through_canonical_json() {
        let cassette = recorded();
        let parsed = Cassette::from_json(&cassette.to_canonical_json()).unwrap();
        assert_eq!(parsed, cassette);
    }

    #[test]
    fn canonical_json_ends_with_a_newline() {
        assert!(recorded().to_canonical_json().ends_with("}\n"));
    }

    #[test]
    fn replay_serves_recorded_answers_in_order() {
        let mut provider = ReplayProvider::new(recorded());
        assert_eq!(
            provider.complete(&request("n0", "one")).unwrap().value,
            json!("first")
        );
        assert_eq!(
            provider.complete(&request("n1", "two")).unwrap().value,
            json!("second")
        );
        assert_eq!(provider.remaining(), 0);
    }

    #[test]
    fn replay_rejects_a_changed_prompt() {
        let mut provider = ReplayProvider::new(recorded());
        let error = provider
            .complete(&request("n0", "a different prompt"))
            .unwrap_err();
        let message = error.to_string();
        assert!(
            message.contains("recorded for a different request"),
            "{message}"
        );
        assert!(message.contains("re-record"), "{message}");
    }

    #[test]
    fn replay_rejects_a_changed_response_type() {
        let mut provider = ReplayProvider::new(recorded());
        let mut changed = request("n0", "one");
        changed.response_type = "string".into();
        let error = provider.complete(&changed).unwrap_err();
        assert!(
            error.to_string().contains("recorded a `markdown` response"),
            "{error}"
        );
    }

    #[test]
    fn replay_reports_an_exhausted_cassette() {
        let mut provider = ReplayProvider::new(recorded());
        provider.complete(&request("n0", "one")).unwrap();
        provider.complete(&request("n1", "two")).unwrap();
        let error = provider.complete(&request("n2", "three")).unwrap_err();
        assert!(error.to_string().contains("asked for another"), "{error}");
    }

    #[test]
    fn a_future_cassette_version_is_rejected() {
        let mut cassette = recorded();
        cassette.cassette_version = "9.0".into();
        let error = Cassette::from_json(&cassette.to_canonical_json()).unwrap_err();
        assert!(error.contains("not supported"), "{error}");
    }

    #[test]
    fn lenient_replay_tolerates_a_changed_prompt() {
        let mut provider = ReplayProvider::new(recorded()).lenient();
        assert!(provider.complete(&request("n0", "changed")).is_ok());
    }
}
