//! Editor-neutral language services for `.ing` source.
//!
//! The language server and editor extensions should adapt this crate rather
//! than reimplementing parsing, checking or formatting. Diagnostics come from
//! `ingot-compiler`, then are projected into LSP-style ranges while keeping the
//! original byte spans for exact CLI/editor comparisons.

use std::{io, path::Path};

use ingot_compiler::{
    compile_path, compile_source, format_source as compiler_format_source, Compilation,
};
use ingot_diagnostics::{DiagnosticBag, Severity};
use ingot_source::{SourceFile, SourceMap, Span};
use serde::Serialize;

pub const LANGUAGE_SERVICE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Default, Clone, Copy)]
pub struct LanguageService;

impl LanguageService {
    pub fn new() -> Self {
        LanguageService
    }

    pub fn check_source(&self, name: impl Into<String>, text: impl Into<String>) -> CheckResult {
        CheckResult::from_compilation(compile_source(name, text))
    }

    pub fn check_file(&self, path: impl AsRef<Path>) -> io::Result<CheckResult> {
        compile_path(path).map(CheckResult::from_compilation)
    }

    pub fn format_source(
        &self,
        name: impl Into<String>,
        text: impl Into<String>,
    ) -> DocumentFormatResult {
        let original = text.into();
        let result = compiler_format_source(name, original.clone());
        let diagnostics = collect_diagnostics(&result.sources, &result.diagnostics);
        let edits = match result.formatted {
            Some(formatted) if formatted != original => {
                vec![TextEdit {
                    range: full_document_range(&result.sources),
                    new_text: formatted,
                }]
            }
            _ => Vec::new(),
        };

        DocumentFormatResult {
            schema_version: LANGUAGE_SERVICE_SCHEMA_VERSION,
            diagnostics,
            edits,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckResult {
    pub schema_version: u32,
    pub has_errors: bool,
    pub error_count: usize,
    pub warning_count: usize,
    pub diagnostics: Vec<EditorDiagnostic>,
}

impl CheckResult {
    fn from_compilation(compilation: Compilation) -> Self {
        let diagnostics = collect_diagnostics(&compilation.sources, &compilation.diagnostics);
        CheckResult {
            schema_version: LANGUAGE_SERVICE_SCHEMA_VERSION,
            has_errors: compilation.has_errors(),
            error_count: compilation.error_count(),
            warning_count: compilation.warning_count(),
            diagnostics,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentFormatResult {
    pub schema_version: u32,
    pub diagnostics: Vec<EditorDiagnostic>,
    pub edits: Vec<TextEdit>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditorDiagnostic {
    pub code: String,
    pub severity: Severity,
    pub message: String,
    pub range: SourceRange,
    pub labels: Vec<EditorLabel>,
    pub notes: Vec<String>,
    pub help: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditorLabel {
    pub primary: bool,
    pub message: String,
    pub range: SourceRange,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceRange {
    pub file: String,
    pub start_byte: u32,
    pub end_byte: u32,
    pub range: EditorRange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditorRange {
    pub start: EditorPosition,
    pub end: EditorPosition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditorPosition {
    /// Zero-based line number.
    pub line: u32,
    /// Zero-based UTF-16 code-unit offset, matching LSP's `Position.character`.
    pub character: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TextEdit {
    pub range: SourceRange,
    pub new_text: String,
}

fn collect_diagnostics(map: &SourceMap, diagnostics: &DiagnosticBag) -> Vec<EditorDiagnostic> {
    diagnostics
        .iter()
        .map(|diagnostic| {
            let labels: Vec<EditorLabel> = diagnostic
                .labels
                .iter()
                .map(|label| EditorLabel {
                    primary: label.primary,
                    message: label.message.clone(),
                    range: source_range(map, label.span),
                })
                .collect();
            let range = labels
                .iter()
                .find(|label| label.primary)
                .or_else(|| labels.first())
                .map(|label| label.range.clone())
                .unwrap_or_else(|| zero_range(map));

            EditorDiagnostic {
                code: diagnostic.code.to_string(),
                severity: diagnostic.severity,
                message: diagnostic.message.clone(),
                range,
                labels,
                notes: diagnostic.notes.clone(),
                help: diagnostic.help.clone(),
            }
        })
        .collect()
}

fn source_range(map: &SourceMap, span: Span) -> SourceRange {
    let file = map.file(span.file);
    SourceRange {
        file: file.name().to_string(),
        start_byte: span.start,
        end_byte: span.end,
        range: editor_range(file.text(), span.start, span.end),
    }
}

fn full_document_range(map: &SourceMap) -> SourceRange {
    let file = first_file(map);
    SourceRange {
        file: file.name().to_string(),
        start_byte: 0,
        end_byte: file.text().len() as u32,
        range: editor_range(file.text(), 0, file.text().len() as u32),
    }
}

fn zero_range(map: &SourceMap) -> SourceRange {
    let file = first_file(map);
    SourceRange {
        file: file.name().to_string(),
        start_byte: 0,
        end_byte: 0,
        range: editor_range(file.text(), 0, 0),
    }
}

fn first_file(map: &SourceMap) -> &SourceFile {
    map.files()
        .next()
        .expect("language service results always have a registered source file")
}

fn editor_range(text: &str, start: u32, end: u32) -> EditorRange {
    EditorRange {
        start: position_at_byte_offset(text, start),
        end: position_at_byte_offset(text, end),
    }
}

fn position_at_byte_offset(text: &str, offset: u32) -> EditorPosition {
    let mut offset = (offset as usize).min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }

    let mut line = 0u32;
    let mut line_start = 0usize;
    for (index, ch) in text.char_indices() {
        if index >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            line_start = index + ch.len_utf8();
        }
    }

    let character = text[line_start..offset]
        .chars()
        .map(|ch| ch.len_utf16() as u32)
        .sum();
    EditorPosition { line, character }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use ingot_compiler::compile_source;

    use super::*;

    const BROKEN_SOURCE: &str = r#"language 0.1
package demo

agent Brief(topic: string) -> report<markdown> {
  flow {
    emit report = ask<markdown>("Write about ${topci}")
  }
}
"#;

    fn repo_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("the crate must live two levels below the repository root")
            .to_path_buf()
    }

    #[test]
    fn editor_and_cli_diagnostics_are_identical() {
        let service = LanguageService::new();
        let editor = service.check_source("broken.ing", BROKEN_SOURCE);
        let cli = compile_source("broken.ing", BROKEN_SOURCE);

        let editor_keys: Vec<_> = editor
            .diagnostics
            .iter()
            .map(|diagnostic| {
                (
                    diagnostic.code.as_str(),
                    diagnostic.range.start_byte,
                    diagnostic.range.end_byte,
                    diagnostic.message.as_str(),
                )
            })
            .collect();
        let cli_keys: Vec<_> = cli
            .diagnostics
            .iter()
            .map(|diagnostic| {
                let span = diagnostic
                    .primary_span()
                    .expect("compiler diagnostics used by editors must have a span");
                (
                    diagnostic.code,
                    span.start,
                    span.end,
                    diagnostic.message.as_str(),
                )
            })
            .collect();

        assert_eq!(editor_keys, cli_keys);
    }

    #[test]
    fn formatting_returns_a_full_document_edit() {
        let service = LanguageService::new();
        let source = r#"language 0.1
package demo

agent Brief(topic:string)->report<markdown>{flow{emit report=ask<markdown>("ok")}}
"#;

        let result = service.format_source("main.ing", source);

        assert!(result.diagnostics.is_empty());
        assert_eq!(result.edits.len(), 1);
        assert_eq!(result.edits[0].range.start_byte, 0);
        assert_eq!(result.edits[0].range.end_byte, source.len() as u32);
        assert!(result.edits[0]
            .new_text
            .contains("agent Brief(topic: string)"));
    }

    #[test]
    fn syntax_errors_return_diagnostics_without_format_edits() {
        let service = LanguageService::new();
        let result = service.format_source("broken.ing", "language 0.1\nagent\n");

        assert!(!result.diagnostics.is_empty());
        assert!(result.edits.is_empty());
    }

    #[test]
    fn reference_examples_are_clean_through_the_language_service() {
        let service = LanguageService::new();
        for example in [
            "document-summarizer",
            "research-agent",
            "code-review-team",
            "repo-digest",
        ] {
            let entry = repo_root().join("examples").join(example).join("main.ing");
            let result = service
                .check_file(&entry)
                .unwrap_or_else(|error| panic!("{} must be readable: {error}", entry.display()));
            assert!(
                !result.has_errors,
                "{example} should be clean through the language service"
            );
        }
    }

    #[test]
    fn positions_are_zero_based_and_utf16_encoded() {
        let text = format!("a{}\nb", '\u{1f600}');
        let newline = text.find('\n').expect("test text has a newline") as u32;

        assert_eq!(
            position_at_byte_offset(&text, newline),
            EditorPosition {
                line: 0,
                character: 3
            }
        );
        assert_eq!(
            position_at_byte_offset(&text, newline + 1),
            EditorPosition {
                line: 1,
                character: 0
            }
        );
    }
}
