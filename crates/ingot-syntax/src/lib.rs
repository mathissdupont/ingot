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
    pub types: Vec<TypeDecl>,
    pub tools: Vec<ToolDecl>,
    pub verifiers: Vec<VerifierDecl>,
    pub agents: Vec<AgentDecl>,
    pub span: Span,
}

// --- type expressions -----------------------------------------------------

/// A syntactic type: either a name or a list of a type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeExpr {
    Named(Ident),
    List { element: Box<TypeExpr>, span: Span },
}

impl TypeExpr {
    pub fn span(&self) -> Span {
        match self {
            TypeExpr::Named(ident) => ident.span,
            TypeExpr::List { span, .. } => *span,
        }
    }

    /// Source-like rendering, used in diagnostics and by the formatter.
    pub fn text(&self) -> String {
        match self {
            TypeExpr::Named(ident) => ident.text.clone(),
            TypeExpr::List { element, .. } => format!("{}[]", element.text()),
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
    pub effects: Vec<Ident>,
    pub doc: Option<String>,
    pub span: Span,
}

/// `verifier CitationCheck(draft: markdown, min_sources: int)`
///
/// Verifiers are deterministic checks the runtime performs on a value. They are
/// declared so that `verify` calls can be type-checked like any other call.
#[derive(Debug, Clone)]
pub struct VerifierDecl {
    pub name: Ident,
    pub params: Vec<ParamDecl>,
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
    pub span: Span,
}

/// `working ephemeral { notes: string[] }`
#[derive(Debug, Clone)]
pub struct WorkingMemory {
    pub lifetime: Ident,
    pub fields: Vec<FieldDecl>,
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
    /// `state.notes = queries`
    StateWrite {
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
            | Expr::Call { span, .. }
            | Expr::ParallelMap { span, .. }
            | Expr::Builtin { span, .. }
            | Expr::Unary { span, .. }
            | Expr::Binary { span, .. }
            | Expr::Error { span } => *span,
        }
    }
}

/// A name with optional field accesses. `state` is a reserved root.
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
}

impl PathRoot {
    pub fn span(&self) -> Span {
        match self {
            PathRoot::Binding(ident) => ident.span,
            PathRoot::State { span } => *span,
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
