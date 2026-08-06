//! String literal processing: escape resolution and `${...}` interpolation.
//!
//! Operates on the raw source text between the quotes so that every
//! interpolation keeps an exact span. The checker relies on those spans to point
//! at the offending placeholder rather than at the whole prompt.

use ingot_diagnostics::{codes, Diagnostic};
use ingot_source::Span;
use ingot_syntax::{Ident, InterpolationPath, PathRoot, StringLit, StringPart};

/// Resolve escapes only, for strings that are never interpolated.
pub fn unescape(raw: &str) -> String {
    let mut out = String::new();
    let mut chars = raw.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('\\') => out.push('\\'),
            Some('"') => out.push('"'),
            Some('$') => out.push('$'),
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

/// Split a raw literal into literal text and interpolation paths.
///
/// `span` is the span of the whole token, quotes included, so inner offsets are
/// `span.start + 1 + index`.
pub fn parse_string_literal(raw: &str, span: Span) -> (StringLit, Vec<Diagnostic>) {
    let base = span.start + 1;
    let bytes = raw.as_bytes();
    let mut parts: Vec<StringPart> = Vec::new();
    let mut diagnostics = Vec::new();
    let mut literal = String::new();
    let mut index = 0usize;

    while index < bytes.len() {
        if bytes[index] == b'\\' && index + 1 < bytes.len() {
            let escaped = bytes[index + 1] as char;
            match escaped {
                'n' => literal.push('\n'),
                'r' => literal.push('\r'),
                't' => literal.push('\t'),
                '\\' => literal.push('\\'),
                '"' => literal.push('"'),
                '$' => literal.push('$'),
                other => literal.push(other),
            }
            index += 2;
            continue;
        }

        if bytes[index] == b'$' && bytes.get(index + 1) == Some(&b'{') {
            let start = index;
            let Some(close) = raw[index + 2..].find('}').map(|offset| index + 2 + offset) else {
                // The lexer already reported the unterminated placeholder.
                literal.push_str(&raw[index..]);
                break;
            };
            if !literal.is_empty() {
                parts.push(StringPart::Literal(std::mem::take(&mut literal)));
            }
            let inner = &raw[index + 2..close];
            let inner_span = Span::new(span.file, base + start as u32, base + close as u32 + 1);
            match parse_interpolation_path(inner, span, base + (index + 2) as u32, inner_span) {
                Ok(path) => parts.push(StringPart::Interpolation(path)),
                Err(diagnostic) => {
                    diagnostics.push(diagnostic);
                    parts.push(StringPart::Literal(format!("${{{inner}}}")));
                }
            }
            index = close + 1;
            continue;
        }

        let ch = raw[index..]
            .chars()
            .next()
            .expect("index is on a char boundary");
        literal.push(ch);
        index += ch.len_utf8();
    }

    if !literal.is_empty() || parts.is_empty() {
        parts.push(StringPart::Literal(literal));
    }

    (StringLit { parts, span }, diagnostics)
}

/// Parse `name`, `name.field` or `state.field` inside a placeholder.
fn parse_interpolation_path(
    inner: &str,
    _literal_span: Span,
    inner_start: u32,
    placeholder_span: Span,
) -> Result<InterpolationPath, Diagnostic> {
    let trimmed = inner.trim();
    if trimmed.is_empty() {
        return Err(Diagnostic::error(
            codes::UNRESOLVED_INTERPOLATION,
            "empty interpolation placeholder",
        )
        .with_primary(placeholder_span, "expected a name")
        .with_help("write `${topic}` to insert the value of `topic`"));
    }

    let leading = inner.len() - inner.trim_start().len();
    let mut offset = inner_start + leading as u32;
    let mut segments: Vec<Ident> = Vec::new();

    for (index, raw_segment) in trimmed.split('.').enumerate() {
        if !is_valid_ident(raw_segment) {
            return Err(Diagnostic::error(
                codes::UNRESOLVED_INTERPOLATION,
                format!("`{trimmed}` is not a valid interpolation path"),
            )
            .with_primary(placeholder_span, "expected a name or `name.field`")
            .with_note("interpolations may only read values; call tools in the flow instead"));
        }
        let segment_span = Span::new(
            placeholder_span.file,
            offset,
            offset + raw_segment.len() as u32,
        );
        segments.push(Ident::new(raw_segment, segment_span));
        offset += raw_segment.len() as u32 + 1; // segment plus the `.`
        let _ = index;
    }

    let first = segments.remove(0);
    let root = if first.text == "state" {
        PathRoot::State { span: first.span }
    } else {
        PathRoot::Binding(first)
    };

    Ok(InterpolationPath {
        root,
        segments,
        span: placeholder_span,
    })
}

fn is_valid_ident(text: &str) -> bool {
    let mut chars = text.chars();
    match chars.next() {
        Some(ch) if ch == '_' || ch.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ingot_source::{FileId, SourceMap};

    fn span_of(text: &str) -> (Span, FileId) {
        let mut map = SourceMap::new();
        let file = map.add_virtual("test.ing", text);
        (Span::new(file, 0, text.len() as u32), file)
    }

    #[test]
    fn splits_literal_and_interpolation_parts() {
        let source = "\"Research: ${topic} now\"";
        let (span, _) = span_of(source);
        let (literal, diagnostics) = parse_string_literal("Research: ${topic} now", span);

        assert!(diagnostics.is_empty());
        assert_eq!(literal.parts.len(), 3);
        assert_eq!(literal.template(), "Research: ${topic} now");
        assert_eq!(literal.plain_text(), "Research:  now");
    }

    #[test]
    fn gives_each_placeholder_its_own_span() {
        let source = "\"a ${topic} b\"";
        let (span, _) = span_of(source);
        let (literal, _) = parse_string_literal("a ${topic} b", span);

        let StringPart::Interpolation(path) = &literal.parts[1] else {
            panic!("expected an interpolation part");
        };
        // `${topic}` starts at index 2 of the inner text, so at offset 3 in source.
        assert_eq!(path.span.start, 3);
        assert_eq!(path.span.end, 11);
    }

    #[test]
    fn resolves_escapes() {
        let source = "\"line\\nnext\"";
        let (span, _) = span_of(source);
        let (literal, diagnostics) = parse_string_literal("line\\nnext", span);
        assert!(diagnostics.is_empty());
        assert_eq!(literal.plain_text(), "line\nnext");
    }

    #[test]
    fn escaped_dollar_is_not_an_interpolation() {
        let source = "\"cost: \\${5}\"";
        let (span, _) = span_of(source);
        let (literal, diagnostics) = parse_string_literal("cost: \\${5}", span);
        assert!(diagnostics.is_empty());
        assert!(literal.is_plain());
        assert_eq!(literal.plain_text(), "cost: ${5}");
    }

    #[test]
    fn rejects_expressions_inside_placeholders() {
        let source = "\"${len(x)}\"";
        let (span, _) = span_of(source);
        let (_, diagnostics) = parse_string_literal("${len(x)}", span);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, codes::UNRESOLVED_INTERPOLATION);
    }

    #[test]
    fn state_is_recognised_as_an_interpolation_root() {
        let source = "\"${state.notes}\"";
        let (span, _) = span_of(source);
        let (literal, diagnostics) = parse_string_literal("${state.notes}", span);
        assert!(diagnostics.is_empty());
        let StringPart::Interpolation(path) = &literal.parts[0] else {
            panic!("expected an interpolation part");
        };
        assert!(matches!(path.root, PathRoot::State { .. }));
        assert_eq!(path.text(), "state.notes");
    }
}
