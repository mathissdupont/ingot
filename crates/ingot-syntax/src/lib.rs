//! Abstract syntax tree for the Ingot agent language.
//!
//! The AST is a faithful, loss-tolerant view of the source: every node carries a
//! [`Span`], and constructs the parser could not understand are represented as
//! explicit error nodes rather than being dropped. That keeps `ingot fmt`,
//! diagnostics and the future language server working on broken input.

use ingot_source::Span;

pub mod printer;

/// An identifier together with the range it was written at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ident {
    pub text: String,
    pub span: Span,
}

impl Ident {
    pub fn new(text: impl Into<String>, span: Span) -> Self {
        Ident {
            text: text.into(),
            span,
        }
    }
}

/// A dotted name such as `web.search` or `heptapus.research`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DottedName {
    pub parts: Vec<Ident>,
    pub span: Span,
}

impl DottedName {
    pub fn text(&self) -> String {
        self.parts
            .iter()
            .map(|part| part.text.as_str())
            .collect::<Vec<_>>()
            .join(".")
    }
}

/// The `language MAJOR.MINOR` declaration every file must start with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LanguageVersion {
    pub major: u32,
    pub minor: u32,
    pub span: Span,
}

impl LanguageVersion {
    pub fn text(&self) -> String {
        format!("{}.{}", self.major, self.minor)
    }
}

/// One parsed source file.
#[derive(Debug, Clone)]
pub struct Program {
    pub language: Option<LanguageVersion>,
    pub package: Option<DottedName>,
    pub imports: Vec<ImportDecl>,
    pub types: Vec<TypeDecl>,
    pub tools: Vec<ToolDecl>,
    pub verifiers: Vec<VerifierDecl>,
    pub functions: Vec<FunctionDecl>,
    pub agents: Vec<AgentDecl>,
    pub span: Span,
}

/// `import "./shared.ing" { type search_result tool web.search }`
#[derive(Debug, Clone)]
pub struct ImportDecl {
    pub path: StringLit,
    pub items: Vec<ImportItem>,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportKind {
    Type,
    Tool,
    Verifier,
}

impl ImportKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ImportKind::Type => "type",
            ImportKind::Tool => "tool",
            ImportKind::Verifier => "verifier",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ImportItem {
    pub kind: ImportKind,
    pub name: DottedName,
    pub span: Span,
}

// --- type expressions -----------------------------------------------------

/// A syntactic type: name, list, optional or union.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeExpr {
    Named(Ident),
    List { element: Box<TypeExpr>, span: Span },
    Optional { inner: Box<TypeExpr>, span: Span },
    Union { options: Vec<TypeExpr>, span: Span },
}

impl TypeExpr {
    pub fn span(&self) -> Span {
        match self {
            TypeExpr::Named(ident) => ident.span,
            TypeExpr::List { span, .. }
            | TypeExpr::Optional { span, .. }
            | TypeExpr::Union { span, .. } => *span,
        }
    }

    /// Source-like rendering, used in diagnostics and by the formatter.
    pub fn text(&self) -> String {
        self.text_with_precedence(0)
    }

    fn text_with_precedence(&self, parent: u8) -> String {
        let precedence = self.precedence();
        let rendered = match self {
            TypeExpr::Named(ident) => ident.text.clone(),
            TypeExpr::List { element, .. } => {
                format!("{}[]", element.text_with_precedence(precedence))
            }
            TypeExpr::Optional { inner, .. } => {
                format!("{}?", inner.text_with_precedence(precedence))
            }
            TypeExpr::Union { options, .. } => options
                .iter()
                .map(|option| option.text_with_precedence(precedence))
                .collect::<Vec<_>>()
                .join(" | "),
        };
        if precedence < parent {
            format!("({rendered})")
        } else {
            rendered
        }
    }

    fn precedence(&self) -> u8 {
        match self {
            TypeExpr::Union { .. } => 1,
            TypeExpr::List { .. } | TypeExpr::Optional { .. } => 2,
            TypeExpr::Named(_) => 3,
        }
    }
}

// --- declarations ---------------------------------------------------------

#[derive(Debug, Clone)]
pub struct FieldDecl {
    pub name: Ident,
    pub ty: TypeExpr,
    pub span: Span,
}

/// `type search_result { title: string, url: string }`
#[derive(Debug, Clone)]
pub struct TypeDecl {
    pub name: Ident,
    pub fields: Vec<FieldDecl>,
    pub doc: Option<String>,
    pub span: Span,
}

pub type ParamDecl = FieldDecl;

/// `tool web.search(query: string) -> search_result[] !network`
#[derive(Debug, Clone)]
pub struct ToolDecl {
    pub name: DottedName,
    pub params: Vec<ParamDecl>,
    pub ret: TypeExpr,
    /// Declared effects, written as `!network !filesystem_write`.
    pub effects: Vec<EffectDecl>,
    pub doc: Option<String>,
    pub span: Span,
}

/// One declared effect on a tool: `!network`, or `!network("arxiv.org")`.
///
/// The parenthesised list is the effect's **reach** — where this tool goes, in
/// the same vocabulary a policy of that subject uses. It is separate from the
/// policy's grant on purpose: the tool states what it needs and the agent states
/// what it permits, so the compiler has two statements to compare rather than
/// one to take on trust. See [RFC-0014](../../../rfcs/0014-a-capabilitys-reach.md).
///
/// `values` is empty for an effect written without parentheses, which declares
/// no reach and is not the same as declaring an empty one — that is refused.
#[derive(Debug, Clone)]
pub struct EffectDecl {
    pub name: Ident,
    pub values: Vec<StringLit>,
    /// Whether a list was written at all, so `!network` and the refused
    /// `!network()` stay distinguishable after parsing.
    pub parenthesised: bool,
    pub span: Span,
}

/// `verifier MinSources(d: draft, min: int) = len(d.sources) >= min`
///
/// Verifiers are deterministic checks the runtime performs on a value. They are
/// declared so that `verify` calls can be type-checked like any other call.
///
/// A verifier with a `body` is one the toolchain can carry out: the body is a
/// pure `bool` expression over the parameters, inlined at each `verify` site.
/// A verifier without one is a name and a signature, which is all Language 0.1
/// could express — it still type-checks, and a run still reports it as
/// `notPerformed` rather than claiming it passed.
#[derive(Debug, Clone)]
pub struct VerifierDecl {
    pub name: Ident,
    pub params: Vec<ParamDecl>,
    /// The check itself, when the author wrote one. Requires language 0.2.
    pub body: Option<Expr>,
    pub doc: Option<String>,
    pub span: Span,
}

/// `fn title(result: search_result) -> string = result.title`
#[derive(Debug, Clone)]
pub struct FunctionDecl {
    pub name: Ident,
    pub params: Vec<ParamDecl>,
    pub ret: TypeExpr,
    pub body: Expr,
    pub doc: Option<String>,
    pub span: Span,
}

/// `-> report<markdown>`: a named output carrying an artifact content type.
#[derive(Debug, Clone)]
pub struct OutputDecl {
    pub name: Ident,
    pub content: Ident,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct AgentDecl {
    pub name: Ident,
    pub params: Vec<ParamDecl>,
    pub output: Option<OutputDecl>,
    pub model: Option<ModelBlock>,
    pub tools: Option<ToolsBlock>,
    pub memory: Option<MemoryBlock>,
    pub budget: Option<BudgetBlock>,
    pub policy: Option<PolicyBlock>,
    pub flow: Option<FlowBlock>,
    pub doc: Option<String>,
    pub span: Span,
}

// --- agent sections -------------------------------------------------------

#[derive(Debug, Clone)]
pub enum ModelBlock {
    /// `model requires { tool_calling  context >= 128k }`
    Requires {
        requirements: Vec<ModelRequirement>,
        span: Span,
    },
    /// `model exact "anthropic/claude-sonnet-5"`
    Exact {
        reference: String,
        reference_span: Span,
        span: Span,
    },
}

impl ModelBlock {
    pub fn span(&self) -> Span {
        match self {
            ModelBlock::Requires { span, .. } | ModelBlock::Exact { span, .. } => *span,
        }
    }
}

#[derive(Debug, Clone)]
pub enum ModelRequirement {
    /// A named capability such as `tool_calling` or `structured_output`.
    Capability(Ident),
    /// `context >= 128k`
    ContextAtLeast { tokens: i64, span: Span },
}

impl ModelRequirement {
    pub fn span(&self) -> Span {
        match self {
            ModelRequirement::Capability(ident) => ident.span,
            ModelRequirement::ContextAtLeast { span, .. } => *span,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ToolsBlock {
    pub grants: Vec<ToolGrant>,
    pub span: Span,
}

/// `mcp web.search` — binds a declared tool to a transport for this agent.
#[derive(Debug, Clone)]
pub struct ToolGrant {
    pub transport: Ident,
    pub name: DottedName,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct MemoryBlock {
    pub working: Option<WorkingMemory>,
    /// `persistent { seen: string[] = [] }`. Requires language 0.2.
    pub persistent: Option<PersistentMemory>,
    pub span: Span,
}

/// `working ephemeral { notes: string[] }`
#[derive(Debug, Clone)]
pub struct WorkingMemory {
    pub lifetime: Ident,
    pub fields: Vec<FieldDecl>,
    pub span: Span,
}

/// `persistent { seen: string[] = [] }`
///
/// A sibling of [`WorkingMemory`] rather than a second lifetime after
/// `working`: a block that outlives the run is not working memory however it is
/// spelled.
#[derive(Debug, Clone)]
pub struct PersistentMemory {
    pub fields: Vec<PersistentFieldDecl>,
    pub span: Span,
}

/// One persistent field, with the value it starts from.
///
/// The initial value is not optional. Persistent memory has no coherent "not
/// yet written" state — the first run has nothing and every later run has
/// something — so declaring the starting point once, in the type, is what
/// removes a read-before-written guard from every use site.
#[derive(Debug, Clone)]
pub struct PersistentFieldDecl {
    pub name: Ident,
    pub ty: TypeExpr,
    /// Required, and required to be a literal. `None` only when the source was
    /// malformed and the parser recovered.
    pub initial: Option<Expr>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct BudgetBlock {
    pub limits: Vec<BudgetLimit>,
    pub span: Span,
}

/// `steps <= 60`, `cost <= 5 usd`
#[derive(Debug, Clone)]
pub struct BudgetLimit {
    pub key: Ident,
    pub value: NumberLit,
    pub unit: Option<Ident>,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NumberLit {
    Int(i64),
    Float(f64),
}

impl NumberLit {
    pub fn as_f64(self) -> f64 {
        match self {
            NumberLit::Int(value) => value as f64,
            NumberLit::Float(value) => value,
        }
    }

    pub fn is_positive(self) -> bool {
        self.as_f64() > 0.0
    }
}

#[derive(Debug, Clone)]
pub struct PolicyBlock {
    pub rules: Vec<PolicyRule>,
    pub span: Span,
}

/// A policy rule is parsed generically (`subject action`) and validated in the
/// semantic phase, so unknown subjects produce a helpful error instead of a
/// syntax error.
#[derive(Debug, Clone)]
pub struct PolicyRule {
    pub subject: Ident,
    pub action: PolicyAction,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum PolicyAction {
    /// `allow`, `allow export`, `allow ["arxiv.org"]`
    Allow {
        qualifier: Option<Ident>,
        values: Vec<StringLit>,
        span: Span,
    },
    /// `deny`, `deny export`
    Deny {
        qualifier: Option<Ident>,
        span: Span,
    },
    /// `require approval`
    RequireApproval { span: Span },
}

impl PolicyAction {
    pub fn span(&self) -> Span {
        match self {
            PolicyAction::Allow { span, .. }
            | PolicyAction::Deny { span, .. }
            | PolicyAction::RequireApproval { span } => *span,
        }
    }

    pub fn describe(&self) -> &'static str {
        match self {
            PolicyAction::Allow { .. } => "allow",
            PolicyAction::Deny { .. } => "deny",
            PolicyAction::RequireApproval { .. } => "require approval",
        }
    }
}

#[derive(Debug, Clone)]
pub struct FlowBlock {
    pub statements: Vec<Stmt>,
    pub span: Span,
}

// --- statements -----------------------------------------------------------

#[derive(Debug, Clone)]
pub enum Stmt {
    /// `queries = ask<string[]>("...")`
    Bind {
        name: Ident,
        value: Expr,
        span: Span,
    },
    /// `state.notes = queries`, or `memory.seen = ...` when `scope` is
    /// [`StateScope::Persistent`].
    StateWrite {
        scope: StateScope,
        field: Ident,
        value: Expr,
        span: Span,
    },
    /// A call evaluated for its effect, e.g. `call files.write(path, data)`.
    Expr { value: Expr, span: Span },
    /// `verify CitationCheck(draft, min_sources: 8)`
    Verify {
        validator: Ident,
        args: Vec<Arg>,
        span: Span,
    },
    /// `emit report = draft`
    Emit {
        output: Ident,
        value: Expr,
        span: Span,
    },
    /// `if cond { ... } else { ... }`
    If {
        condition: Expr,
        then_branch: Vec<Stmt>,
        else_branch: Option<Vec<Stmt>>,
        span: Span,
    },
    /// `loop max 5 while cond { ... }`
    Loop {
        max: Option<LoopBound>,
        guard: Option<Expr>,
        body: Vec<Stmt>,
        span: Span,
    },
    /// `checkpoint "after-research"`
    Checkpoint { label: StringLit, span: Span },
    /// A statement the parser could not understand; kept so later phases and the
    /// formatter still see the shape of the flow.
    Error { span: Span },
}

impl Stmt {
    pub fn span(&self) -> Span {
        match self {
            Stmt::Bind { span, .. }
            | Stmt::StateWrite { span, .. }
            | Stmt::Expr { span, .. }
            | Stmt::Verify { span, .. }
            | Stmt::Emit { span, .. }
            | Stmt::If { span, .. }
            | Stmt::Loop { span, .. }
            | Stmt::Checkpoint { span, .. }
            | Stmt::Error { span } => *span,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct LoopBound {
    pub value: i64,
    pub span: Span,
}

// --- expressions ----------------------------------------------------------

#[derive(Debug, Clone)]
pub enum Expr {
    Str(StringLit),
    Int {
        value: i64,
        span: Span,
    },
    Float {
        value: f64,
        span: Span,
    },
    Bool {
        value: bool,
        span: Span,
    },
    List {
        items: Vec<Expr>,
        span: Span,
    },
    /// `topic`, `source.title`, `state.notes`
    Path(PathExpr),
    /// `ask<markdown>("...", context: sources)`
    Ask {
        result: TypeExpr,
        args: Vec<Arg>,
        span: Span,
    },
    /// `consult("Which framing?", choices: ["a", "b"])` — a question put to a
    /// person, whose answer the flow reads.
    ///
    /// No type parameter, unlike [`Expr::Ask`]. A person types text or picks
    /// from a list, so the answer is always `string` — and a type parameter with
    /// exactly one legal value is not a type parameter. See
    /// [RFC-0020](../../../rfcs/0020-a-person-in-the-loop.md).
    Consult {
        args: Vec<Arg>,
        span: Span,
    },
    /// `call web.search(query)` — a tool call or a sub-agent call.
    Call {
        callee: DottedName,
        args: Vec<Arg>,
        span: Span,
    },
    /// `parallel map queries as query { ... }`
    ParallelMap {
        source: Box<Expr>,
        binder: Ident,
        body: Vec<Stmt>,
        span: Span,
    },
    /// `len(sources)`
    Builtin {
        name: Ident,
        args: Vec<Expr>,
        span: Span,
    },
    /// `normalise_title(result)` â€” a pure user-defined helper call.
    FunctionCall {
        callee: Ident,
        args: Vec<Arg>,
        span: Span,
    },
    /// `call fs.read_file(path) else "no write-up was filed"` — a value to use
    /// when the attempt fails. Requires language 0.4.
    ///
    /// `attempt` is an [`Expr::Ask`] or an [`Expr::Call`]; nothing else may carry
    /// an `else`, and the semantic phase is what refuses the rest. `fallback` is
    /// a **pure** expression — the restriction is the whole design, because a
    /// fallback that reaches nothing cannot widen what the artifact's policy
    /// describes and cannot spend a second step. See
    /// [RFC-0022](../../../rfcs/0022-a-failure-an-iteration-can-absorb.md).
    Fallback {
        attempt: Box<Expr>,
        fallback: Box<Expr>,
        span: Span,
    },
    Unary {
        op: UnaryOp,
        operand: Box<Expr>,
        span: Span,
    },
    Binary {
        op: BinaryOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
        span: Span,
    },
    Error {
        span: Span,
    },
}

impl Expr {
    pub fn span(&self) -> Span {
        match self {
            Expr::Str(literal) => literal.span,
            Expr::Path(path) => path.span,
            Expr::Int { span, .. }
            | Expr::Float { span, .. }
            | Expr::Bool { span, .. }
            | Expr::List { span, .. }
            | Expr::Ask { span, .. }
            | Expr::Consult { span, .. }
            | Expr::Call { span, .. }
            | Expr::ParallelMap { span, .. }
            | Expr::Builtin { span, .. }
            | Expr::FunctionCall { span, .. }
            | Expr::Fallback { span, .. }
            | Expr::Unary { span, .. }
            | Expr::Binary { span, .. }
            | Expr::Error { span } => *span,
        }
    }
}

/// A name with optional field accesses. `state` and `memory` are reserved roots.
#[derive(Debug, Clone)]
pub struct PathExpr {
    pub root: PathRoot,
    pub segments: Vec<Ident>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum PathRoot {
    /// An input, binding or loop variable.
    Binding(Ident),
    /// The `state` keyword, addressing working memory.
    State { span: Span },
    /// The `memory` keyword, addressing persistent memory. Requires
    /// language 0.2.
    Memory { span: Span },
}

impl PathRoot {
    pub fn span(&self) -> Span {
        match self {
            PathRoot::Binding(ident) => ident.span,
            PathRoot::State { span } | PathRoot::Memory { span } => *span,
        }
    }

    /// The store this root addresses, if it addresses one at all.
    pub fn scope(&self) -> Option<StateScope> {
        match self {
            PathRoot::Binding(_) => None,
            PathRoot::State { .. } => Some(StateScope::Ephemeral),
            PathRoot::Memory { .. } => Some(StateScope::Persistent),
        }
    }
}

/// Which of an agent's two stores a read or a write addresses.
///
/// Kept distinct at every use site rather than resolved to one map, because a
/// write that outlives the run is a different act from a write to a scratchpad
/// and the reader of a flow should not have to scroll to the declaration to
/// tell which one they are looking at. See
/// [RFC-0018](../../../rfcs/0018-state-that-outlives-a-run.md) §2.1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateScope {
    /// `state.x` — working memory, discarded when the run ends.
    Ephemeral,
    /// `memory.x` — persistent memory, written back to the agent's store.
    Persistent,
}

impl StateScope {
    /// The keyword that addresses this store.
    pub fn root(self) -> &'static str {
        match self {
            StateScope::Ephemeral => "state",
            StateScope::Persistent => "memory",
        }
    }

    /// What a field of this store is called, in a diagnostic.
    pub fn noun(self) -> &'static str {
        match self {
            StateScope::Ephemeral => "state field",
            StateScope::Persistent => "persistent memory field",
        }
    }
}

/// A positional or named call argument.
#[derive(Debug, Clone)]
pub struct Arg {
    pub name: Option<Ident>,
    pub value: Expr,
    pub span: Span,
}

/// A string literal, split into literal text and `${...}` interpolations.
#[derive(Debug, Clone)]
pub struct StringLit {
    pub parts: Vec<StringPart>,
    pub span: Span,
}

impl StringLit {
    /// The literal with interpolations written back in `${...}` form.
    pub fn template(&self) -> String {
        let mut out = String::new();
        for part in &self.parts {
            match part {
                StringPart::Literal(text) => out.push_str(text),
                StringPart::Interpolation(path) => {
                    out.push_str("${");
                    out.push_str(&path.text());
                    out.push('}');
                }
            }
        }
        out
    }

    /// True when the literal has no interpolations.
    pub fn is_plain(&self) -> bool {
        self.parts
            .iter()
            .all(|part| matches!(part, StringPart::Literal(_)))
    }

    /// The plain text of a literal without interpolations.
    pub fn plain_text(&self) -> String {
        self.parts
            .iter()
            .filter_map(|part| match part {
                StringPart::Literal(text) => Some(text.as_str()),
                StringPart::Interpolation(_) => None,
            })
            .collect()
    }
}

#[derive(Debug, Clone)]
pub enum StringPart {
    Literal(String),
    Interpolation(InterpolationPath),
}

#[derive(Debug, Clone)]
pub struct InterpolationPath {
    pub root: PathRoot,
    pub segments: Vec<Ident>,
    pub span: Span,
}

impl InterpolationPath {
    pub fn text(&self) -> String {
        let mut out = match &self.root {
            PathRoot::Binding(ident) => ident.text.clone(),
            PathRoot::State { .. } => "state".to_string(),
            PathRoot::Memory { .. } => "memory".to_string(),
        };
        for segment in &self.segments {
            out.push('.');
            out.push_str(&segment.text);
        }
        out
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Not,
    Neg,
}

impl UnaryOp {
    pub fn symbol(self) -> &'static str {
        match self {
            UnaryOp::Not => "!",
            UnaryOp::Neg => "-",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Eq,
    NotEq,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
    Add,
    Sub,
}

impl BinaryOp {
    pub fn symbol(self) -> &'static str {
        match self {
            BinaryOp::Eq => "==",
            BinaryOp::NotEq => "!=",
            BinaryOp::Lt => "<",
            BinaryOp::Le => "<=",
            BinaryOp::Gt => ">",
            BinaryOp::Ge => ">=",
            BinaryOp::And => "&&",
            BinaryOp::Or => "||",
            BinaryOp::Add => "+",
            BinaryOp::Sub => "-",
        }
    }

    pub fn is_comparison(self) -> bool {
        matches!(
            self,
            BinaryOp::Eq
                | BinaryOp::NotEq
                | BinaryOp::Lt
                | BinaryOp::Le
                | BinaryOp::Gt
                | BinaryOp::Ge
        )
    }

    pub fn is_logical(self) -> bool {
        matches!(self, BinaryOp::And | BinaryOp::Or)
    }

    pub fn is_arithmetic(self) -> bool {
        matches!(self, BinaryOp::Add | BinaryOp::Sub)
    }
}

/// Builtin functions available in flow expressions.
pub const BUILTINS: &[&str] = &["len"];
