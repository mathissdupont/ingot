//! The type, effect and capability vocabulary of Ingot.
//!
//! This crate is deliberately free of syntax and of any backend knowledge: it is
//! the shared vocabulary that the semantic checker, the IR and every future
//! target backend agree on.

use std::fmt;

pub mod effects;
pub mod policy;

pub use effects::{Effect, EffectSet};
pub use policy::{PolicyDecision, PolicySubject};

/// A resolved type.
///
/// Ingot v0.1 has no generics, no optionals and no unions. `Unknown` exists only
/// so that a single type error does not cascade into dozens of follow-ups.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ty {
    String,
    Int,
    Float,
    Bool,
    Json,
    Bytes,
    /// Plain text content.
    Text,
    /// Markdown content.
    Markdown,
    /// A file handle produced or consumed by a tool.
    File,
    List(Box<Ty>),
    /// A user-declared record type, referenced by name.
    Record(String),
    /// Produced after an error so that checking can continue.
    Unknown,
}

impl Ty {
    /// Resolve a primitive type name. Record types are resolved by the checker.
    pub fn from_primitive_name(name: &str) -> Option<Ty> {
        let ty = match name {
            "string" => Ty::String,
            "int" => Ty::Int,
            "float" => Ty::Float,
            "bool" => Ty::Bool,
            "json" => Ty::Json,
            "bytes" => Ty::Bytes,
            "text" => Ty::Text,
            "markdown" => Ty::Markdown,
            "file" => Ty::File,
            _ => return None,
        };
        Some(ty)
    }

    pub fn list_of(element: Ty) -> Ty {
        Ty::List(Box::new(element))
    }

    pub fn element(&self) -> Option<&Ty> {
        match self {
            Ty::List(element) => Some(element),
            _ => None,
        }
    }

    pub fn is_unknown(&self) -> bool {
        match self {
            Ty::Unknown => true,
            Ty::List(element) => element.is_unknown(),
            _ => false,
        }
    }

    /// Whether a value of type `self` may be used where `expected` is required.
    ///
    /// Language 0.1 permits exactly two widenings, both of them lossless:
    ///
    /// * `int` to `float`
    /// * `markdown` to `text`, because every markdown value is also valid text
    ///
    /// Nothing else converts, including in the other direction and inside
    /// lists. Keeping the rule this small is what makes "it compiles" a
    /// meaningful statement about a portable artifact.
    pub fn is_assignable_to(&self, expected: &Ty) -> bool {
        if self.is_unknown() || expected.is_unknown() {
            return true;
        }
        if self == expected {
            return true;
        }
        match (self, expected) {
            (Ty::Int, Ty::Float) => true,
            (Ty::Markdown, Ty::Text) => true,
            (Ty::List(actual), Ty::List(expected)) => actual.is_assignable_to(expected),
            _ => false,
        }
    }

    /// Types that may appear as the content type of an agent output.
    pub fn is_artifact_content(&self) -> bool {
        matches!(self, Ty::Text | Ty::Markdown | Ty::Json | Ty::File)
    }

    /// Types a comparison operator accepts.
    pub fn is_comparable(&self) -> bool {
        matches!(
            self,
            Ty::String | Ty::Int | Ty::Float | Ty::Bool | Ty::Text | Ty::Markdown
        )
    }

    /// Types `+` and `-` accept.
    pub fn is_numeric(&self) -> bool {
        matches!(self, Ty::Int | Ty::Float)
    }

    /// Types that can be substituted into a prompt.
    pub fn is_renderable(&self) -> bool {
        !matches!(self, Ty::Bytes | Ty::File)
    }
}

impl fmt::Display for Ty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Ty::String => f.write_str("string"),
            Ty::Int => f.write_str("int"),
            Ty::Float => f.write_str("float"),
            Ty::Bool => f.write_str("bool"),
            Ty::Json => f.write_str("json"),
            Ty::Bytes => f.write_str("bytes"),
            Ty::Text => f.write_str("text"),
            Ty::Markdown => f.write_str("markdown"),
            Ty::File => f.write_str("file"),
            Ty::List(element) => write!(f, "{element}[]"),
            Ty::Record(name) => f.write_str(name),
            Ty::Unknown => f.write_str("<unknown>"),
        }
    }
}

/// Model capabilities an agent may require.
///
/// Requiring capabilities instead of naming a model is what lets the same source
/// compile against different providers; the resolver picks a model that
/// satisfies the set.
pub const MODEL_CAPABILITIES: &[&str] = &[
    "tool_calling",
    "structured_output",
    "streaming",
    "vision",
    "reasoning",
    "parallel_tool_calls",
];

pub fn is_known_model_capability(name: &str) -> bool {
    MODEL_CAPABILITIES.contains(&name)
}

/// Budget keys recognised in v0.1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BudgetKey {
    Steps,
    Tokens,
    Cost,
}

impl BudgetKey {
    pub fn from_name(name: &str) -> Option<BudgetKey> {
        match name {
            "steps" => Some(BudgetKey::Steps),
            "tokens" => Some(BudgetKey::Tokens),
            "cost" => Some(BudgetKey::Cost),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            BudgetKey::Steps => "steps",
            BudgetKey::Tokens => "tokens",
            BudgetKey::Cost => "cost",
        }
    }

    /// `cost` carries a currency; the others are plain counts.
    pub fn requires_unit(self) -> bool {
        matches!(self, BudgetKey::Cost)
    }
}

/// Currencies accepted for `cost` budgets in v0.1.
pub const SUPPORTED_CURRENCIES: &[&str] = &["usd", "eur", "try"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn int_widens_to_float_but_not_the_other_way() {
        assert!(Ty::Int.is_assignable_to(&Ty::Float));
        assert!(!Ty::Float.is_assignable_to(&Ty::Int));
    }

    #[test]
    fn markdown_widens_to_text_but_not_the_other_way() {
        assert!(Ty::Markdown.is_assignable_to(&Ty::Text));
        assert!(!Ty::Text.is_assignable_to(&Ty::Markdown));
        assert!(!Ty::Markdown.is_assignable_to(&Ty::String));
    }

    #[test]
    fn lists_are_covariant_only_through_the_same_widening() {
        let ints = Ty::list_of(Ty::Int);
        let floats = Ty::list_of(Ty::Float);
        assert!(ints.is_assignable_to(&floats));
        assert!(!Ty::list_of(Ty::String).is_assignable_to(&floats));
    }

    #[test]
    fn unknown_absorbs_errors_without_cascading() {
        assert!(Ty::Unknown.is_assignable_to(&Ty::Markdown));
        assert!(Ty::Markdown.is_assignable_to(&Ty::Unknown));
    }

    #[test]
    fn only_content_types_can_back_an_artifact() {
        assert!(Ty::Markdown.is_artifact_content());
        assert!(Ty::Json.is_artifact_content());
        assert!(!Ty::Int.is_artifact_content());
        assert!(!Ty::list_of(Ty::Markdown).is_artifact_content());
    }

    #[test]
    fn renders_types_the_way_source_spells_them() {
        assert_eq!(Ty::list_of(Ty::String).to_string(), "string[]");
        assert_eq!(
            Ty::Record("search_result".into()).to_string(),
            "search_result"
        );
    }
}
