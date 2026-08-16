//! Token kinds and the reserved keyword set.

use ingot_source::Span;

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
    /// Doc comment (`///`) block immediately preceding this token, if any.
    pub doc: Option<String>,
}

impl Token {
    pub fn is(&self, kind: &TokenKind) -> bool {
        &self.kind == kind
    }

    pub fn keyword(&self) -> Option<Keyword> {
        match self.kind {
            TokenKind::Keyword(keyword) => Some(keyword),
            _ => None,
        }
    }

    pub fn ident_text(&self) -> Option<&str> {
        match &self.kind {
            TokenKind::Ident(text) => Some(text),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    Ident(String),
    Keyword(Keyword),
    Int(i64),
    Float(f64),
    Str(String),

    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Colon,
    Semicolon,
    Dot,
    Question,
    Eq,
    EqEq,
    Bang,
    BangEq,
    Lt,
    Le,
    Gt,
    Ge,
    Plus,
    Minus,
    AmpAmp,
    Pipe,
    PipePipe,
    Arrow,

    /// Emitted after a lexical error so the parser can resynchronize.
    Error,
    Eof,
}

impl TokenKind {
    /// Human-readable form used in "expected X, found Y" diagnostics.
    pub fn describe(&self) -> String {
        match self {
            TokenKind::Ident(text) => format!("identifier `{text}`"),
            TokenKind::Keyword(keyword) => format!("keyword `{}`", keyword.as_str()),
            TokenKind::Int(value) => format!("integer `{value}`"),
            TokenKind::Float(value) => format!("float `{value}`"),
            TokenKind::Str(_) => "string literal".to_string(),
            TokenKind::Error => "invalid token".to_string(),
            TokenKind::Eof => "end of file".to_string(),
            other => format!("`{}`", other.symbol()),
        }
    }

    /// The literal spelling of punctuation tokens.
    pub fn symbol(&self) -> &'static str {
        match self {
            TokenKind::LParen => "(",
            TokenKind::RParen => ")",
            TokenKind::LBrace => "{",
            TokenKind::RBrace => "}",
            TokenKind::LBracket => "[",
            TokenKind::RBracket => "]",
            TokenKind::Comma => ",",
            TokenKind::Colon => ":",
            TokenKind::Semicolon => ";",
            TokenKind::Dot => ".",
            TokenKind::Question => "?",
            TokenKind::Eq => "=",
            TokenKind::EqEq => "==",
            TokenKind::Bang => "!",
            TokenKind::BangEq => "!=",
            TokenKind::Lt => "<",
            TokenKind::Le => "<=",
            TokenKind::Gt => ">",
            TokenKind::Ge => ">=",
            TokenKind::Plus => "+",
            TokenKind::Minus => "-",
            TokenKind::AmpAmp => "&&",
            TokenKind::Pipe => "|",
            TokenKind::PipePipe => "||",
            TokenKind::Arrow => "->",
            _ => "",
        }
    }
}

/// Reserved words.
///
/// The set is deliberately small. Names that only appear in one position —
/// effect names, budget keys, model capabilities, currencies — stay ordinary
/// identifiers so that the vocabulary can grow without breaking existing source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Keyword {
    Language,
    Package,
    Import,
    Type,
    Tool,
    Verifier,
    Fn,
    Agent,

    Model,
    Tools,
    Memory,
    Budget,
    Policy,
    Flow,

    Requires,
    Exact,
    Mcp,
    Working,
    Ephemeral,

    Allow,
    Deny,
    Require,
    Approval,

    Ask,
    Consult,
    Call,
    Parallel,
    Map,
    As,
    Verify,
    Emit,
    If,
    Else,
    Loop,
    Max,
    While,
    Checkpoint,
    State,

    True,
    False,
}

impl Keyword {
    pub fn as_str(self) -> &'static str {
        match self {
            Keyword::Language => "language",
            Keyword::Package => "package",
            Keyword::Import => "import",
            Keyword::Type => "type",
            Keyword::Tool => "tool",
            Keyword::Verifier => "verifier",
            Keyword::Fn => "fn",
            Keyword::Agent => "agent",
            Keyword::Model => "model",
            Keyword::Tools => "tools",
            Keyword::Memory => "memory",
            Keyword::Budget => "budget",
            Keyword::Policy => "policy",
            Keyword::Flow => "flow",
            Keyword::Requires => "requires",
            Keyword::Exact => "exact",
            Keyword::Mcp => "mcp",
            Keyword::Working => "working",
            Keyword::Ephemeral => "ephemeral",
            Keyword::Allow => "allow",
            Keyword::Deny => "deny",
            Keyword::Require => "require",
            Keyword::Approval => "approval",
            Keyword::Ask => "ask",
            Keyword::Consult => "consult",
            Keyword::Call => "call",
            Keyword::Parallel => "parallel",
            Keyword::Map => "map",
            Keyword::As => "as",
            Keyword::Verify => "verify",
            Keyword::Emit => "emit",
            Keyword::If => "if",
            Keyword::Else => "else",
            Keyword::Loop => "loop",
            Keyword::Max => "max",
            Keyword::While => "while",
            Keyword::Checkpoint => "checkpoint",
            Keyword::State => "state",
            Keyword::True => "true",
            Keyword::False => "false",
        }
    }

    /// Look a word up in the reserved set.
    ///
    /// Deliberately not `FromStr`: absence of a keyword is an ordinary outcome
    /// here, not a parse failure.
    pub fn lookup(text: &str) -> Option<Keyword> {
        let keyword = match text {
            "language" => Keyword::Language,
            "package" => Keyword::Package,
            "import" => Keyword::Import,
            "type" => Keyword::Type,
            "tool" => Keyword::Tool,
            "verifier" => Keyword::Verifier,
            "fn" => Keyword::Fn,
            "agent" => Keyword::Agent,
            "model" => Keyword::Model,
            "tools" => Keyword::Tools,
            "memory" => Keyword::Memory,
            "budget" => Keyword::Budget,
            "policy" => Keyword::Policy,
            "flow" => Keyword::Flow,
            "requires" => Keyword::Requires,
            "exact" => Keyword::Exact,
            "mcp" => Keyword::Mcp,
            "working" => Keyword::Working,
            "ephemeral" => Keyword::Ephemeral,
            "allow" => Keyword::Allow,
            "deny" => Keyword::Deny,
            "require" => Keyword::Require,
            "approval" => Keyword::Approval,
            "ask" => Keyword::Ask,
            "consult" => Keyword::Consult,
            "call" => Keyword::Call,
            "parallel" => Keyword::Parallel,
            "map" => Keyword::Map,
            "as" => Keyword::As,
            "verify" => Keyword::Verify,
            "emit" => Keyword::Emit,
            "if" => Keyword::If,
            "else" => Keyword::Else,
            "loop" => Keyword::Loop,
            "max" => Keyword::Max,
            "while" => Keyword::While,
            "checkpoint" => Keyword::Checkpoint,
            "state" => Keyword::State,
            "true" => Keyword::True,
            "false" => Keyword::False,
            _ => return None,
        };
        Some(keyword)
    }
}

/// Every reserved word, for documentation and editor grammars.
pub const KEYWORDS: &[&str] = &[
    "language",
    "package",
    "import",
    "type",
    "tool",
    "verifier",
    "fn",
    "agent",
    "model",
    "tools",
    "memory",
    "budget",
    "policy",
    "flow",
    "requires",
    "exact",
    "mcp",
    "working",
    "ephemeral",
    "allow",
    "deny",
    "require",
    "approval",
    "ask",
    "call",
    "parallel",
    "map",
    "as",
    "verify",
    "emit",
    "if",
    "else",
    "loop",
    "max",
    "while",
    "checkpoint",
    "state",
    "true",
    "false",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyword_table_round_trips() {
        for text in KEYWORDS {
            let keyword = Keyword::lookup(text).expect("keyword must be recognised");
            assert_eq!(keyword.as_str(), *text);
        }
    }

    #[test]
    fn non_keywords_stay_identifiers() {
        for text in [
            "network",
            "steps",
            "tokens",
            "cost",
            "usd",
            "tool_calling",
            "report",
        ] {
            assert!(
                Keyword::lookup(text).is_none(),
                "`{text}` must not be reserved"
            );
        }
    }
}
