//! The normalised event stream.
//!
//! Events carry no timestamps and no wall-clock durations. That is deliberate:
//! replaying the same cassette produces the same event sequence byte for byte,
//! which is what makes an event stream assertable in a test rather than merely
//! inspectable by a human.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::provider::Usage;

/// What a `verify` node actually did.
///
/// Three states rather than a boolean, because "the check passed" and "there was
/// no check" are different facts and a boolean can only tell you one of them.
/// Agent IR records a verifier's name and signature and carries no way to
/// execute one, so a backend that cannot perform a check says so here instead of
/// reporting a pass nothing earned.
///
/// See [Runtime 0.2 §1](../../../specs/runtime/v0.2.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum VerifyOutcome {
    /// The backend has no implementation for this verifier. Not a failure: the
    /// property is simply unchecked, and the run says so.
    NotPerformed,
    Passed,
    Failed,
}

impl VerifyOutcome {
    pub fn describe(self) -> &'static str {
        match self {
            VerifyOutcome::NotPerformed => "not performed",
            VerifyOutcome::Passed => "passed",
            VerifyOutcome::Failed => "FAILED",
        }
    }

    /// Whether this outcome is a check that ran and said no.
    ///
    /// Deliberately not `!passed`: a check that never ran has not failed, and
    /// treating it as a failure would be the mirror of the bug this type exists
    /// to fix.
    pub fn is_failure(self) -> bool {
        matches!(self, VerifyOutcome::Failed)
    }
}

/// One line of the run record.
///
/// `rename_all_fields` is load-bearing and easy to lose: on an enum,
/// `rename_all` renames the *variants*, not their fields. Without the second
/// attribute `response_type` serialised as `response_type` while every
/// specification example and the second backend said `responseType` — a
/// divergence no single-implementation test could see, and the first thing the
/// conformance suite found.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "event",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum RunEvent {
    RunStarted {
        agent: String,
        provider: String,
    },
    NodeStarted {
        node: String,
        kind: String,
    },
    ModelCall {
        node: String,
        model: String,
        response_type: String,
        usage: Usage,
    },
    ToolCall {
        node: String,
        tool: String,
        effects: Vec<String>,
    },
    AgentCall {
        node: String,
        agent: String,
    },
    /// The compiler inserted an approval gate; this is the runtime asking.
    ApprovalRequested {
        node: String,
        effects: Vec<String>,
        reason: String,
    },
    ApprovalDecided {
        node: String,
        allowed: bool,
    },
    StateWritten {
        node: String,
        field: String,
    },
    Verified {
        node: String,
        verifier: String,
        outcome: VerifyOutcome,
    },
    Checkpoint {
        node: String,
        label: String,
    },
    BranchTaken {
        node: String,
        /// `then` or `else`.
        arm: String,
    },
    LoopIteration {
        node: String,
        iteration: u32,
    },
    MapIteration {
        node: String,
        index: usize,
        total: usize,
    },
    Emitted {
        node: String,
        output: String,
    },
    RunFinished {
        steps: u32,
        usage: Usage,
    },
    RunFailed {
        reason: String,
    },
    /// The run stopped at a resumable checkpoint and can be continued.
    ///
    /// Distinct from `runFinished` because without it a stopped run and a run
    /// that finished having produced nothing look identical in a record, and
    /// the check that every declared output was emitted would have to be
    /// skipped on a guess. This is that guess made explicit: it is the only
    /// thing that suppresses the check, and a reader can see it was suppressed.
    RunStopped {
        node: String,
        label: String,
    },
}

impl RunEvent {
    /// One line, for a terminal.
    pub fn to_line(&self) -> String {
        match self {
            RunEvent::RunStarted { agent, provider } => {
                format!("run {agent} (provider: {provider})")
            }
            RunEvent::NodeStarted { node, kind } => format!("  {node}  {kind}"),
            RunEvent::ModelCall {
                model,
                response_type,
                usage,
                ..
            } => format!(
                "        model {model} -> {response_type} ({} in, {} out)",
                usage.input_tokens, usage.output_tokens
            ),
            RunEvent::ToolCall { tool, effects, .. } => {
                format!("        tool {tool} [{}]", effects.join(", "))
            }
            RunEvent::AgentCall { agent, .. } => format!("        agent {agent}"),
            RunEvent::ApprovalRequested {
                effects, reason, ..
            } => {
                format!(
                    "        approval needed for [{}]: {reason}",
                    effects.join(", ")
                )
            }
            RunEvent::ApprovalDecided { allowed, .. } => {
                format!(
                    "        approval {}",
                    if *allowed { "granted" } else { "denied" }
                )
            }
            RunEvent::StateWritten { field, .. } => format!("        state.{field} written"),
            RunEvent::Verified {
                verifier, outcome, ..
            } => format!("        verify {verifier}: {}", outcome.describe()),
            RunEvent::Checkpoint { label, .. } => format!("        checkpoint \"{label}\""),
            RunEvent::BranchTaken { arm, .. } => format!("        branch: {arm}"),
            RunEvent::LoopIteration { iteration, .. } => format!("        iteration {iteration}"),
            RunEvent::MapIteration { index, total, .. } => {
                format!("        element {}/{total}", index + 1)
            }
            RunEvent::Emitted { output, .. } => format!("        emit {output}"),
            RunEvent::RunFinished { steps, usage } => {
                format!("done: {steps} step(s), {} token(s)", usage.total())
            }
            RunEvent::RunFailed { reason } => format!("failed: {reason}"),
            RunEvent::RunStopped { label, .. } => {
                format!("stopped at \"{label}\"; resume to continue")
            }
        }
    }

    /// One JSON object, for piping into another tool.
    pub fn to_json_line(&self) -> String {
        serde_json::to_string(self).expect("events are always serializable")
    }
}

/// Where a run's observable output goes.
///
/// Two channels, and the difference between them is the point.
///
/// [`emit`](EventSink::emit) carries the **event stream**: the record of what
/// the run did. It is ordered, it is timestamp-free, and replaying a cassette
/// reproduces it byte for byte, which is what lets a test assert on it.
///
/// [`delta`](EventSink::delta) and [`settled`](EventSink::settled) carry the
/// **live channel**: text as a model produces it. None of it is a record of
/// anything. How an answer arrived over the wire is a property of the
/// connection, not of the run, so a delta is never an event, never recorded in
/// a cassette, and never asserted on. Both default to discarding, so a sink
/// that only wants the record gets exactly the record.
pub trait EventSink {
    fn emit(&mut self, event: RunEvent);

    /// A fragment of a model's answer, as it arrives.
    ///
    /// Called only on a live call against a provider that streams. Fragments
    /// for one node arrive in order and concatenate to the answer's text; a
    /// watcher that shows them is showing something that may yet be thrown
    /// away, which is what [`settled`](EventSink::settled) is for.
    fn delta(&mut self, node: &str, text: &str) {
        let _ = (node, text);
    }

    /// No more deltas for this node, and whether the text became the answer.
    ///
    /// `kept` is false when the response was discarded — the answer was cut
    /// off, or it did not match its declared type — so a watcher can strike
    /// what it showed instead of leaving a half-finished answer on screen
    /// looking like a result. Called only after at least one delta.
    fn settled(&mut self, node: &str, kept: bool) {
        let _ = (node, kept);
    }
}

/// Keeps every event, for tests and for the run report.
#[derive(Debug, Default)]
pub struct CollectingSink {
    pub events: Vec<RunEvent>,
}

impl EventSink for CollectingSink {
    fn emit(&mut self, event: RunEvent) {
        self.events.push(event);
    }
}

/// Discards events.
pub struct NullSink;

impl EventSink for NullSink {
    fn emit(&mut self, _event: RunEvent) {}
}

/// Collects events and also hands each to a callback, for live output.
///
/// Events only. Deltas are discarded, because a callback that took both would
/// have to decide which stream it was being handed on every call. A watcher
/// that wants the live text implements [`EventSink`] directly and overrides
/// [`EventSink::delta`].
pub struct TeeSink<F: FnMut(&RunEvent)> {
    pub events: Vec<RunEvent>,
    callback: F,
}

impl<F: FnMut(&RunEvent)> TeeSink<F> {
    pub fn new(callback: F) -> TeeSink<F> {
        TeeSink {
            events: Vec::new(),
            callback,
        }
    }
}

impl<F: FnMut(&RunEvent)> EventSink for TeeSink<F> {
    fn emit(&mut self, event: RunEvent) {
        (self.callback)(&event);
        self.events.push(event);
    }
}

/// A value produced by the run, ready to be written out.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Artifact {
    pub name: String,
    /// The artifact content type, e.g. `markdown`.
    pub content_type: String,
    pub value: Value,
}

impl Artifact {
    /// The bytes to write to disk.
    ///
    /// Prose types are written as-is; anything else as canonical JSON, because
    /// writing a JSON-quoted markdown document to a `.md` file would be useless.
    pub fn to_bytes(&self) -> Vec<u8> {
        match (&self.value, self.content_type.as_str()) {
            (Value::String(text), "markdown" | "text") => text.clone().into_bytes(),
            (value, _) => {
                let mut json = serde_json::to_string_pretty(value)
                    .expect("artifact values are always serializable");
                json.push('\n');
                json.into_bytes()
            }
        }
    }

    /// Conventional file extension for this content type.
    pub fn extension(&self) -> &'static str {
        match self.content_type.as_str() {
            "markdown" => "md",
            "text" => "txt",
            _ => "json",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn events_round_trip_through_json() {
        let event = RunEvent::ModelCall {
            node: "n0".into(),
            model: "test".into(),
            response_type: "markdown".into(),
            usage: Usage {
                input_tokens: 1,
                output_tokens: 2,
                cache_read_tokens: 0,
            },
        };
        let parsed: RunEvent = serde_json::from_str(&event.to_json_line()).unwrap();
        assert_eq!(parsed, event);
    }

    #[test]
    fn markdown_artifacts_are_written_as_prose() {
        let artifact = Artifact {
            name: "report".into(),
            content_type: "markdown".into(),
            value: json!("# Title\n\nBody"),
        };
        assert_eq!(artifact.to_bytes(), b"# Title\n\nBody");
        assert_eq!(artifact.extension(), "md");
    }

    #[test]
    fn structured_artifacts_are_written_as_json() {
        let artifact = Artifact {
            name: "data".into(),
            content_type: "json".into(),
            value: json!({"a": 1}),
        };
        let text = String::from_utf8(artifact.to_bytes()).unwrap();
        assert!(text.starts_with('{'));
        assert!(text.ends_with("}\n"));
        assert_eq!(artifact.extension(), "json");
    }

    #[test]
    fn the_collecting_sink_preserves_order() {
        let mut sink = CollectingSink::default();
        sink.emit(RunEvent::RunStarted {
            agent: "a".into(),
            provider: "p".into(),
        });
        sink.emit(RunEvent::RunFinished {
            steps: 1,
            usage: Usage::default(),
        });
        assert_eq!(sink.events.len(), 2);
        assert!(matches!(sink.events[0], RunEvent::RunStarted { .. }));
    }

    #[test]
    fn a_delta_is_not_an_event() {
        // The property the whole design rests on: what a watcher sees live
        // leaves no trace in the stream a replay has to reproduce.
        let mut sink = CollectingSink::default();
        sink.delta("n0", "half an ans");
        sink.delta("n0", "wer");
        sink.settled("n0", true);
        assert!(
            sink.events.is_empty(),
            "deltas leaked into the event stream: {:?}",
            sink.events
        );
    }
}
