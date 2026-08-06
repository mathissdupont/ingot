//! Tool hosting and approval.
//!
//! The runtime never executes a tool itself. It resolves the call, re-checks the
//! artifact's policy, and hands the invocation to a [`ToolHost`] the operator
//! supplied. The default host denies everything, so a tool runs only because
//! somebody chose to provide it.

use std::collections::BTreeMap;
use std::fmt;

use serde_json::Value;

/// One resolved tool call.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolInvocation {
    /// Transport-qualified reference, e.g. `mcp:web.search`.
    pub reference: String,
    /// Bare tool name, e.g. `web.search`.
    pub name: String,
    pub transport: String,
    /// Arguments in the callee's declaration order.
    pub arguments: BTreeMap<String, Value>,
    /// Effects the artifact declares for this tool.
    pub effects: Vec<String>,
    /// The Ingot type the tool is declared to return.
    pub result_type: String,
}

#[derive(Debug)]
pub enum ToolError {
    /// The host does not provide this tool.
    NotAvailable(String),
    /// The tool ran and failed.
    Failed(String),
    /// The tool returned something that is not its declared type.
    InvalidResult(String),
}

impl fmt::Display for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ToolError::NotAvailable(name) => write!(
                f,
                "no host provides the tool `{name}`; \
                 the artifact requires it, so the run cannot continue"
            ),
            ToolError::Failed(message) => write!(f, "the tool failed: {message}"),
            ToolError::InvalidResult(message) => {
                write!(f, "the tool returned an unexpected value: {message}")
            }
        }
    }
}

impl std::error::Error for ToolError {}

/// Something that can execute a tool call.
pub trait ToolHost {
    fn name(&self) -> &str;

    /// Whether this host can serve `tool`, checked before the call is attempted
    /// so that a missing tool is reported before any effect happens.
    fn provides(&self, tool: &str) -> bool;

    fn call(&mut self, invocation: &ToolInvocation) -> Result<Value, ToolError>;
}

/// Refuses every tool. The default, so nothing runs by accident.
pub struct DenyAllTools;

impl ToolHost for DenyAllTools {
    fn name(&self) -> &str {
        "deny-all"
    }

    fn provides(&self, _tool: &str) -> bool {
        false
    }

    fn call(&mut self, invocation: &ToolInvocation) -> Result<Value, ToolError> {
        Err(ToolError::NotAvailable(invocation.name.clone()))
    }
}

/// Serves tools from an in-process table. Test scaffolding, and the basis for
/// the MCP host that replaces it.
#[derive(Default)]
pub struct StaticToolHost {
    #[allow(clippy::type_complexity)]
    handlers: BTreeMap<String, Box<dyn FnMut(&ToolInvocation) -> Result<Value, ToolError>>>,
}

impl StaticToolHost {
    pub fn new() -> StaticToolHost {
        StaticToolHost::default()
    }

    pub fn with(
        mut self,
        name: impl Into<String>,
        handler: impl FnMut(&ToolInvocation) -> Result<Value, ToolError> + 'static,
    ) -> StaticToolHost {
        self.handlers.insert(name.into(), Box::new(handler));
        self
    }
}

impl ToolHost for StaticToolHost {
    fn name(&self) -> &str {
        "static"
    }

    fn provides(&self, tool: &str) -> bool {
        self.handlers.contains_key(tool)
    }

    fn call(&mut self, invocation: &ToolInvocation) -> Result<Value, ToolError> {
        match self.handlers.get_mut(&invocation.name) {
            Some(handler) => handler(invocation),
            None => Err(ToolError::NotAvailable(invocation.name.clone())),
        }
    }
}

/// What the runtime should do when it reaches an `approval` node.
pub enum ApprovalMode {
    /// Ask the handler.
    Ask(Box<dyn ApprovalHandler>),
    /// Approve without asking. Requires an explicit opt-in from the operator,
    /// because the artifact asked for a human.
    AssumeYes,
    /// Refuse every gate. The safe default for unattended runs.
    Deny,
}

impl fmt::Debug for ApprovalMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ApprovalMode::Ask(_) => f.write_str("Ask(..)"),
            ApprovalMode::AssumeYes => f.write_str("AssumeYes"),
            ApprovalMode::Deny => f.write_str("Deny"),
        }
    }
}

/// Asked whether a gated action may proceed.
pub trait ApprovalHandler {
    fn approve(&mut self, request: &ApprovalRequest) -> bool;
}

#[derive(Debug, Clone, PartialEq)]
pub struct ApprovalRequest {
    pub node: String,
    pub effects: Vec<String>,
    /// The label the compiler attached, naming what is about to happen.
    pub reason: String,
}

/// Answers from a fixed list, then denies. Test scaffolding.
pub struct ScriptedApprovals {
    answers: Vec<bool>,
    position: usize,
}

impl ScriptedApprovals {
    pub fn new(answers: Vec<bool>) -> ScriptedApprovals {
        ScriptedApprovals {
            answers,
            position: 0,
        }
    }
}

impl ApprovalHandler for ScriptedApprovals {
    fn approve(&mut self, _request: &ApprovalRequest) -> bool {
        let answer = self.answers.get(self.position).copied().unwrap_or(false);
        self.position += 1;
        answer
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn invocation(name: &str) -> ToolInvocation {
        ToolInvocation {
            reference: format!("mcp:{name}"),
            name: name.to_string(),
            transport: "mcp".to_string(),
            arguments: BTreeMap::new(),
            effects: vec!["network".to_string()],
            result_type: "json".to_string(),
        }
    }

    #[test]
    fn the_default_host_denies_everything() {
        let mut host = DenyAllTools;
        assert!(!host.provides("web.search"));
        let error = host.call(&invocation("web.search")).unwrap_err();
        assert!(error.to_string().contains("no host provides"), "{error}");
    }

    #[test]
    fn a_static_host_serves_registered_tools() {
        let mut host = StaticToolHost::new().with("web.search", |_| Ok(json!(["a", "b"])));
        assert!(host.provides("web.search"));
        assert!(!host.provides("files.write"));
        assert_eq!(
            host.call(&invocation("web.search")).unwrap(),
            json!(["a", "b"])
        );
    }

    #[test]
    fn an_unregistered_tool_is_reported_by_name() {
        let mut host = StaticToolHost::new().with("web.search", |_| Ok(json!(null)));
        let error = host.call(&invocation("files.write")).unwrap_err();
        assert!(error.to_string().contains("files.write"), "{error}");
    }

    #[test]
    fn scripted_approvals_deny_once_exhausted() {
        let mut handler = ScriptedApprovals::new(vec![true]);
        let request = ApprovalRequest {
            node: "n0".into(),
            effects: vec!["external_write".into()],
            reason: "test".into(),
        };
        assert!(handler.approve(&request));
        assert!(
            !handler.approve(&request),
            "an exhausted script must not keep approving"
        );
    }
}
