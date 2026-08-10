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

use sha2::{Digest, Sha256};

use crate::provider::{CompletionRequest, CompletionResponse, ModelProvider, ProviderError, Usage};
use crate::tools::{ToolError, ToolHost, ToolInvocation};

/// The version this crate writes.
pub const CASSETTE_VERSION: &str = "0.2";

/// Versions this crate can read.
///
/// 0.2 is 0.1 plus `toolCalls`, so a 0.1 recording is a valid 0.2 one with no
/// tool calls in it and keeps replaying unchanged. Re-recording moves it.
pub const SUPPORTED_CASSETTE_VERSIONS: &[&str] = &["0.1", "0.2"];

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
    /// Tool invocations and what they returned, in the order they happened.
    ///
    /// Kept in their own list rather than interleaved with the model exchanges,
    /// because the two are matched independently: an agent may call three tools
    /// between two `ask`s, and a single ordered stream would make each side's
    /// position depend on the other's.
    ///
    /// **A recorded result contains whatever the tool returned.** That is the
    /// review burden this format adds, and why the build-time secret scan reads
    /// cassettes as well as source.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolExchange>,
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

/// One tool invocation and what it returned.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolExchange {
    pub index: usize,
    /// The IR node that made the call.
    pub node: String,
    /// Bare tool name, e.g. `web.search`.
    pub tool: String,
    /// Digest of everything that determined the answer.
    pub invocation_digest: String,
    /// The Ingot type the tool is declared to return.
    pub result_type: String,
    /// What the tool returned, already typed as the artifact declared.
    ///
    /// Absent when the recorded call failed: a failure has no value, and
    /// writing `null` would make "returned nothing" and "did not run"
    /// indistinguishable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    /// The failure the tool produced, when it produced one.
    ///
    /// Recorded rather than dropped: an agent's behaviour when a tool fails is
    /// exactly the behaviour worth having a test for, and a cassette that could
    /// only record success would be a cassette of the happy path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Cassette {
    pub fn new(agent: impl Into<String>) -> Cassette {
        Cassette {
            cassette_version: CASSETTE_VERSION.to_string(),
            agent: agent.into(),
            inputs: BTreeMap::new(),
            interactions: Vec::new(),
            tool_calls: Vec::new(),
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
        if !SUPPORTED_CASSETTE_VERSIONS.contains(&cassette.cassette_version.as_str()) {
            return Err(format!(
                "cassette version `{}` is not supported by this compiler (supported: {})",
                cassette.cassette_version,
                SUPPORTED_CASSETTE_VERSIONS.join(", ")
            ));
        }
        // A 0.1 recording that carries tool calls was written by something that
        // did not mean 0.1. Refusing beats replaying a field the stated version
        // does not have.
        if cassette.cassette_version == "0.1" && !cassette.tool_calls.is_empty() {
            return Err(
                "the cassette states version `0.1` and carries tool calls, which 0.1 has no \
                 field for; re-record it"
                    .to_string(),
            );
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

/// A stable digest of everything that determines what a tool returns.
///
/// The agent is in it because two agents in one program deliberately hold
/// different policies, so the same call from a different agent is a different
/// call. Effects are not: they say what the call is allowed to do, not what it
/// answers.
pub fn invocation_digest(invocation: &ToolInvocation) -> String {
    let mut hasher = Sha256::new();
    hasher.update(invocation.agent.as_bytes());
    hasher.update([0]);
    hasher.update(invocation.reference.as_bytes());
    hasher.update([0]);
    hasher.update(invocation.result_type.as_bytes());
    // A BTreeMap iterates in key order and `to_string` sorts object keys, so
    // this is stable across runs and across machines.
    for (name, value) in &invocation.arguments {
        hasher.update([0]);
        hasher.update(name.as_bytes());
        hasher.update([0]);
        hasher.update(value.to_string().as_bytes());
    }
    format!("{:x}", hasher.finalize())
}

/// Serves recorded tool results in order.
///
/// The same bargain [`ReplayProvider`] strikes, for the other half of a run: no
/// server is started, nothing is reached, and a call the recording does not
/// match fails loudly rather than being answered from the wrong row.
pub struct ReplayToolHost {
    calls: Vec<ToolExchange>,
    position: usize,
    strict: bool,
}

impl ReplayToolHost {
    pub fn new(calls: Vec<ToolExchange>) -> ReplayToolHost {
        ReplayToolHost {
            calls,
            position: 0,
            strict: true,
        }
    }

    pub fn lenient(mut self) -> ReplayToolHost {
        self.strict = false;
        self
    }

    /// Tool calls recorded but never played back.
    pub fn remaining(&self) -> usize {
        self.calls.len().saturating_sub(self.position)
    }
}

impl ToolHost for ReplayToolHost {
    fn name(&self) -> &str {
        "replay"
    }

    /// Every tool, because what a replay can serve is decided by the recording
    /// rather than by a table of names. A call beyond the recording is reported
    /// by [`ToolHost::call`], where the position is known and the message can
    /// say which call it was.
    fn provides(&self, _tool: &str) -> bool {
        true
    }

    fn call(&mut self, invocation: &ToolInvocation) -> Result<Value, ToolError> {
        let Some(recorded) = self.calls.get(self.position) else {
            return Err(ToolError::Failed(format!(
                "the cassette records {} tool call(s) and the run asked for another: `{}`;                  re-record it",
                self.calls.len(),
                invocation.name
            )));
        };
        self.position += 1;

        // Checked before the digest: a different tool is a specific, nameable
        // difference, and saying so beats "something about the call changed".
        if recorded.tool != invocation.name {
            return Err(ToolError::Failed(format!(
                "tool call {} recorded `{}` and the run called `{}`; re-record the cassette",
                recorded.index, recorded.tool, invocation.name
            )));
        }
        if self.strict && recorded.invocation_digest != invocation_digest(invocation) {
            return Err(ToolError::Failed(format!(
                "tool call {} recorded different arguments for `{}`.                  The call changed since recording — re-record the cassette and review the diff.",
                recorded.index, recorded.tool
            )));
        }

        match (&recorded.value, &recorded.error) {
            (Some(value), _) => Ok(value.clone()),
            (None, Some(error)) => Err(ToolError::Failed(error.clone())),
            (None, None) => Err(ToolError::InvalidResult(format!(
                "tool call {} for `{}` recorded neither a value nor an error",
                recorded.index, recorded.tool
            ))),
        }
    }
}

/// Wraps another host and records everything it answers.
pub struct RecordingTools<H: ToolHost> {
    inner: H,
    calls: Vec<ToolExchange>,
}

impl<H: ToolHost> RecordingTools<H> {
    pub fn new(inner: H) -> RecordingTools<H> {
        RecordingTools {
            inner,
            calls: Vec::new(),
        }
    }

    pub fn finish(self) -> Vec<ToolExchange> {
        self.calls
    }
}

impl<H: ToolHost> ToolHost for RecordingTools<H> {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn provides(&self, tool: &str) -> bool {
        self.inner.provides(tool)
    }

    fn call(&mut self, invocation: &ToolInvocation) -> Result<Value, ToolError> {
        let result = self.inner.call(invocation);
        let (value, error) = match &result {
            Ok(value) => (Some(value.clone()), None),
            // A failure is recorded too: how an agent behaves when a tool fails
            // is exactly the behaviour worth testing, and a recording that could
            // only hold successes would be a recording of the happy path.
            Err(error) => (None, Some(error.to_string())),
        };
        self.calls.push(ToolExchange {
            index: self.calls.len(),
            node: invocation.node.clone(),
            tool: invocation.name.clone(),
            invocation_digest: invocation_digest(invocation),
            result_type: invocation.result_type.clone(),
            value,
            error,
        });
        result
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

    // --- tool calls -------------------------------------------------------

    fn tool_call(name: &str, path: &str) -> ToolInvocation {
        ToolInvocation {
            node: "n0".into(),
            agent: "test.Agent".into(),
            reference: format!("mcp:{name}"),
            name: name.into(),
            transport: "mcp".into(),
            arguments: [("path".to_string(), json!(path))].into(),
            effects: vec!["filesystem_read".into()],
            result_type: "text".into(),
        }
    }

    struct FixedTool(Result<Value, &'static str>);

    impl ToolHost for FixedTool {
        fn name(&self) -> &str {
            "fixed"
        }
        fn provides(&self, _tool: &str) -> bool {
            true
        }
        fn call(&mut self, _invocation: &ToolInvocation) -> Result<Value, ToolError> {
            self.0
                .clone()
                .map_err(|error| ToolError::Failed(error.into()))
        }
    }

    #[test]
    fn a_recorded_tool_call_replays_without_reaching_anything() {
        let mut recorder = RecordingTools::new(FixedTool(Ok(json!("# Sample"))));
        recorder
            .call(&tool_call("fs.read_file", "README.md"))
            .unwrap();
        let recorded = recorder.finish();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].tool, "fs.read_file");

        let mut replay = ReplayToolHost::new(recorded);
        assert_eq!(
            replay
                .call(&tool_call("fs.read_file", "README.md"))
                .unwrap(),
            json!("# Sample")
        );
        assert_eq!(replay.remaining(), 0);
    }

    #[test]
    fn a_recorded_failure_replays_as_a_failure() {
        // How an agent behaves when a tool fails is the behaviour most worth
        // testing, so a recording that could only hold successes would be a
        // recording of the happy path.
        let mut recorder = RecordingTools::new(FixedTool(Err("no such file")));
        assert!(recorder
            .call(&tool_call("fs.read_file", "gone.md"))
            .is_err());
        let recorded = recorder.finish();
        assert_eq!(recorded[0].value, None);
        assert!(recorded[0]
            .error
            .as_deref()
            .unwrap()
            .contains("no such file"));

        let mut replay = ReplayToolHost::new(recorded);
        let error = replay
            .call(&tool_call("fs.read_file", "gone.md"))
            .unwrap_err();
        assert!(error.to_string().contains("no such file"), "{error}");
    }

    #[test]
    fn replay_refuses_a_call_whose_arguments_changed() {
        let mut recorder = RecordingTools::new(FixedTool(Ok(json!("# Sample"))));
        recorder
            .call(&tool_call("fs.read_file", "README.md"))
            .unwrap();
        let mut replay = ReplayToolHost::new(recorder.finish());

        let error = replay
            .call(&tool_call("fs.read_file", "notes.md"))
            .unwrap_err();
        assert!(error.to_string().contains("re-record"), "{error}");
    }

    #[test]
    fn replay_refuses_a_different_tool_by_name() {
        let mut recorder = RecordingTools::new(FixedTool(Ok(json!("# Sample"))));
        recorder
            .call(&tool_call("fs.read_file", "README.md"))
            .unwrap();
        let mut replay = ReplayToolHost::new(recorder.finish());

        let error = replay
            .call(&tool_call("fs.list_dir", "README.md"))
            .unwrap_err();
        let message = error.to_string();
        assert!(message.contains("fs.read_file"), "{message}");
        assert!(message.contains("fs.list_dir"), "{message}");
    }

    #[test]
    fn replay_reports_a_call_beyond_the_recording() {
        let mut replay = ReplayToolHost::new(Vec::new());
        let error = replay
            .call(&tool_call("fs.read_file", "README.md"))
            .unwrap_err();
        assert!(error.to_string().contains("asked for another"), "{error}");
    }

    #[test]
    fn the_invocation_digest_ignores_effects_and_notices_the_agent() {
        let base = tool_call("fs.read_file", "README.md");

        let mut other_effects = tool_call("fs.read_file", "README.md");
        other_effects.effects = vec!["filesystem_write".into()];
        assert_eq!(
            invocation_digest(&base),
            invocation_digest(&other_effects),
            "effects say what a call may do, not what it answers"
        );

        let mut other_agent = tool_call("fs.read_file", "README.md");
        other_agent.agent = "test.Other".into();
        assert_ne!(
            invocation_digest(&base),
            invocation_digest(&other_agent),
            "two agents hold different policies, so the same call from another is another call"
        );
    }

    #[test]
    fn a_zero_one_cassette_still_replays_and_a_lying_one_does_not() {
        let mut cassette = recorded();
        cassette.cassette_version = "0.1".into();
        let parsed = Cassette::from_json(&cassette.to_canonical_json()).unwrap();
        assert!(parsed.tool_calls.is_empty());

        cassette.tool_calls.push(ToolExchange {
            index: 0,
            node: "n0".into(),
            tool: "fs.read_file".into(),
            invocation_digest: "x".into(),
            result_type: "text".into(),
            value: Some(json!("hi")),
            error: None,
        });
        let error = Cassette::from_json(&cassette.to_canonical_json()).unwrap_err();
        assert!(error.contains("re-record"), "{error}");
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
