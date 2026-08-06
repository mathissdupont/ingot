//! The normalised event stream.
//!
//! Events carry no timestamps and no wall-clock durations. That is deliberate:
//! replaying the same cassette produces the same event sequence byte for byte,
//! which is what makes an event stream assertable in a test rather than merely
//! inspectable by a human.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::provider::Usage;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "camelCase")]
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
        passed: bool,
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
                verifier, passed, ..
            } => {
                format!(
                    "        verify {verifier}: {}",
                    if *passed { "passed" } else { "FAILED" }
                )
            }
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
        }
    }

    /// One JSON object, for piping into another tool.
    pub fn to_json_line(&self) -> String {
        serde_json::to_string(self).expect("events are always serializable")
    }
}

/// Where events go.
pub trait EventSink {
    fn emit(&mut self, event: RunEvent);
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
}
