//! The reference interpreter for the Ingot Agent IR.
//!
//! This crate is the executable definition of what an IR document *means*. It is
//! deliberately narrow — see [RFC-0002] and [ADR-0002]. It has no context
//! management, no provider routing, no session state and no orchestration
//! features, because its job is to make the IR's semantics precise and testable,
//! not to be a good place to host an agent.
//!
//! ```no_run
//! use ingot_runtime::{run, RunOptions, ScriptedProvider, DenyAllTools, CollectingSink};
//! use std::collections::BTreeMap;
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let ir = ingot_ir::AgentIr::from_json(&std::fs::read_to_string("Brief.ir.json")?)?;
//! let mut provider = ScriptedProvider::new(vec![serde_json::json!("# Brief\n\n...")]);
//! let mut tools = DenyAllTools;
//! let mut events = CollectingSink::default();
//!
//! let report = run(
//!     &ir,
//!     &BTreeMap::new(),
//!     &mut provider,
//!     &mut tools,
//!     &mut events,
//!     RunOptions {
//!         inputs: [("topic".to_string(), serde_json::json!("compilers"))].into(),
//!         ..RunOptions::default()
//!     },
//! )?;
//!
//! println!("{}", String::from_utf8_lossy(&report.outputs["brief"].to_bytes()));
//! # Ok(())
//! # }
//! ```
//!
//! [RFC-0002]: https://github.com/mathissdupont/ingot/blob/main/rfcs/0002-runtime-execution-model.md
//! [ADR-0002]: https://github.com/mathissdupont/ingot/blob/main/docs/adr/0002-compiler-not-runtime.md

use std::collections::BTreeMap;
use std::fmt;

pub mod cassette;
pub mod catalogue;
pub mod events;
mod interp;
pub mod price;
pub mod provider;
pub mod router;
pub mod schema;
pub mod snapshot;
pub mod tools;

/// Shared by every network provider, so there is one retry rule rather than one
/// per vendor.
#[cfg(feature = "http")]
pub mod http;

#[cfg(feature = "anthropic")]
pub mod anthropic;

#[cfg(feature = "openai")]
pub mod openai;

#[cfg(feature = "google")]
pub mod google;

#[cfg(test)]
mod tests;

pub use cassette::{
    invocation_digest, load_directory, Cassette, Interaction, RecordingProvider, RecordingTools,
    ReplayProvider, ReplayToolHost, ScriptedProvider, ToolExchange, CASSETTE_VERSION,
    SUPPORTED_CASSETTE_VERSIONS,
};
pub use catalogue::{ModelConfig, ProviderConfig, ProviderKind};
pub use events::{Artifact, CollectingSink, EventSink, NullSink, RunEvent, TeeSink, VerifyOutcome};
pub use interp::{run, AgentRegistry, RunOptions};
pub use provider::{
    CompletionRequest, CompletionResponse, ModelProvider, ModelSelection, ProviderError, Usage,
};
pub use router::RoutingProvider;
pub use snapshot::{artifact_digest, Resumption, SnapshotError};
pub use tools::{
    ApprovalRequest, ConsultError, ConsultRequest, DenyAllTools, HumanChannel, Interlocutor,
    ScriptedAnswers, ScriptedApprovals, StaticToolHost, ToolError, ToolHost, ToolInvocation,
};

/// What a completed run produced.
#[derive(Debug, Clone, PartialEq)]
pub struct RunReport {
    pub agent: String,
    pub outputs: BTreeMap<String, Artifact>,
    /// Present when the run stopped at a checkpoint instead of finishing.
    ///
    /// The caller writes it down; the interpreter does not touch a filesystem.
    /// A report with this set has not produced the artifact's declared outputs
    /// and is not expected to have.
    pub stopped: Option<snapshot::Resumption>,
    /// Persistent memory as the run left it, for the caller to write back.
    ///
    /// The interpreter does not touch the filesystem, so it hands the store's
    /// new contents up rather than saving them. Empty when the artifact
    /// declares no `persistent` block.
    pub memory: BTreeMap<String, serde_json::Value>,
    pub usage: Usage,
    pub steps: u32,
    /// What the run cost, and every model it could not price.
    ///
    /// Empty when the artifact states no `cost` budget: pricing a run nobody
    /// bounded would be arithmetic for its own sake.
    pub spend: price::Spend,
}

/// Why a run stopped.
///
/// Every variant names the node it happened at, because "the agent failed" is
/// not actionable and "node n7 needed `network`, which the policy denies" is.
#[derive(Debug)]
pub enum RunError {
    /// A declared input was not supplied.
    MissingInput { name: String, ty: String },
    /// An input was supplied with the wrong type.
    InvalidInput { name: String, reason: String },
    /// An input was supplied that the agent does not declare.
    UnknownInput { name: String, expected: Vec<String> },
    /// A call needed an effect the artifact's policy does not permit.
    CapabilityDenied {
        node: String,
        effect: String,
        explicit: bool,
    },
    /// An approval gate was refused.
    ApprovalDenied { node: String, reason: String },
    /// A question could not be put to a person, or was not answered.
    ///
    /// Not a refusal like [`RunError::ApprovalDenied`]: a gate has a safe answer
    /// and a question does not, so there is nothing to carry on with.
    ConsultFailed {
        node: String,
        question: String,
        reason: String,
    },
    /// A `verify` ran its check and the property did not hold.
    ///
    /// The `verified` event carrying `failed` is emitted before this is
    /// returned, so the record says what the check found and then says the run
    /// ended. Artifacts emitted earlier in the flow stay in the record: they
    /// happened.
    VerificationFailed { node: String, verifier: String },
    /// A budget ran out.
    BudgetExceeded {
        budget: String,
        limit: String,
        node: String,
    },
    /// The model provider failed.
    Provider { node: String, source: ProviderError },
    /// A tool failed or was unavailable.
    Tool { node: String, source: ToolError },
    /// A sub-agent run failed.
    SubAgent {
        node: String,
        agent: String,
        source: Box<RunError>,
    },
    /// A sub-agent was called but its artifact was not supplied.
    AgentNotAvailable { node: String, agent: String },
    /// A response type cannot be requested from a model.
    UnsupportedResponseType {
        node: String,
        ty: String,
        reason: &'static str,
    },
    /// Working memory was read before it was written.
    StateNotSet { node: String, field: String },
    /// A value did not match its declared type.
    TypeMismatch {
        node: String,
        what: String,
        reason: String,
    },
    /// The flow finished without producing a declared output.
    OutputNotProduced { name: String },
    /// A stored persistent value did not match its declared type.
    InvalidMemory { field: String, reason: String },
    /// A snapshot could not be used to continue this artifact.
    Snapshot(snapshot::SnapshotError),
    /// Inputs were supplied alongside a resumption that already carries them.
    InputsAfterResume,
    /// `stop_at` named a checkpoint the run could not stop at.
    NotResumable {
        label: String,
        /// True when the label exists but sits inside a branch arm or a loop.
        nested: bool,
        available: Vec<String>,
    },
    /// The store carries a field this artifact does not declare.
    UnknownMemoryField {
        field: String,
        expected: Vec<String>,
    },
    /// The artifact's IR major version is not implemented.
    UnsupportedIrVersion { found: String, supported: String },
    /// The artifact is internally inconsistent.
    MalformedIr(String),
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RunError::MissingInput { name, ty } => {
                write!(f, "missing input `{name}` (expected `{ty}`)")
            }
            RunError::InvalidInput { name, reason } => {
                write!(f, "input `{name}` is invalid: {reason}")
            }
            RunError::UnknownInput { name, expected } => write!(
                f,
                "this agent has no input named `{name}`; it declares: {}",
                if expected.is_empty() { "none".to_string() } else { expected.join(", ") }
            ),
            RunError::CapabilityDenied { node, effect, explicit: true } => write!(
                f,
                "node `{node}` needs the `{effect}` effect, which the artifact's policy denies"
            ),
            RunError::CapabilityDenied { node, effect, explicit: false } => write!(
                f,
                "node `{node}` needs the `{effect}` effect, and the artifact's policy grants no \
                 rule for it (an absent rule is a denial)"
            ),
            RunError::ApprovalDenied { node, reason } => {
                write!(f, "approval was refused at node `{node}`: {reason}")
            }
            RunError::ConsultFailed {
                node,
                question,
                reason,
            } => write!(
                f,
                "the question at node `{node}` was not answered: {reason}
  asked: {question}"
            ),
            RunError::VerificationFailed { node, verifier } => write!(
                f,
                "the check `{verifier}` did not hold at node `{node}`, so the run stopped there"
            ),
            RunError::BudgetExceeded { budget, limit, node } => write!(
                f,
                "the `{budget}` budget of {limit} was exhausted at node `{node}`"
            ),
            RunError::Provider { node, source } => write!(f, "at node `{node}`: {source}"),
            RunError::Tool { node, source } => write!(f, "at node `{node}`: {source}"),
            RunError::SubAgent { node, agent, source } => {
                write!(f, "at node `{node}`, sub-agent `{agent}` failed: {source}")
            }
            RunError::AgentNotAvailable { node, agent } => write!(
                f,
                "node `{node}` calls `{agent}`, whose artifact was not supplied to this run"
            ),
            RunError::UnsupportedResponseType { node, ty, reason } => {
                write!(f, "node `{node}` asks for `{ty}`, which cannot be requested: {reason}")
            }
            RunError::StateNotSet { node, field } if node.is_empty() => {
                write!(f, "`state.{field}` was read before it was written")
            }
            RunError::StateNotSet { node, field } => {
                write!(f, "node `{node}` read `state.{field}` before it was written")
            }
            RunError::TypeMismatch { node, what, reason } => {
                write!(f, "at node `{node}`, {what}: {reason}")
            }
            RunError::OutputNotProduced { name } => {
                write!(f, "the run finished without producing the declared output `{name}`")
            }
            RunError::InvalidMemory { field, reason } => write!(
                f,
                "the stored value for `memory.{field}` does not match its declared type: {reason}"
            ),
            RunError::Snapshot(error) => write!(f, "{error}"),
            RunError::InputsAfterResume => f.write_str(
                "a resumption already carries the inputs the run started with
                   supplying different ones would let the two halves of one run disagree                  about what it was given",
            ),
            RunError::NotResumable { label, nested, available } => {
                let why = if *nested {
                    format!(
                        "the checkpoint \"{label}\" is inside a branch or a loop, so a run                          cannot stop at it
  resuming into one would mean serialising a                          continuation, which is not a file anybody could read"
                    )
                } else {
                    format!("no checkpoint is labelled \"{label}\"")
                };
                let offered = if available.is_empty() {
                    "this agent has no resumable checkpoint".to_string()
                } else {
                    format!("resumable checkpoints: {}", available.join(", "))
                };
                write!(f, "{why}
  {offered}")
            }
            RunError::UnknownMemoryField { field, expected } => write!(
                f,
                "the store carries `{field}`, which this agent does not declare
                   it declares: {}",
                if expected.is_empty() { "nothing".to_string() } else { expected.join(", ") }
            ),
            RunError::UnsupportedIrVersion { found, supported } => write!(
                f,
                "this artifact declares IR version `{found}`; this runtime implements `{supported}`. \
                 Refusing to run it rather than ignoring the parts it does not understand."
            ),
            RunError::MalformedIr(message) => write!(f, "the artifact is malformed: {message}"),
        }
    }
}

impl std::error::Error for RunError {}

impl RunError {
    /// Whether the failure is the operator's to fix (inputs, approvals,
    /// missing tools) rather than a defect in the artifact.
    pub fn is_operator_error(&self) -> bool {
        matches!(
            self,
            RunError::MissingInput { .. }
                | RunError::InvalidInput { .. }
                | RunError::UnknownInput { .. }
                | RunError::ApprovalDenied { .. }
                | RunError::ConsultFailed { .. }
                | RunError::AgentNotAvailable { .. }
                | RunError::Tool {
                    source: ToolError::NotAvailable(_),
                    ..
                }
        )
    }
}
