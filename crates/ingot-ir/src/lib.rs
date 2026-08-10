//! Agent IR: the canonical, target-neutral representation of a compiled agent.
//!
//! The IR is not meant to be written by hand. It is the contract between the
//! compiler front end and every backend, and the thing a portability report is
//! computed against.
//!
//! Three properties are load-bearing:
//!
//! * **Canonical.** [`AgentIr::to_canonical_json`] is the only blessed encoding:
//!   two-space indentation, sorted maps, a trailing newline. The same source and
//!   the same compiler always produce byte-identical output, which is what makes
//!   a reproducible artifact digest possible later.
//! * **Flat.** Control flow lives in one `nodes` array with explicit `next`
//!   pointers. Nested regions (branch arms, loop and map bodies) are referenced
//!   by the id of their first node and terminate with `next: null`, so a backend
//!   can walk the graph without recursive pattern matching.
//! * **Explicit.** Effects, policy decisions, budgets and approval checkpoints
//!   are data in the IR, not conventions a backend has to rediscover.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub mod node;

pub use node::{Node, NodeKind, RefScope, SourceSpan, TemplatePart, Value};

/// Version of the IR schema this crate emits and understands.
pub const IR_VERSION: &str = "0.2";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentIr {
    /// Schema version of this document.
    pub ir_version: String,
    /// Language version of the source it was compiled from.
    pub language: String,
    /// Fully qualified agent name, e.g. `heptapus.research.ResearchAgent`.
    pub agent: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
    /// Input name to type, e.g. `{"topic": "string"}`.
    pub inputs: BTreeMap<String, String>,
    /// Output name to artifact type, e.g. `{"report": "artifact<markdown>"}`.
    pub outputs: BTreeMap<String, String>,
    /// Record types referenced by this program.
    #[serde(skip_serializing_if = "BTreeMap::is_empty", default)]
    pub types: BTreeMap<String, RecordType>,
    pub requirements: Requirements,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub tools: Vec<ToolBinding>,
    /// Working memory field name to type.
    #[serde(skip_serializing_if = "BTreeMap::is_empty", default)]
    pub state: BTreeMap<String, String>,
    pub budget: Budget,
    /// Policy decision per subject. Subjects absent here are denied.
    pub policy: BTreeMap<String, PolicyRule>,
    /// Union of every effect the flow can trigger, sorted.
    pub effects: Vec<String>,
    /// Id of the first node, or `null` for an empty flow.
    pub entry: Option<String>,
    pub nodes: Vec<Node>,
}

impl AgentIr {
    /// The canonical encoding. Byte-for-byte stable for a given input.
    pub fn to_canonical_json(&self) -> String {
        let mut json =
            serde_json::to_string_pretty(self).expect("the IR model is always serializable");
        json.push('\n');
        json
    }

    pub fn from_json(text: &str) -> Result<AgentIr, serde_json::Error> {
        serde_json::from_str(text)
    }

    pub fn node(&self, id: &str) -> Option<&Node> {
        self.nodes.iter().find(|node| node.id == id)
    }

    /// Walk the linear chain starting at `entry`, not descending into regions.
    pub fn main_path(&self) -> Vec<&Node> {
        let mut path = Vec::new();
        let mut current = self.entry.clone();
        while let Some(id) = current {
            let Some(node) = self.node(&id) else { break };
            path.push(node);
            current = node.next.clone();
        }
        path
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordType {
    pub fields: Vec<FieldType>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldType {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Requirements {
    pub model: ModelRequirement,
}

/// How the runtime is expected to choose a model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "camelCase")]
pub enum ModelRequirement {
    /// Any model satisfying these capabilities is acceptable.
    Capabilities {
        capabilities: Vec<String>,
        #[serde(rename = "contextTokens", skip_serializing_if = "Option::is_none")]
        context_tokens: Option<ContextTokens>,
    },
    /// A pinned provider/model reference.
    Exact { reference: String },
    /// The backend's default applies.
    Unspecified,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextTokens {
    pub min: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolBinding {
    /// Transport-qualified reference, e.g. `mcp:web.search`.
    #[serde(rename = "ref")]
    pub reference: String,
    pub name: String,
    pub transport: String,
    pub effects: Vec<String>,
    /// Where this tool goes, per effect, as the source declared it.
    ///
    /// Sorted and deduplicated by the compiler, and omitted entirely when the
    /// tool declared no reach — so an artifact that uses none encodes exactly
    /// as it did before [RFC-0014], and no package digest moves.
    ///
    /// Every key here is also in `effects`. Kept separate rather than folded
    /// into that list because `effects` is a set a backend already walks to
    /// check the policy, and an entry that is sometimes a string and sometimes
    /// an object is the shape that makes a second implementation guess.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub scopes: BTreeMap<String, Vec<String>>,
    pub signature: ToolSignature,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolSignature {
    pub params: Vec<FieldType>,
    pub result: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Budget {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub steps: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<Cost>,
}

/// A monetary limit.
///
/// The amount is a decimal **string**: binary floats round-trip differently
/// across platforms and serializers, and a reproducible artifact cannot depend
/// on that.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cost {
    pub amount: String,
    pub currency: String,
}

impl Cost {
    pub fn new(amount: f64, currency: impl Into<String>) -> Cost {
        Cost {
            amount: format_amount(amount),
            currency: currency.into(),
        }
    }
}

/// Format a monetary amount as a stable decimal string.
///
/// Uses six fractional digits and trims trailing zeros, so `5.0` becomes `"5"`
/// and `0.25` stays `"0.25"` on every platform.
pub fn format_amount(amount: f64) -> String {
    let rendered = format!("{amount:.6}");
    let trimmed = rendered.trim_end_matches('0').trim_end_matches('.');
    if trimmed.is_empty() || trimmed == "-" {
        "0".to_string()
    } else {
        trimmed.to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PolicyRule {
    pub decision: Decision,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub values: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub qualifier: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Decision {
    Allow,
    Deny,
    RequireApproval,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> AgentIr {
        AgentIr {
            ir_version: IR_VERSION.to_string(),
            language: "0.1".to_string(),
            agent: "heptapus.research.ResearchAgent".to_string(),
            doc: None,
            inputs: [("topic".to_string(), "string".to_string())]
                .into_iter()
                .collect(),
            outputs: [("report".to_string(), "artifact<markdown>".to_string())]
                .into_iter()
                .collect(),
            types: BTreeMap::new(),
            requirements: Requirements {
                model: ModelRequirement::Capabilities {
                    capabilities: vec!["tool_calling".to_string()],
                    context_tokens: Some(ContextTokens { min: 131_072 }),
                },
            },
            tools: Vec::new(),
            state: BTreeMap::new(),
            budget: Budget {
                steps: Some(60),
                tokens: None,
                cost: Some(Cost::new(5.0, "usd")),
            },
            policy: [(
                "network".to_string(),
                PolicyRule {
                    decision: Decision::Allow,
                    values: vec!["arxiv.org".to_string()],
                    qualifier: None,
                },
            )]
            .into_iter()
            .collect(),
            effects: vec!["model_access".to_string(), "network".to_string()],
            entry: None,
            nodes: Vec::new(),
        }
    }

    #[test]
    fn every_key_is_camel_case() {
        let json = sample().to_canonical_json();
        for line in json.lines() {
            let Some(key) = line
                .trim()
                .strip_prefix('"')
                .and_then(|rest| rest.split('"').next())
            else {
                continue;
            };
            if !line.trim_start().starts_with(&format!("\"{key}\":")) {
                continue;
            }
            assert!(
                !key.contains('_'),
                "`{key}` breaks the camelCase convention of the IR schema"
            );
        }
    }

    #[test]
    fn canonical_json_round_trips() {
        let ir = sample();
        let json = ir.to_canonical_json();
        let parsed = AgentIr::from_json(&json).expect("canonical JSON must parse");
        assert_eq!(parsed, ir);
    }

    #[test]
    fn canonical_json_is_stable_across_runs() {
        assert_eq!(sample().to_canonical_json(), sample().to_canonical_json());
    }

    #[test]
    fn canonical_json_ends_with_a_newline() {
        assert!(sample().to_canonical_json().ends_with("}\n"));
    }

    #[test]
    fn map_keys_are_emitted_in_sorted_order() {
        let mut ir = sample();
        ir.inputs.insert("zeta".to_string(), "int".to_string());
        ir.inputs.insert("alpha".to_string(), "int".to_string());
        let json = ir.to_canonical_json();
        let alpha = json.find("\"alpha\"").unwrap();
        let topic = json.find("\"topic\"").unwrap();
        let zeta = json.find("\"zeta\"").unwrap();
        assert!(alpha < topic && topic < zeta);
    }

    #[test]
    fn cost_amounts_are_platform_stable_decimal_strings() {
        assert_eq!(format_amount(5.0), "5");
        assert_eq!(format_amount(0.25), "0.25");
        assert_eq!(format_amount(1.5), "1.5");
        assert_eq!(format_amount(0.0), "0");
    }

    #[test]
    fn empty_collections_are_omitted() {
        let json = sample().to_canonical_json();
        assert!(!json.contains("\"types\""));
        assert!(!json.contains("\"tools\""));
        assert!(!json.contains("\"state\""));
    }
}
