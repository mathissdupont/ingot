//! The persistent memory store: an agent's `memory.` fields, on disk.
//!
//! One JSON document per agent, holding the values **and the declaration they
//! were written under**. Keeping the declaration is what makes a changed
//! artifact reportable rather than merely detectable: a digest can say no, and
//! this can say which field.
//!
//! # Nothing here is locked
//!
//! One run owns a store for its duration. Two runs sharing one interleave, and
//! the second to finish wins outright, because each writes the whole document.
//! There is no lock and no detection.
//!
//! That is stated rather than fixed. The default path is per-agent and under
//! the build directory, which makes the collision hard to reach by accident,
//! and the fix is not small — a lock file with all the staleness questions that
//! brings, or per-field merge with a conflict model. See
//! [RFC-0018](../../../rfcs/0018-state-that-outlives-a-run.md) §5 and
//! [GAP-035](../../../docs/gaps.md#gap-035).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use ingot_ir::AgentIr;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Where stores live inside a project's build directory.
///
/// Beside the run records, and for the same reasons: it is output, version
/// control already ignores it, and deleting the build directory is expected to
/// lose it.
pub const MEMORY_DIR: &str = "memory";

/// Refuse a store larger than this.
///
/// Not an algorithmic limit. A store this size means an agent is accumulating
/// without bound into a document that is rewritten in full every run, and being
/// told at the point that first happens beats discovering it from a slow run
/// months later.
const MAX_BYTES: u64 = 4 * 1024 * 1024;

const SNAPSHOT_VERSION: &str = "0.1";
const KIND: &str = "memory";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MemoryStore {
    pub ingot_snapshot: String,
    pub kind: String,
    pub agent: String,
    /// The declaration this store was written under: field name to type.
    pub shape: BTreeMap<String, String>,
    pub fields: BTreeMap<String, Value>,
}

/// What the caller asked for on the command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryMode {
    /// Open the agent's store, creating it on first write.
    Open,
    /// Run from the declared initial values and discard every write.
    Disabled,
    /// Open it, and accept losing what no longer fits the declaration.
    Migrate,
}

/// An opened store, and what to say about it.
pub struct OpenedMemory {
    /// Where it will be written, or `None` when writes are discarded.
    pub path: Option<PathBuf>,
    /// The values the run starts from. Empty means every field starts from its
    /// declared initial value, which is a correct first run.
    pub fields: BTreeMap<String, Value>,
    /// One line for the run log. A store is not a thing that should be
    /// discoverable only by finding the file. Suppressed by `--events quiet`,
    /// like every other progress line.
    pub note: String,
    /// What a migration dropped, when one happened.
    ///
    /// Separate from `note` and never suppressed: losing a stored value is a
    /// result, not progress, and `--events quiet` asks for less chatter rather
    /// than for silence about data going away.
    pub dropped: Option<String>,
}

/// The declaration an artifact makes, in the form a store records.
pub fn shape_of(ir: &AgentIr) -> BTreeMap<String, String> {
    ir.persistent
        .iter()
        .map(|(name, field)| (name.clone(), field.ty.clone()))
        .collect()
}

/// The store this agent uses when the operator names no other.
pub fn default_path(out_dir: &Path, agent: &str) -> PathBuf {
    out_dir
        .join(MEMORY_DIR)
        .join(format!("{}.json", sanitise(agent)))
}

/// An agent's qualified name as one path segment.
///
/// `research.Report` is a filename on every platform; a name with a separator
/// in it would silently become a directory, so every character that is not
/// plainly safe becomes `_`.
fn sanitise(agent: &str) -> String {
    agent
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '.' || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

/// Open the store for a run, or explain why it cannot be used.
///
/// An artifact that declares no `persistent` block opens nothing: this affects
/// exactly the programs that asked for it.
pub fn open(
    ir: &AgentIr,
    path: Option<&Path>,
    out_dir: Option<&Path>,
    mode: MemoryMode,
) -> Result<OpenedMemory> {
    if ir.persistent.is_empty() {
        return Ok(OpenedMemory {
            path: None,
            fields: BTreeMap::new(),
            note: String::new(),
            dropped: None,
        });
    }

    if mode == MemoryMode::Disabled {
        return Ok(OpenedMemory {
            path: None,
            fields: BTreeMap::new(),
            note: "memory: starting from the declared values; writes are discarded (--no-memory)"
                .to_string(),
            dropped: None,
        });
    }

    let path = match (path, out_dir) {
        (Some(path), _) => path.to_path_buf(),
        (None, Some(out_dir)) => default_path(out_dir, &ir.agent),
        // Nowhere to put it. Refusing beats running an agent that declared
        // memory and silently keeping none of it.
        (None, None) => bail!(
            "`{}` declares persistent memory, but this run has no build directory to keep a \
             store in\n  pass `--memory <FILE>` to choose one, or `--no-memory` to run \
             from the declared values and discard what is written",
            ir.agent
        ),
    };

    if !path.exists() {
        return Ok(OpenedMemory {
            note: format!("memory: {} (new)", path.display()),
            path: Some(path),
            fields: BTreeMap::new(),
            dropped: None,
        });
    }

    let stored = read(&path)?;
    let declared = shape_of(ir);
    let differences = diff(&stored.shape, &declared);

    if differences.is_empty() {
        return Ok(OpenedMemory {
            note: format!(
                "memory: {} ({} field(s))",
                path.display(),
                stored.fields.len()
            ),
            path: Some(path),
            fields: stored.fields,
            dropped: None,
        });
    }

    if mode != MemoryMode::Migrate {
        bail!(
            "the memory store was written for a different declaration\n  --> {}\n{}\n  \
             help: `--migrate-memory` keeps what still matches and drops the rest\n  \
             help: `--no-memory` ignores the store for this run without changing it",
            path.display(),
            describe(&differences, &stored.fields)
        );
    }

    // Keep only fields whose name and type both still match. A migration that
    // reinterprets a stored value as a new type is how a store corrupts itself;
    // this one would rather lose the value and say so.
    let stored_fields = stored.fields.clone();
    let kept: BTreeMap<String, Value> = stored
        .fields
        .into_iter()
        .filter(|(name, _)| declared.get(name) == stored.shape.get(name))
        .collect();

    Ok(OpenedMemory {
        note: format!("memory: {} (migrated)", path.display()),
        dropped: Some(format!(
            "warning: migrated {}, keeping {} field(s)\n{}",
            path.display(),
            kept.len(),
            describe(&differences, &stored_fields)
        )),
        path: Some(path),
        fields: kept,
    })
}

/// Write the store back, under the declaration this run used.
pub fn save(path: &Path, ir: &AgentIr, fields: &BTreeMap<String, Value>) -> Result<()> {
    let store = MemoryStore {
        ingot_snapshot: SNAPSHOT_VERSION.to_string(),
        kind: KIND.to_string(),
        agent: ir.agent.clone(),
        shape: shape_of(ir),
        fields: fields.clone(),
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    // Sorted keys and a trailing newline: a file two identical runs produce
    // differently is a file nobody can diff. `BTreeMap` gives the ordering.
    let mut text = serde_json::to_string_pretty(&store).context("encoding the memory store")?;
    text.push('\n');
    std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))
}

fn read(path: &Path) -> Result<MemoryStore> {
    let size = std::fs::metadata(path)
        .with_context(|| format!("reading {}", path.display()))?
        .len();
    if size > MAX_BYTES {
        bail!(
            "the memory store at {} is {} bytes, over the {MAX_BYTES}-byte ceiling\n  \
             a store this size is an agent accumulating without bound into a document \
             that is rewritten in full every run",
            path.display(),
            size
        );
    }

    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let store: MemoryStore = serde_json::from_str(&text)
        .with_context(|| format!("{} is not a memory store", path.display()))?;

    if store.kind != KIND {
        bail!(
            "{} is a `{}` snapshot, not a `{KIND}` one\n  \
             a resumption snapshot is passed to `--resume`, not `--memory`",
            path.display(),
            store.kind
        );
    }
    if store.ingot_snapshot != SNAPSHOT_VERSION {
        bail!(
            "{} declares snapshot version `{}`; this build writes `{SNAPSHOT_VERSION}`",
            path.display(),
            store.ingot_snapshot
        );
    }
    Ok(store)
}

/// How a stored declaration differs from the current one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Difference {
    /// Declared now, absent from the store.
    Added { field: String, ty: String },
    /// In the store, no longer declared.
    Removed { field: String, ty: String },
    /// Declared under a different type.
    Retyped {
        field: String,
        stored: String,
        declared: String,
    },
}

fn diff(stored: &BTreeMap<String, String>, declared: &BTreeMap<String, String>) -> Vec<Difference> {
    let mut out = Vec::new();
    for (field, ty) in declared {
        match stored.get(field) {
            None => out.push(Difference::Added {
                field: field.clone(),
                ty: ty.clone(),
            }),
            Some(was) if was != ty => out.push(Difference::Retyped {
                field: field.clone(),
                stored: was.clone(),
                declared: ty.clone(),
            }),
            Some(_) => {}
        }
    }
    for (field, ty) in stored {
        if !declared.contains_key(field) {
            out.push(Difference::Removed {
                field: field.clone(),
                ty: ty.clone(),
            });
        }
    }
    out
}

/// The differences, one per line, with what each costs.
fn describe(differences: &[Difference], stored: &BTreeMap<String, Value>) -> String {
    let mut lines = Vec::new();
    for difference in differences {
        lines.push(match difference {
            Difference::Added { field, ty } => {
                format!("  added:    {field}: {ty} (starts from the declared value)")
            }
            Difference::Removed { field, ty } => {
                let held = if stored.contains_key(field) {
                    " and would be dropped"
                } else {
                    ""
                };
                format!("  removed:  {field}: {ty} (stored{held})")
            }
            Difference::Retyped {
                field,
                stored: was,
                declared,
            } => format!("  retyped:  {field}: {declared} (stored as {was})"),
        });
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn shape(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(name, ty)| (name.to_string(), ty.to_string()))
            .collect()
    }

    #[test]
    fn an_unchanged_declaration_has_no_differences() {
        let declared = shape(&[("seen", "string[]")]);
        assert!(diff(&declared.clone(), &declared).is_empty());
    }

    #[test]
    fn each_kind_of_change_is_named_separately() {
        let stored = shape(&[("seen", "string[]"), ("gone", "int")]);
        let declared = shape(&[("seen", "int[]"), ("fresh", "bool")]);
        let differences = diff(&stored, &declared);
        assert!(differences.contains(&Difference::Retyped {
            field: "seen".into(),
            stored: "string[]".into(),
            declared: "int[]".into(),
        }));
        assert!(differences.contains(&Difference::Added {
            field: "fresh".into(),
            ty: "bool".into(),
        }));
        assert!(differences.contains(&Difference::Removed {
            field: "gone".into(),
            ty: "int".into(),
        }));
    }

    #[test]
    fn the_message_names_the_field_rather_than_only_refusing() {
        let stored = shape(&[("seen", "string[]")]);
        let declared = shape(&[("seen", "int[]")]);
        let text = describe(&diff(&stored, &declared), &BTreeMap::new());
        assert!(text.contains("seen"), "{text}");
        assert!(text.contains("string[]"), "{text}");
        assert!(text.contains("int[]"), "{text}");
    }

    #[test]
    fn a_removal_says_whether_a_value_would_be_lost() {
        let stored = shape(&[("gone", "int")]);
        let differences = diff(&stored, &BTreeMap::new());
        let held: BTreeMap<String, Value> = [("gone".to_string(), json!(3))].into_iter().collect();
        assert!(describe(&differences, &held).contains("dropped"));
        assert!(!describe(&differences, &BTreeMap::new()).contains("dropped"));
    }

    #[test]
    fn an_agent_name_becomes_one_path_segment() {
        assert_eq!(sanitise("research.Report"), "research.Report");
        assert_eq!(sanitise("a/b"), "a_b");
        assert_eq!(sanitise("a\\b"), "a_b");
    }
}
