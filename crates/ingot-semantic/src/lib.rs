//! Name resolution, type checking and effect/policy analysis.
//!
//! The output is an [`Analysis`]: symbol tables plus side tables that give every
//! expression a type and an effect set. Lowering reads those tables instead of
//! re-deriving them, so the compiler has exactly one place where a program's
//! meaning is decided.
//!
//! Two rules shape everything here:
//!
//! * **Default-deny.** An effect is available only if the `policy` block grants
//!   it. A missing rule is a denial with its own diagnostic, so silence is never
//!   mistaken for permission.
//! * **Statically bounded.** Loops carry a `max`, recursion is rejected, and the
//!   shortest path through the flow is compared against the `steps` budget.

use std::collections::{BTreeMap, HashMap};

use ingot_diagnostics::DiagnosticBag;
use ingot_source::Span;
use ingot_syntax::Program;
use ingot_types::{Effect, EffectSet, PolicyDecision, PolicySubject, Ty};

mod check;

#[cfg(test)]
mod tests;

/// A named, typed slot: an input, a record field, a state field or a parameter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Param {
    pub name: String,
    pub ty: Ty,
}

#[derive(Debug, Clone)]
pub struct RecordInfo {
    pub name: String,
    pub fields: Vec<Param>,
    pub doc: Option<String>,
}

impl RecordInfo {
    pub fn field(&self, name: &str) -> Option<&Param> {
        self.fields.iter().find(|field| field.name == name)
    }
}

#[derive(Debug, Clone)]
pub struct ToolInfo {
    /// Dotted name as written, e.g. `web.search`.
    pub name: String,
    pub params: Vec<Param>,
    pub result: Ty,
    pub effects: EffectSet,
    /// Where this tool goes, as it declared it. Empty when it declared nothing,
    /// which is not the same as reaching nothing.
    pub reach: Vec<DeclaredReach>,
    pub doc: Option<String>,
    pub span: Span,
}

/// One value of a tool's declared reach: `!network("arxiv.org")`.
///
/// Flat rather than a map from effect to values, because every use is either a
/// scan for containment or a sort into the IR, and both read better over a
/// list. The span is the value's own, so a diagnostic can underline the host
/// that is not granted rather than the whole declaration.
///
/// See [RFC-0014](../../rfcs/0014-a-capabilitys-reach.md).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredReach {
    pub effect: Effect,
    pub value: String,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct VerifierInfo {
    pub name: String,
    pub params: Vec<Param>,
    pub doc: Option<String>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct FunctionInfo {
    pub name: String,
    pub params: Vec<Param>,
    pub result: Ty,
    pub doc: Option<String>,
    pub span: Span,
    pub decl_index: usize,
}

#[derive(Debug, Clone)]
pub struct OutputInfo {
    pub name: String,
    /// The artifact content type, e.g. `markdown`.
    pub content: Ty,
}

/// How the agent picks a model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelInfo {
    /// Capability requirements; the runtime resolves a model that satisfies them.
    Requires {
        capabilities: Vec<String>,
        context_tokens: Option<i64>,
    },
    /// A pinned provider/model reference.
    Exact { reference: String },
    /// No `model` section: the backend's default applies.
    Unspecified,
}

#[derive(Debug, Clone)]
pub struct GrantInfo {
    pub transport: String,
    pub tool: String,
    pub span: Span,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct BudgetInfo {
    pub steps: Option<i64>,
    pub tokens: Option<i64>,
    pub cost: Option<CostBudget>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CostBudget {
    pub amount: f64,
    pub currency: String,
}

#[derive(Debug, Clone)]
pub struct PolicyRuleInfo {
    pub decision: PolicyDecision,
    /// Allowlist entries, e.g. hosts for `network allow [...]`.
    pub values: Vec<String>,
    pub qualifier: Option<String>,
    pub span: Span,
}

/// Everything the checker learned about one agent.
#[derive(Debug, Clone)]
pub struct AgentInfo {
    pub name: String,
    /// `package.Name`, or just `Name` when the file declares no package.
    pub qualified_name: String,
    pub inputs: Vec<Param>,
    pub output: Option<OutputInfo>,
    pub model: ModelInfo,
    pub grants: Vec<GrantInfo>,
    pub state: Vec<Param>,
    pub budget: BudgetInfo,
    pub policy: BTreeMap<PolicySubject, PolicyRuleInfo>,
    /// Union of the effects every reachable call in the flow can trigger.
    pub effects: EffectSet,
    /// Index into `Program::agents`, so lowering can find the syntax again.
    pub decl_index: usize,
}

impl AgentInfo {
    /// What the policy says about an effect. Default-deny: unmentioned subjects
    /// come back as [`PolicyDecision::Unspecified`], which does not permit.
    pub fn decision_for(&self, effect: Effect) -> PolicyDecision {
        if effect.is_implicitly_granted() {
            return PolicyDecision::Allow;
        }
        let Some(subject) = PolicySubject::for_effect(effect) else {
            return PolicyDecision::Allow;
        };
        self.policy
            .get(&subject)
            .map(|rule| rule.decision)
            .unwrap_or(PolicyDecision::Unspecified)
    }

    /// True when performing `effects` needs a human approval checkpoint first.
    pub fn requires_approval(&self, effects: &EffectSet) -> bool {
        effects
            .iter()
            .any(|effect| self.decision_for(effect) == PolicyDecision::RequireApproval)
    }
}

/// What a `call` resolved to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallTarget {
    Tool(String),
    Agent(String),
}

/// Resolution result for one `call` expression.
#[derive(Debug, Clone)]
pub struct CallInfo {
    pub target: CallTarget,
    pub result: Ty,
    pub effects: EffectSet,
    /// For each declared parameter, the index of the argument that supplies it.
    /// Named arguments are normalised into declaration order here so that
    /// lowering emits a deterministic argument list.
    pub arg_order: Vec<Option<usize>>,
}

/// Resolution result for one `verify` statement.
#[derive(Debug, Clone)]
pub struct VerifyInfo {
    pub verifier: String,
    pub arg_order: Vec<Option<usize>>,
}

/// Resolution result for one pure helper function call.
#[derive(Debug, Clone)]
pub struct FunctionCallInfo {
    pub function: String,
    pub result: Ty,
    pub arg_order: Vec<Option<usize>>,
}

/// Type and effects of one expression.
#[derive(Debug, Clone)]
pub struct ExprInfo {
    pub ty: Ty,
    pub effects: EffectSet,
}

/// The result of checking one program.
#[derive(Debug)]
pub struct Analysis {
    pub language: Option<(u32, u32)>,
    pub package: Option<String>,
    pub records: BTreeMap<String, RecordInfo>,
    pub tools: BTreeMap<String, ToolInfo>,
    pub verifiers: BTreeMap<String, VerifierInfo>,
    pub functions: BTreeMap<String, FunctionInfo>,
    pub agents: Vec<AgentInfo>,
    /// Keyed by expression span, which is unique within a compilation.
    pub exprs: HashMap<Span, ExprInfo>,
    /// Keyed by the span of the `call` expression.
    pub calls: HashMap<Span, CallInfo>,
    /// Keyed by the span of the `verify` statement.
    pub verifies: HashMap<Span, VerifyInfo>,
    /// Keyed by the span of the helper call expression.
    pub function_calls: HashMap<Span, FunctionCallInfo>,
    /// Type of each resolved `${...}` placeholder, keyed by the placeholder span.
    /// Lowering needs it to tell a backend how to render each substitution.
    pub interpolations: HashMap<Span, Ty>,
    pub diagnostics: DiagnosticBag,
}

impl Analysis {
    pub fn expr(&self, span: Span) -> Option<&ExprInfo> {
        self.exprs.get(&span)
    }

    /// The inferred type of an expression, or `Unknown` if it failed to check.
    pub fn type_of(&self, span: Span) -> Ty {
        self.exprs
            .get(&span)
            .map(|info| info.ty.clone())
            .unwrap_or(Ty::Unknown)
    }

    pub fn call(&self, span: Span) -> Option<&CallInfo> {
        self.calls.get(&span)
    }

    pub fn function_call(&self, span: Span) -> Option<&FunctionCallInfo> {
        self.function_calls.get(&span)
    }

    pub fn agent(&self, name: &str) -> Option<&AgentInfo> {
        self.agents.iter().find(|agent| agent.name == name)
    }

    pub fn has_errors(&self) -> bool {
        self.diagnostics.has_errors()
    }
}

/// Check a parsed program.
pub fn analyze(program: &Program) -> Analysis {
    check::Checker::new(program).run()
}
