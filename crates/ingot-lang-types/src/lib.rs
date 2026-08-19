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
/// `Unknown` exists only so that a single type error does not cascade into
/// dozens of follow-ups.
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
    /// A nullable value: either `T` or absent/null.
    Optional(Box<Ty>),
    /// A value that may be any one of several alternatives.
    Union(Vec<Ty>),
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

    pub fn optional(inner: Ty) -> Ty {
        Ty::Optional(Box::new(inner))
    }

    pub fn union(options: Vec<Ty>) -> Ty {
        let mut flattened = Vec::new();
        for option in options {
            match option {
                Ty::Union(nested) => flattened.extend(nested),
                other => flattened.push(other),
            }
        }
        let mut unique = Vec::new();
        for option in flattened {
            if !unique.contains(&option) {
                unique.push(option);
            }
        }
        if unique.len() == 1 {
            unique.pop().unwrap()
        } else {
            Ty::Union(unique)
        }
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
            Ty::List(element) | Ty::Optional(element) => element.is_unknown(),
            Ty::Union(options) => options.iter().any(Ty::is_unknown),
            _ => false,
        }
    }

    /// Whether a value of type `self` may be used where `expected` is required.
    ///
    /// Exactly three widenings, all of them lossless and all in the same
    /// direction — from the more specific type to the less:
    ///
    /// * `int` to `float`
    /// * `markdown` to `text`, because every markdown value is also valid text
    /// * `string` to `text`, because a string is text (Language 0.4)
    ///
    /// Nothing else converts, including in the other direction and inside
    /// lists. Keeping the rule this small is what makes "it compiles" a
    /// meaningful statement about a portable artifact.
    ///
    /// **`string` to `markdown` is deliberately not here**, and the reason is the
    /// direction. The two rules above widen; that one would narrow, claiming an
    /// arbitrary string satisfies the more specific type. Admitting it would
    /// leave no ground to refuse `text` to `markdown` either — a `text` value is
    /// no less markdown than a bare string is — and then `markdown` and `text`
    /// are one type with two names, which is not what an artifact's content type
    /// or its file extension mean. A program that wants a markdown fallback asks
    /// for `string` and lets the step that assembles the document produce the
    /// markdown. See [GAP-045].
    ///
    /// [GAP-045]: https://github.com/mathissdupont/ingot/blob/main/docs/gaps.md#gap-045
    pub fn is_assignable_to(&self, expected: &Ty) -> bool {
        if self.is_unknown() || expected.is_unknown() {
            return true;
        }
        if self == expected {
            return true;
        }
        match (self, expected) {
            (Ty::Int, Ty::Float) => true,
            (Ty::String, Ty::Text) => true,
            (Ty::Markdown, Ty::Text) => true,
            (Ty::Optional(actual), Ty::Optional(expected)) => actual.is_assignable_to(expected),
            (actual, Ty::Optional(expected)) => actual.is_assignable_to(expected),
            (Ty::Optional(_), _) => false,
            (Ty::Union(actual), expected) => actual
                .iter()
                .all(|option| option.is_assignable_to(expected)),
            (actual, Ty::Union(expected)) => expected
                .iter()
                .any(|option| actual.is_assignable_to(option)),
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
        match self {
            Ty::Bytes | Ty::File => false,
            Ty::List(element) | Ty::Optional(element) => element.is_renderable(),
            Ty::Union(options) => options.iter().all(Ty::is_renderable),
            _ => true,
        }
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
            Ty::List(element) => write!(f, "{}[]", display_with_precedence(element, 2)),
            Ty::Optional(inner) => write!(f, "{}?", display_with_precedence(inner, 2)),
            Ty::Union(options) => {
                for (index, option) in options.iter().enumerate() {
                    if index > 0 {
                        f.write_str(" | ")?;
                    }
                    write!(f, "{}", display_with_precedence(option, 1))?;
                }
                Ok(())
            }
            Ty::Record(name) => f.write_str(name),
            Ty::Unknown => f.write_str("<unknown>"),
        }
    }
}

fn display_with_precedence(ty: &Ty, parent: u8) -> String {
    let precedence = match ty {
        Ty::Union(_) => 1,
        Ty::List(_) | Ty::Optional(_) => 2,
        _ => 3,
    };
    let rendered = ty.to_string();
    if precedence < parent {
        format!("({rendered})")
    } else {
        rendered
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
    fn string_widens_to_text_but_not_to_markdown() {
        // Language 0.4. A string is text, so it widens the way markdown does.
        assert!(Ty::String.is_assignable_to(&Ty::Text));
        assert!(!Ty::Text.is_assignable_to(&Ty::String));

        // And it stops there, which is the load-bearing half. `markdown` is the
        // more specific type; letting an arbitrary string claim it would leave
        // no ground to refuse `text` either, and then the two are one type with
        // two names. See GAP-045.
        assert!(!Ty::String.is_assignable_to(&Ty::Markdown));
        assert!(!Ty::String.is_assignable_to(&Ty::Json));
        assert!(!Ty::String.is_assignable_to(&Ty::File));
    }

    #[test]
    fn the_widenings_stay_at_three() {
        // A guard on the table rather than on any one entry: every ordered pair
        // of scalar types is checked, so a fourth widening added without a
        // reason breaks this and has to argue for itself.
        let scalars = [
            Ty::String,
            Ty::Int,
            Ty::Float,
            Ty::Bool,
            Ty::Text,
            Ty::Markdown,
            Ty::Json,
            Ty::File,
        ];
        let mut widenings = Vec::new();
        for from in &scalars {
            for to in &scalars {
                if from != to && from.is_assignable_to(to) {
                    widenings.push((format!("{from}"), format!("{to}")));
                }
            }
        }
        widenings.sort();
        assert_eq!(
            widenings,
            vec![
                ("int".to_string(), "float".to_string()),
                ("markdown".to_string(), "text".to_string()),
                ("string".to_string(), "text".to_string()),
            ],
            "the assignability table changed"
        );
    }

    #[test]
    fn lists_are_covariant_only_through_the_same_widening() {
        let ints = Ty::list_of(Ty::Int);
        let floats = Ty::list_of(Ty::Float);
        assert!(ints.is_assignable_to(&floats));
        assert!(!Ty::list_of(Ty::String).is_assignable_to(&floats));
    }

    #[test]
    fn concrete_values_assign_to_optional_slots_but_not_back() {
        let optional_markdown = Ty::optional(Ty::Markdown);
        assert!(Ty::Markdown.is_assignable_to(&optional_markdown));
        assert!(Ty::optional(Ty::Markdown).is_assignable_to(&optional_markdown));
        assert!(!optional_markdown.is_assignable_to(&Ty::Markdown));
    }

    #[test]
    fn union_values_assign_only_when_every_alternative_is_safe() {
        let content = Ty::union(vec![Ty::Markdown, Ty::Text]);
        assert!(Ty::Markdown.is_assignable_to(&content));
        assert!(content.is_assignable_to(&Ty::Text));
        assert!(!Ty::union(vec![Ty::Markdown, Ty::File]).is_assignable_to(&Ty::Text));
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
        assert_eq!(Ty::optional(Ty::String).to_string(), "string?");
        assert_eq!(
            Ty::list_of(Ty::union(vec![Ty::String, Ty::Int])).to_string(),
            "(string | int)[]"
        );
        assert_eq!(
            Ty::Record("search_result".into()).to_string(),
            "search_result"
        );
    }
}
