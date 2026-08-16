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
    /// The IR node that made the call, for cassette matching and error
    /// messages. The same role `CompletionRequest::node` plays for a model call.
    pub node: String,
    /// The agent making the call.
    ///
    /// A host that bounds what a tool can reach needs this: two agents in one
    /// program deliberately hold different policies, and a bound wide enough
    /// for both would hand each of them the other's grants.
    pub agent: String,
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

/// Lets a boxed host be used wherever a host is expected — including as the
/// inner host of a [`crate::RecordingTools`], which is how the CLI wraps a
/// recorder around a host it chose at runtime. The same courtesy
/// [`crate::ModelProvider`] already gets.
impl<H: ToolHost + ?Sized> ToolHost for Box<H> {
    fn name(&self) -> &str {
        (**self).name()
    }

    fn provides(&self, tool: &str) -> bool {
        (**self).provides(tool)
    }

    fn call(&mut self, invocation: &ToolInvocation) -> Result<Value, ToolError> {
        (**self).call(invocation)
    }
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

/// How this run reaches a person, if it can.
///
/// One channel for both of the things a run can want from a human: a yes or no
/// on an effect the policy gates, and the answer to a question the program
/// wrote. They were designed together because they want the same channel, and
/// building it twice would have given two. See
/// [RFC-0020](../../../rfcs/0020-a-person-in-the-loop.md).
pub enum HumanChannel {
    /// Ask whoever is on the other end.
    Ask(Box<dyn Interlocutor>),
    /// Approve without asking. Requires an explicit opt-in from the operator,
    /// because the artifact asked for a human.
    ///
    /// **Approves a gate and cannot answer a question.** There is no default
    /// answer to *which framing should the report take*, and inventing one would
    /// put a value into the flow and the recording that nobody chose. The
    /// asymmetry is the difference between a decision with a known safe side and
    /// one without.
    AssumeYes,
    /// Refuse every gate and every question. The safe default for unattended
    /// runs.
    Deny,
}

impl fmt::Debug for HumanChannel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HumanChannel::Ask(_) => f.write_str("Ask(..)"),
            HumanChannel::AssumeYes => f.write_str("AssumeYes"),
            HumanChannel::Deny => f.write_str("Deny"),
        }
    }
}

/// A person, from the run's side of the channel.
pub trait Interlocutor {
    /// Whether a gated action may proceed.
    fn approve(&mut self, request: &ApprovalRequest) -> bool;

    /// Put a question to a person and return what they said.
    ///
    /// Fallible where [`Interlocutor::approve`] is not, and the difference is
    /// real rather than stylistic: a gate that cannot reach anybody has a safe
    /// answer, and a question does not. Refusing to guess is the only thing
    /// left, so this reports why instead.
    fn consult(&mut self, request: &ConsultRequest) -> Result<String, ConsultError>;
}

/// A question the program wrote, on its way to a person.
#[derive(Debug, Clone, PartialEq)]
pub struct ConsultRequest {
    pub node: String,
    /// Which consultation this is within the run, counting from zero.
    ///
    /// The same number the cassette matches by, so a recorded event stream and a
    /// recording line up without either having to carry the other's identifier.
    pub index: usize,
    pub question: String,
    /// What a person may answer, when the program limited it. Empty means free
    /// text.
    pub choices: Vec<String>,
    /// What the run wants the person to see first, named as the source named it.
    ///
    /// The same shape a model call carries, and for the same reason: the surface
    /// showing it decides how to render, because a terminal and a page do not
    /// render the same way.
    pub context: Vec<(String, Value)>,
}

/// Why a question could not be answered.
#[derive(Debug, Clone, PartialEq)]
pub enum ConsultError {
    /// There is nobody to ask.
    NoChannel(String),
    /// Somebody was asked and the answer did not arrive.
    Failed(String),
    /// An answer arrived that was not one of the choices offered.
    NotAChoice {
        answer: String,
        choices: Vec<String>,
    },
}

impl fmt::Display for ConsultError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConsultError::NoChannel(reason) => write!(f, "there is nobody to ask: {reason}"),
            ConsultError::Failed(reason) => write!(f, "the question was not answered: {reason}"),
            ConsultError::NotAChoice { answer, choices } => write!(
                f,
                "`{answer}` is not one of the choices offered ({})",
                choices.join(", ")
            ),
        }
    }
}

impl std::error::Error for ConsultError {}

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

impl Interlocutor for ScriptedApprovals {
    fn approve(&mut self, _request: &ApprovalRequest) -> bool {
        let answer = self.answers.get(self.position).copied().unwrap_or(false);
        self.position += 1;
        answer
    }

    fn consult(&mut self, request: &ConsultRequest) -> Result<String, ConsultError> {
        Err(ConsultError::NoChannel(format!(
            "scripted approvals answer gates and not questions (node `{}`)",
            request.node
        )))
    }
}

/// Answers questions from a fixed list, then refuses. Test scaffolding.
pub struct ScriptedAnswers {
    answers: Vec<String>,
    position: usize,
}

impl ScriptedAnswers {
    pub fn new(answers: Vec<impl Into<String>>) -> ScriptedAnswers {
        ScriptedAnswers {
            answers: answers.into_iter().map(Into::into).collect(),
            position: 0,
        }
    }
}

impl Interlocutor for ScriptedAnswers {
    /// Approves, so a test can put a gate and a question in one flow without
    /// needing two channels.
    fn approve(&mut self, _request: &ApprovalRequest) -> bool {
        true
    }

    fn consult(&mut self, request: &ConsultRequest) -> Result<String, ConsultError> {
        let Some(answer) = self.answers.get(self.position).cloned() else {
            return Err(ConsultError::NoChannel(format!(
                "the script has {} answer(s) and node `{}` asked for another",
                self.answers.len(),
                request.node
            )));
        };
        self.position += 1;
        Ok(answer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn invocation(name: &str) -> ToolInvocation {
        ToolInvocation {
            node: "n0".to_string(),
            agent: "test.Agent".to_string(),
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
