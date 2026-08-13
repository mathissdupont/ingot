//! A run that stopped, in a form that can be continued.
//!
//! Everything an interrupted run held that the rest of it needs: the inputs,
//! every binding in scope, working memory, the outputs produced so far, and the
//! counters. It is a JSON document a person can read, and that is a
//! requirement rather than a convenience — it is what rules out serialising a
//! continuation, which in turn is why only a top-level `checkpoint` is
//! resumable. See
//! [RFC-0018](../../../rfcs/0018-state-that-outlives-a-run.md) §4.
//!
//! # What is deliberately absent
//!
//! **Persistent memory.** It belongs to the agent, not to the interrupted run,
//! and both halves read and write the agent's store as normal.
//!
//! **A cassette.** A resumed run is given one the same way the first half was.
//! Copying the recording into the snapshot would make the two disagree the
//! moment either was re-recorded.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use ingot_ir::AgentIr;

use crate::events::Artifact;
use crate::price::Spend;
use crate::provider::Usage;

/// Format version of this document.
pub const SNAPSHOT_VERSION: &str = "0.1";

/// What kind of snapshot this is.
///
/// A memory store is the other one, and pointing `--resume` at it is a
/// plausible mistake. Naming both kinds turns "unexpected field" into a
/// sentence that explains itself.
pub const KIND: &str = "resumption";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Resumption {
    pub ingot_snapshot: String,
    pub kind: String,
    pub agent: String,
    /// SHA-256 of the artifact's canonical JSON, `sha256:…`.
    pub artifact: String,
    /// The label of the checkpoint the run stopped at.
    pub label: String,
    /// The checkpoint node itself, which the `runStopped` event names.
    pub stopped_at: String,
    /// The node to continue from — the one **after** the checkpoint, so a
    /// resumed run does not re-emit the checkpoint's event.
    pub resume_at: String,
    pub inputs: BTreeMap<String, Value>,
    pub bindings: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub state: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub outputs: BTreeMap<String, Artifact>,
    pub steps: u32,
    pub usage: Usage,
    #[serde(default)]
    pub spend: Spend,
    /// How many model calls the first half made.
    ///
    /// A cassette is matched by position, so a resumed run replaying one has to
    /// start where the first half stopped. Counted here rather than asked of
    /// the provider because it is a property of the run: a live run records the
    /// same number, and a resumed live run simply ignores it.
    #[serde(default)]
    pub model_calls: u32,
    /// How many tool calls it made, for the same reason.
    #[serde(default)]
    pub tool_calls: u32,
}

/// Why a snapshot could not be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnapshotError {
    Io(String),
    Malformed(String),
    /// A snapshot of the other kind, or of a version this build does not write.
    WrongKind {
        found: String,
    },
    UnsupportedVersion {
        found: String,
    },
    /// The artifact is not the one the run stopped in.
    DifferentArtifact {
        agent: String,
    },
    /// The node to continue from is gone.
    UnknownNode {
        node: String,
    },
}

impl std::fmt::Display for SnapshotError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SnapshotError::Io(reason) => write!(f, "{reason}"),
            SnapshotError::Malformed(reason) => write!(f, "this is not a snapshot: {reason}"),
            SnapshotError::WrongKind { found } => write!(
                f,
                "this is a `{found}` snapshot, not a `{KIND}` one\n  \
                 a memory store is passed to `--memory`, not `--resume`"
            ),
            SnapshotError::UnsupportedVersion { found } => write!(
                f,
                "this snapshot declares version `{found}`; this build writes `{SNAPSHOT_VERSION}`"
            ),
            SnapshotError::DifferentArtifact { agent } => write!(
                f,
                "`{agent}` has changed since the run stopped\n  \
                 continuing against a modified program produces a result that is neither \
                 program's, and nothing in the record would say which parts came from which\n  \
                 start the run again"
            ),
            SnapshotError::UnknownNode { node } => write!(
                f,
                "the snapshot continues from node `{node}`, which this artifact does not have"
            ),
        }
    }
}

impl std::error::Error for SnapshotError {}

/// The digest a snapshot identifies its artifact by.
pub fn artifact_digest(ir: &AgentIr) -> String {
    let mut hasher = Sha256::new();
    hasher.update(ir.to_canonical_json().as_bytes());
    format!("sha256:{:x}", hasher.finalize())
}

impl Resumption {
    /// Read one, and refuse anything that is not one.
    pub fn load(path: &Path) -> Result<Resumption, SnapshotError> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| SnapshotError::Io(format!("reading {}: {error}", path.display())))?;
        let snapshot: Resumption = serde_json::from_str(&text).map_err(|error| {
            // A memory store parses far enough to have a `kind`, so say which
            // file this is before complaining about its fields.
            match serde_json::from_str::<serde_json::Value>(&text) {
                Ok(value) if value.get("kind").and_then(Value::as_str) == Some("memory") => {
                    SnapshotError::WrongKind {
                        found: "memory".to_string(),
                    }
                }
                _ => SnapshotError::Malformed(error.to_string()),
            }
        })?;

        if snapshot.kind != KIND {
            return Err(SnapshotError::WrongKind {
                found: snapshot.kind,
            });
        }
        if snapshot.ingot_snapshot != SNAPSHOT_VERSION {
            return Err(SnapshotError::UnsupportedVersion {
                found: snapshot.ingot_snapshot,
            });
        }
        Ok(snapshot)
    }

    /// Write it, with sorted keys and a trailing newline.
    ///
    /// The same rule the IR follows: a file two identical runs produce
    /// differently is a file nobody can diff.
    pub fn save(&self, path: &Path) -> Result<(), SnapshotError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                SnapshotError::Io(format!("creating {}: {error}", parent.display()))
            })?;
        }
        let mut text = serde_json::to_string_pretty(self)
            .map_err(|error| SnapshotError::Malformed(error.to_string()))?;
        text.push('\n');
        std::fs::write(path, text)
            .map_err(|error| SnapshotError::Io(format!("writing {}: {error}", path.display())))
    }

    /// Whether this snapshot belongs to `ir`, and continues into a node it has.
    ///
    /// There is no override. See
    /// [Runtime 0.5 §2.4](../../../specs/runtime/v0.5.md).
    pub fn check(&self, ir: &AgentIr) -> Result<(), SnapshotError> {
        if self.artifact != artifact_digest(ir) {
            return Err(SnapshotError::DifferentArtifact {
                agent: self.agent.clone(),
            });
        }
        if ir.node(&self.resume_at).is_none() {
            return Err(SnapshotError::UnknownNode {
                node: self.resume_at.clone(),
            });
        }
        Ok(())
    }
}

/// Every label a run could be stopped at, in flow order.
///
/// Used to refuse `--stop-at` before the run starts, and to say what was
/// available when the label does not match.
pub fn resumable_labels(ir: &AgentIr) -> Vec<String> {
    ir.nodes
        .iter()
        .filter(|node| node.resumable)
        .filter_map(|node| node.label.clone())
        .collect()
}

/// Every checkpoint label, resumable or not.
///
/// The difference between this and [`resumable_labels`] is what turns "no such
/// checkpoint" into "that checkpoint is inside a loop".
pub fn all_checkpoint_labels(ir: &AgentIr) -> Vec<String> {
    ir.nodes
        .iter()
        .filter(|node| node.kind == ingot_ir::NodeKind::Checkpoint)
        .filter_map(|node| node.label.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ingot_ir::{Budget, ModelRequirement, Node, NodeKind, Requirements, IR_VERSION};

    fn artifact() -> AgentIr {
        let mut checkpoint = Node::new("n0", NodeKind::Checkpoint);
        checkpoint.label = Some("half-way".to_string());
        checkpoint.resumable = true;
        checkpoint.next = Some("n1".to_string());

        let mut nested = Node::new("n1", NodeKind::Checkpoint);
        nested.label = Some("inside".to_string());

        AgentIr {
            ir_version: IR_VERSION.to_string(),
            language: "0.2".to_string(),
            agent: "test.Stops".to_string(),
            doc: None,
            inputs: BTreeMap::new(),
            outputs: BTreeMap::new(),
            types: BTreeMap::new(),
            requirements: Requirements {
                model: ModelRequirement::Unspecified,
            },
            tools: Vec::new(),
            state: BTreeMap::new(),
            persistent: BTreeMap::new(),
            budget: Budget::default(),
            policy: BTreeMap::new(),
            effects: Vec::new(),
            entry: Some("n0".to_string()),
            nodes: vec![checkpoint, nested],
        }
    }

    fn snapshot(ir: &AgentIr) -> Resumption {
        Resumption {
            ingot_snapshot: SNAPSHOT_VERSION.to_string(),
            kind: KIND.to_string(),
            agent: ir.agent.clone(),
            artifact: artifact_digest(ir),
            label: "half-way".to_string(),
            stopped_at: "n0".to_string(),
            resume_at: "n1".to_string(),
            inputs: BTreeMap::new(),
            bindings: BTreeMap::new(),
            state: BTreeMap::new(),
            outputs: BTreeMap::new(),
            steps: 3,
            usage: Usage::default(),
            spend: Spend::default(),
            model_calls: 0,
            tool_calls: 0,
        }
    }

    #[test]
    fn a_snapshot_round_trips_through_json() {
        let ir = artifact();
        let original = snapshot(&ir);
        let text = serde_json::to_string(&original).unwrap();
        let parsed: Resumption = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn a_snapshot_belongs_to_exactly_one_artifact() {
        let ir = artifact();
        let snapshot = snapshot(&ir);
        assert!(snapshot.check(&ir).is_ok());

        // One node id renamed is a different program.
        let mut edited = ir.clone();
        edited.budget.steps = Some(9);
        let error = snapshot.check(&edited).unwrap_err();
        assert!(
            matches!(error, SnapshotError::DifferentArtifact { .. }),
            "{error}"
        );
        // The message says to start again, because there is no override.
        assert!(error.to_string().contains("start the run again"));
    }

    #[test]
    fn only_a_top_level_checkpoint_is_offered() {
        let ir = artifact();
        assert_eq!(resumable_labels(&ir), vec!["half-way".to_string()]);
        assert_eq!(
            all_checkpoint_labels(&ir),
            vec!["half-way".to_string(), "inside".to_string()]
        );
    }

    #[test]
    fn a_memory_store_pointed_at_resume_says_which_file_it_is() {
        let dir = std::env::temp_dir().join(format!("ingot-snapshot-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("store.json");
        std::fs::write(
            &path,
            r#"{"ingotSnapshot":"0.1","kind":"memory","agent":"a","shape":{},"fields":{}}"#,
        )
        .unwrap();
        let error = Resumption::load(&path).unwrap_err();
        assert!(
            matches!(error, SnapshotError::WrongKind { ref found } if found == "memory"),
            "{error}"
        );
        assert!(error.to_string().contains("--memory"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
