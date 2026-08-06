//! IR nodes and the value expressions they carry.
//!
//! Every node is one wide struct with optional fields rather than a tagged
//! union. That keeps the JSON in a fixed, readable field order (a `#[serde(flatten)]`
//! union would collapse into an alphabetically sorted map) and lets a backend
//! read `node.kind` and pick the fields it understands.

use serde::{Deserialize, Serialize};

/// What a node does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeKind {
    /// A model call with a typed response.
    #[serde(rename = "llm.call")]
    LlmCall,
    /// A tool invocation over a declared transport.
    #[serde(rename = "tool.call")]
    ToolCall,
    /// A call to another agent.
    #[serde(rename = "agent.call")]
    AgentCall,
    /// Two-way conditional. Arms are referenced by node id.
    #[serde(rename = "branch")]
    Branch,
    /// Bounded fan-out over a list.
    #[serde(rename = "parallel")]
    Parallel,
    /// Bounded iteration.
    #[serde(rename = "loop")]
    Loop,
    /// Human approval checkpoint, inserted by the compiler from policy.
    #[serde(rename = "approval")]
    Approval,
    /// Deterministic validation of a value.
    #[serde(rename = "verify")]
    Verify,
    /// Read from working memory.
    #[serde(rename = "state.read")]
    StateRead,
    /// Write to working memory.
    #[serde(rename = "state.write")]
    StateWrite,
    /// Produce a declared output artifact.
    #[serde(rename = "artifact.emit")]
    ArtifactEmit,
    /// Resumable boundary.
    #[serde(rename = "checkpoint")]
    Checkpoint,
}

impl NodeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            NodeKind::LlmCall => "llm.call",
            NodeKind::ToolCall => "tool.call",
            NodeKind::AgentCall => "agent.call",
            NodeKind::Branch => "branch",
            NodeKind::Parallel => "parallel",
            NodeKind::Loop => "loop",
            NodeKind::Approval => "approval",
            NodeKind::Verify => "verify",
            NodeKind::StateRead => "state.read",
            NodeKind::StateWrite => "state.write",
            NodeKind::ArtifactEmit => "artifact.emit",
            NodeKind::Checkpoint => "checkpoint",
        }
    }

    /// Whether the node consumes a step from the `steps` budget.
    pub fn consumes_step(self) -> bool {
        matches!(
            self,
            NodeKind::LlmCall | NodeKind::ToolCall | NodeKind::AgentCall
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Node {
    pub id: String,
    pub kind: NodeKind,

    /// Name this node's result is bound to, when it produces one.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub binding: Option<String>,

    // --- calls ------------------------------------------------------------
    /// `mcp:web.search` for tool calls.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub tool: Option<String>,
    /// Target agent name for `agent.call`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent: Option<String>,
    /// Verifier name for `verify`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub verifier: Option<String>,
    /// Prompt template for `llm.call`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub prompt: Option<Value>,
    /// Declared response type of an `llm.call`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub response_type: Option<String>,
    /// Positional arguments, in declaration order.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub args: Vec<Argument>,

    // --- control flow -----------------------------------------------------
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub condition: Option<Value>,
    /// First node of the "then" arm.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub then: Option<String>,
    #[serde(rename = "else", skip_serializing_if = "Option::is_none", default)]
    pub otherwise: Option<String>,
    /// First node of a loop or map body.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub body: Option<String>,
    /// Loop variable introduced by `parallel`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub binder: Option<String>,
    /// List a `parallel` node maps over.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub source: Option<Value>,
    /// Fan-out mode; `map` is the only one in IR 0.1.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub max_iterations: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub guard: Option<Value>,

    // --- state, outputs, checkpoints --------------------------------------
    /// State field for `state.read` and `state.write`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub field: Option<String>,
    /// Value written by `state.write` or emitted by `artifact.emit`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub value: Option<Value>,
    /// Output name for `artifact.emit`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub output: Option<String>,
    /// Label of a `checkpoint`, or the reason an `approval` was inserted.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub label: Option<String>,
    /// Effects the approval gate covers.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub effects: Vec<String>,

    /// Next node in this region, or `null` at the end of one.
    pub next: Option<String>,
}

impl Node {
    pub fn new(id: impl Into<String>, kind: NodeKind) -> Node {
        Node {
            id: id.into(),
            kind,
            binding: None,
            tool: None,
            agent: None,
            verifier: None,
            prompt: None,
            response_type: None,
            args: Vec::new(),
            condition: None,
            then: None,
            otherwise: None,
            body: None,
            binder: None,
            source: None,
            mode: None,
            max_iterations: None,
            guard: None,
            field: None,
            value: None,
            output: None,
            label: None,
            effects: Vec::new(),
            next: None,
        }
    }
}

/// One argument of a call, always named so backends never rely on order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Argument {
    pub name: String,
    pub value: Value,
}

/// Where a reference reads from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RefScope {
    /// An agent input.
    Input,
    /// A value bound earlier in the flow, including loop variables.
    Binding,
    /// A working-memory field.
    State,
}

/// A pure expression the runtime evaluates without calling anything.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Value {
    /// A constant, with the Ingot type it was written as.
    Literal {
        #[serde(rename = "type")]
        ty: String,
        value: serde_json::Value,
    },
    /// A read of an input, a binding or a state field, plus field accesses.
    Ref {
        scope: RefScope,
        /// First element is the root name; the rest are record field accesses.
        path: Vec<String>,
    },
    List {
        items: Vec<Value>,
    },
    /// A prompt with typed placeholders resolved at compile time.
    Template {
        parts: Vec<TemplatePart>,
    },
    Unary {
        op: String,
        operand: Box<Value>,
    },
    Binary {
        op: String,
        lhs: Box<Value>,
        rhs: Box<Value>,
    },
    Builtin {
        name: String,
        args: Vec<Value>,
    },
    /// Emitted where the source failed to check. Present so that a partial IR
    /// can still be inspected; a build never ships one.
    Unknown,
}

impl Value {
    pub fn string(text: impl Into<String>) -> Value {
        Value::Literal {
            ty: "string".to_string(),
            value: serde_json::Value::String(text.into()),
        }
    }

    pub fn int(value: i64) -> Value {
        Value::Literal {
            ty: "int".to_string(),
            value: serde_json::Value::from(value),
        }
    }

    pub fn bool(value: bool) -> Value {
        Value::Literal {
            ty: "bool".to_string(),
            value: serde_json::Value::Bool(value),
        }
    }
}

/// One piece of a prompt.
///
/// A substitution carries a full [`Value`] rather than only a reference: a
/// placeholder may resolve to a folded constant or to a pure expression, and a
/// backend should not have to care which. The declared type tells it how to
/// render the result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum TemplatePart {
    /// Literal prompt text.
    Text { value: String },
    /// A resolved `${...}` placeholder.
    Value {
        value: Value,
        #[serde(rename = "type")]
        ty: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_kinds_serialise_with_dotted_names() {
        let json = serde_json::to_string(&NodeKind::ToolCall).unwrap();
        assert_eq!(json, "\"tool.call\"");
        assert_eq!(NodeKind::ArtifactEmit.as_str(), "artifact.emit");
    }

    #[test]
    fn only_call_nodes_consume_steps() {
        assert!(NodeKind::LlmCall.consumes_step());
        assert!(NodeKind::ToolCall.consumes_step());
        assert!(NodeKind::AgentCall.consumes_step());
        assert!(!NodeKind::Branch.consumes_step());
        assert!(!NodeKind::Checkpoint.consumes_step());
    }

    #[test]
    fn empty_node_fields_are_omitted() {
        let node = Node::new("n0", NodeKind::Checkpoint);
        let json = serde_json::to_string(&node).unwrap();
        assert_eq!(json, r#"{"id":"n0","kind":"checkpoint","next":null}"#);
    }

    #[test]
    fn values_round_trip() {
        let value = Value::Template {
            parts: vec![
                TemplatePart::Text {
                    value: "Research: ".to_string(),
                },
                TemplatePart::Value {
                    value: Value::Ref {
                        scope: RefScope::Input,
                        path: vec!["topic".to_string()],
                    },
                    ty: "string".to_string(),
                },
            ],
        };
        let json = serde_json::to_string(&value).unwrap();
        let parsed: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, value);
    }

    #[test]
    fn the_else_arm_is_spelled_else_in_json() {
        let mut node = Node::new("n0", NodeKind::Branch);
        node.otherwise = Some("n5".to_string());
        let json = serde_json::to_string(&node).unwrap();
        assert!(json.contains(r#""else":"n5""#));
    }
}
