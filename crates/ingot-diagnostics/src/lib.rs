//! Diagnostic model and renderer.
//!
//! Diagnostics are the product surface of a compiler, so they are a first-class
//! data type rather than formatted strings: every diagnostic carries a stable
//! machine-readable [`code`](Diagnostic::code), a primary label, optional
//! secondary labels and optional notes/help. The terminal renderer and the
//! (future) language server both consume the same structure.

use std::fmt::Write as _;

use ingot_source::{SourceMap, Span};
use serde::Serialize;

pub mod codes;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Compilation cannot produce a valid artifact.
    Error,
    /// Compilation continues, but the program is suspicious or lossy.
    Warning,
    /// Informational; typically explains an implicit compiler action.
    Note,
}

impl Severity {
    pub fn label(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Note => "note",
        }
    }

    fn color(self) -> &'static str {
        match self {
            Severity::Error => "\u{1b}[1;31m",
            Severity::Warning => "\u{1b}[1;33m",
            Severity::Note => "\u{1b}[1;36m",
        }
    }
}

/// A span annotated with an explanatory message.
#[derive(Debug, Clone, Serialize)]
pub struct Label {
    #[serde(skip)]
    pub span: Span,
    pub message: String,
    /// Primary labels are rendered with `^`, secondary ones with `-`.
    pub primary: bool,
}

impl Label {
    pub fn primary(span: Span, message: impl Into<String>) -> Self {
        Label {
            span,
            message: message.into(),
            primary: true,
        }
    }

    pub fn secondary(span: Span, message: impl Into<String>) -> Self {
        Label {
            span,
            message: message.into(),
            primary: false,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Diagnostic {
    /// Stable identifier such as `ING3001`. Never reused for another meaning.
    pub code: &'static str,
    pub severity: Severity,
    pub message: String,
    pub labels: Vec<Label>,
    pub notes: Vec<String>,
    pub help: Option<String>,
}

impl Diagnostic {
    pub fn new(severity: Severity, code: &'static str, message: impl Into<String>) -> Self {
        Diagnostic {
            code,
            severity,
            message: message.into(),
            labels: Vec::new(),
            notes: Vec::new(),
            help: None,
        }
    }

    pub fn error(code: &'static str, message: impl Into<String>) -> Self {
        Diagnostic::new(Severity::Error, code, message)
    }

    pub fn warning(code: &'static str, message: impl Into<String>) -> Self {
        Diagnostic::new(Severity::Warning, code, message)
    }

    pub fn note(code: &'static str, message: impl Into<String>) -> Self {
        Diagnostic::new(Severity::Note, code, message)
    }

    pub fn with_primary(mut self, span: Span, message: impl Into<String>) -> Self {
        self.labels.push(Label::primary(span, message));
        self
    }

    pub fn with_secondary(mut self, span: Span, message: impl Into<String>) -> Self {
        self.labels.push(Label::secondary(span, message));
        self
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }

    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    pub fn primary_span(&self) -> Option<Span> {
        self.labels
            .iter()
            .find(|label| label.primary)
            .or_else(|| self.labels.first())
            .map(|label| label.span)
    }
}

/// Collected diagnostics for one compilation.
#[derive(Debug, Default)]
pub struct DiagnosticBag {
    diagnostics: Vec<Diagnostic>,
}

impl DiagnosticBag {
    pub fn new() -> Self {
        DiagnosticBag::default()
    }

    pub fn push(&mut self, diagnostic: Diagnostic) {
        self.diagnostics.push(diagnostic);
    }

    pub fn extend(&mut self, other: impl IntoIterator<Item = Diagnostic>) {
        self.diagnostics.extend(other);
    }

    pub fn iter(&self) -> impl Iterator<Item = &Diagnostic> {
        self.diagnostics.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.diagnostics.is_empty()
    }

    pub fn len(&self) -> usize {
        self.diagnostics.len()
    }

    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error)
    }

    pub fn error_count(&self) -> usize {
        self.diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .count()
    }

    pub fn warning_count(&self) -> usize {
        self.diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Warning)
            .count()
    }

    pub fn into_vec(self) -> Vec<Diagnostic> {
        self.diagnostics
    }

    /// Order diagnostics by source position so that output is stable across runs.
    pub fn sort_by_position(&mut self) {
        self.diagnostics.sort_by_key(|d| {
            let span = d.primary_span();
            (
                span.map(|s| s.file.index()).unwrap_or(usize::MAX),
                span.map(|s| s.start).unwrap_or(u32::MAX),
                d.code,
            )
        });
    }
}

impl IntoIterator for DiagnosticBag {
    type Item = Diagnostic;
    type IntoIter = std::vec::IntoIter<Diagnostic>;

    fn into_iter(self) -> Self::IntoIter {
        self.diagnostics.into_iter()
    }
}

/// Whether the renderer emits ANSI escape sequences.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorChoice {
    Always,
    Never,
}

const RESET: &str = "\u{1b}[0m";
const BOLD: &str = "\u{1b}[1m";
const BLUE: &str = "\u{1b}[1;34m";

struct Palette {
    enabled: bool,
}

impl Palette {
    fn severity(&self, severity: Severity) -> &'static str {
        if self.enabled {
            severity.color()
        } else {
            ""
        }
    }

    fn bold(&self) -> &'static str {
        if self.enabled {
            BOLD
        } else {
            ""
        }
    }

    fn gutter(&self) -> &'static str {
        if self.enabled {
            BLUE
        } else {
            ""
        }
    }

    fn reset(&self) -> &'static str {
        if self.enabled {
            RESET
        } else {
            ""
        }
    }
}

/// Render one diagnostic in the familiar `rustc` layout.
///
/// ```text
/// error[ING4001]: tool `web.search` requires capability `network`
///   --> examples/research-agent/main.ing:31:14
///    |
/// 31 |       call web.search(query)
///    |            ^^^^^^^^^^ requires the `network` effect
///    |
///    = help: add `network allow ["example.com"]` to the policy block
/// ```
pub fn render(map: &SourceMap, diagnostic: &Diagnostic, color: ColorChoice) -> String {
    let palette = Palette {
        enabled: color == ColorChoice::Always,
    };
    let mut out = String::new();

    let _ = write!(
        out,
        "{}{}[{}]{}{}: {}{}",
        palette.severity(diagnostic.severity),
        diagnostic.severity.label(),
        diagnostic.code,
        palette.reset(),
        palette.bold(),
        diagnostic.message,
        palette.reset()
    );
    out.push('\n');

    let primary = diagnostic.labels.iter().find(|label| label.primary);
    let anchor = primary.or_else(|| diagnostic.labels.first());

    let gutter_width = diagnostic
        .labels
        .iter()
        .map(|label| map.file(label.span.file).line_col(label.span.start).line)
        .max()
        .map(|line| line.to_string().len())
        .unwrap_or(1);
    let pad = " ".repeat(gutter_width);

    if let Some(anchor) = anchor {
        let _ = writeln!(
            out,
            "{pad}{}-->{} {}",
            palette.gutter(),
            palette.reset(),
            map.location(anchor.span)
        );
    }

    if !diagnostic.labels.is_empty() {
        let _ = writeln!(out, "{pad} {}|{}", palette.gutter(), palette.reset());
    }

    for label in &diagnostic.labels {
        let file = map.file(label.span.file);
        let position = file.line_col(label.span.start);
        let line_text = file.line_text(position.line);
        let line_number = position.line.to_string();
        let number_pad = " ".repeat(gutter_width.saturating_sub(line_number.len()));

        let _ = writeln!(
            out,
            "{number_pad}{}{line_number} |{} {line_text}",
            palette.gutter(),
            palette.reset()
        );

        let underline_char = if label.primary { '^' } else { '-' };
        let line_start = file.line_start(position.line) as usize;
        let label_start = (label.span.start as usize).max(line_start);
        let label_end = (label.span.end as usize).max(label_start);
        let line_end = line_start + line_text.len();
        let clamped_end = label_end.min(line_end);

        let leading = file.text()[line_start..label_start].chars().count();
        let width = file.text()[label_start..clamped_end].chars().count().max(1);

        let _ = write!(
            out,
            "{pad} {}|{} {}{}{}",
            palette.gutter(),
            palette.reset(),
            " ".repeat(leading),
            palette.severity(if label.primary {
                diagnostic.severity
            } else {
                Severity::Note
            }),
            underline_char.to_string().repeat(width)
        );
        if !label.message.is_empty() {
            let _ = write!(out, " {}", label.message);
        }
        let _ = writeln!(out, "{}", palette.reset());
    }

    if !diagnostic.labels.is_empty() && (!diagnostic.notes.is_empty() || diagnostic.help.is_some())
    {
        let _ = writeln!(out, "{pad} {}|{}", palette.gutter(), palette.reset());
    }

    for note in &diagnostic.notes {
        let _ = writeln!(
            out,
            "{pad} {}={} note: {note}",
            palette.gutter(),
            palette.reset()
        );
    }
    if let Some(help) = &diagnostic.help {
        let _ = writeln!(
            out,
            "{pad} {}={} help: {help}",
            palette.gutter(),
            palette.reset()
        );
    }

    out
}

/// Render every diagnostic followed by a one-line summary.
pub fn render_all(map: &SourceMap, bag: &DiagnosticBag, color: ColorChoice) -> String {
    let mut out = String::new();
    for diagnostic in bag.iter() {
        out.push_str(&render(map, diagnostic, color));
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_primary_label_with_caret_underline() {
        let mut map = SourceMap::new();
        let file = map.add_virtual("main.ing", "agent A() -> out<markdown> {\n  flow { }\n}\n");
        let span = Span::new(file, 6, 7);

        let diagnostic = Diagnostic::error(codes::UNRESOLVED_NAME, "cannot find `A`")
            .with_primary(span, "not found in this scope")
            .with_help("declare it first");
        let rendered = render(&map, &diagnostic, ColorChoice::Never);

        assert!(rendered.contains("error[ING2001]: cannot find `A`"));
        assert!(rendered.contains("--> main.ing:1:7"));
        assert!(rendered.contains("^ not found in this scope"));
        assert!(rendered.contains("= help: declare it first"));
    }

    #[test]
    fn sorts_diagnostics_by_source_position() {
        let mut map = SourceMap::new();
        let file = map.add_virtual("main.ing", "0123456789\n");
        let mut bag = DiagnosticBag::new();
        bag.push(
            Diagnostic::error(codes::TYPE_MISMATCH, "second")
                .with_primary(Span::new(file, 5, 6), ""),
        );
        bag.push(
            Diagnostic::error(codes::TYPE_MISMATCH, "first")
                .with_primary(Span::new(file, 1, 2), ""),
        );
        bag.sort_by_position();

        let messages: Vec<_> = bag.iter().map(|d| d.message.as_str()).collect();
        assert_eq!(messages, ["first", "second"]);
    }
}
