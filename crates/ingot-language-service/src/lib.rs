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
use ingot_syntax::{
    AgentDecl, Arg, BudgetLimit, DottedName, Expr, FieldDecl, FlowBlock, Ident, InterpolationPath,
    ModelRequirement, OutputDecl, PathRoot, PolicyAction, PolicyRule, Program, Stmt, StringLit,
    StringPart, ToolDecl, TypeDecl, TypeExpr, VerifierDecl,
};
use ingot_types::{PolicySubject, MODEL_CAPABILITIES};
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

    pub fn completion_items(
        &self,
        name: impl Into<String>,
        text: impl Into<String>,
        position: EditorPosition,
    ) -> CompletionResult {
        let text = text.into();
        let compilation = compile_source(name, text.clone());
        let diagnostics = collect_diagnostics(&compilation.sources, &compilation.diagnostics);
        let prefix = word_at_position(&text, position).unwrap_or_default();
        let symbols = collect_symbols(&compilation);
        let mut items = builtin_completion_items();

        for symbol in symbols {
            items.push(CompletionItem {
                label: symbol.name,
                kind: completion_kind_for_symbol(symbol.kind),
                detail: Some(symbol.detail),
                documentation: symbol.documentation,
                insert_text: None,
            });
        }

        if !prefix.is_empty() {
            items.retain(|item| item.label.starts_with(&prefix) || item.label.contains(&prefix));
        }
        items.sort_by(|left, right| {
            completion_sort_key(left.kind, &left.label)
                .cmp(&completion_sort_key(right.kind, &right.label))
        });
        items.dedup_by(|left, right| left.label == right.label && left.kind == right.kind);

        CompletionResult {
            schema_version: LANGUAGE_SERVICE_SCHEMA_VERSION,
            diagnostics,
            items,
        }
    }

    pub fn hover(
        &self,
        name: impl Into<String>,
        text: impl Into<String>,
        position: EditorPosition,
    ) -> HoverResult {
        let text = text.into();
        let compilation = compile_source(name, text.clone());
        let diagnostics = collect_diagnostics(&compilation.sources, &compilation.diagnostics);
        let offset = byte_offset_for_position(&text, position);
        let word = word_at_byte_offset(&text, offset).unwrap_or_default();
        let symbols = collect_symbols(&compilation);

        let hover = symbols
            .iter()
            .find(|symbol| contains_byte(&symbol.name_range, offset))
            .or_else(|| {
                (!word.is_empty())
                    .then(|| symbols.iter().find(|symbol| symbol.name == word))
                    .flatten()
            })
            .map(|symbol| HoverItem {
                contents: hover_markdown(symbol),
                range: symbol.name_range.clone(),
            })
            .or_else(|| keyword_hover(&word, &compilation.sources, &text, offset));

        HoverResult {
            schema_version: LANGUAGE_SERVICE_SCHEMA_VERSION,
            diagnostics,
            hover,
        }
    }

    pub fn definition(
        &self,
        name: impl Into<String>,
        text: impl Into<String>,
        position: EditorPosition,
    ) -> DefinitionResult {
        let text = text.into();
        let compilation = compile_source(name, text.clone());
        let diagnostics = collect_diagnostics(&compilation.sources, &compilation.diagnostics);
        let offset = byte_offset_for_position(&text, position);
        let word = word_at_byte_offset(&text, offset).unwrap_or_default();
        let symbols = collect_symbols(&compilation);

        let definition = symbols
            .iter()
            .filter(|symbol| is_definition_target(symbol))
            .find(|symbol| contains_byte(&symbol.name_range, offset))
            .or_else(|| {
                (!word.is_empty())
                    .then(|| {
                        symbols
                            .iter()
                            .filter(|symbol| is_definition_target(symbol))
                            .find(|symbol| symbol.name == word)
                    })
                    .flatten()
            })
            .map(|symbol| DefinitionItem {
                name: symbol.name.clone(),
                kind: symbol.kind,
                target: symbol.name_range.clone(),
                declaration: symbol.declaration_range.clone(),
            });

        DefinitionResult {
            schema_version: LANGUAGE_SERVICE_SCHEMA_VERSION,
            diagnostics,
            definition,
        }
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
pub struct CompletionResult {
    pub schema_version: u32,
    pub diagnostics: Vec<EditorDiagnostic>,
    pub items: Vec<CompletionItem>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionItem {
    pub label: String,
    pub kind: CompletionKind,
    pub detail: Option<String>,
    pub documentation: Option<String>,
    pub insert_text: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CompletionKind {
    Keyword,
    Type,
    Tool,
    Verifier,
    Agent,
    Field,
    Binding,
    Output,
    Builtin,
    Section,
    Value,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HoverResult {
    pub schema_version: u32,
    pub diagnostics: Vec<EditorDiagnostic>,
    pub hover: Option<HoverItem>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HoverItem {
    pub contents: String,
    pub range: SourceRange,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DefinitionResult {
    pub schema_version: u32,
    pub diagnostics: Vec<EditorDiagnostic>,
    pub definition: Option<DefinitionItem>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DefinitionItem {
    pub name: String,
    pub kind: SymbolKind,
    pub target: SourceRange,
    pub declaration: SourceRange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SymbolKind {
    Package,
    Type,
    Tool,
    Verifier,
    Agent,
    Field,
    Parameter,
    State,
    Output,
    Binding,
    LoopVariable,
    ModelCapability,
    BudgetKey,
    PolicySubject,
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

fn byte_offset_for_position(text: &str, position: EditorPosition) -> u32 {
    let mut line = 0u32;
    let mut line_start = 0usize;
    for (index, ch) in text.char_indices() {
        if line == position.line {
            break;
        }
        if ch == '\n' {
            line += 1;
            line_start = index + ch.len_utf8();
        }
    }

    if line < position.line {
        return text.len() as u32;
    }

    let mut utf16 = 0u32;
    for (relative, ch) in text[line_start..].char_indices() {
        if ch == '\n' || utf16 >= position.character {
            return (line_start + relative) as u32;
        }
        let width = ch.len_utf16() as u32;
        if utf16 + width > position.character {
            return (line_start + relative) as u32;
        }
        utf16 += width;
    }

    text.len() as u32
}

#[derive(Debug, Clone)]
struct EditorSymbol {
    name: String,
    kind: SymbolKind,
    detail: String,
    documentation: Option<String>,
    name_range: SourceRange,
    declaration_range: SourceRange,
}

fn collect_symbols(compilation: &Compilation) -> Vec<EditorSymbol> {
    let mut symbols = Vec::new();
    add_program_symbols(&compilation.sources, &compilation.program, &mut symbols);
    symbols
}

fn add_program_symbols(map: &SourceMap, program: &Program, symbols: &mut Vec<EditorSymbol>) {
    if let Some(package) = &program.package {
        symbols.push(symbol_from_dotted(
            map,
            package,
            package.span,
            SymbolKind::Package,
            format!("package {}", package.text()),
            None,
        ));
    }

    for decl in &program.types {
        add_type_symbols(map, decl, symbols);
    }
    for decl in &program.tools {
        add_tool_symbols(map, decl, symbols);
    }
    for decl in &program.verifiers {
        add_verifier_symbols(map, decl, symbols);
    }
    for decl in &program.agents {
        add_agent_symbols(map, decl, symbols);
    }
}

fn add_type_symbols(map: &SourceMap, decl: &TypeDecl, symbols: &mut Vec<EditorSymbol>) {
    symbols.push(symbol_from_ident(
        map,
        &decl.name,
        decl.span,
        SymbolKind::Type,
        format!("type {} {{ ... }}", decl.name.text),
        decl.doc.clone(),
    ));
    for field in &decl.fields {
        add_field_symbol(map, field, SymbolKind::Field, symbols);
        add_type_reference(map, &field.ty, symbols);
    }
}

fn add_tool_symbols(map: &SourceMap, decl: &ToolDecl, symbols: &mut Vec<EditorSymbol>) {
    symbols.push(symbol_from_dotted(
        map,
        &decl.name,
        decl.span,
        SymbolKind::Tool,
        format!(
            "tool {}({}) -> {}{}",
            decl.name.text(),
            params_text(&decl.params),
            decl.ret.text(),
            effects_text(&decl.effects),
        ),
        decl.doc.clone(),
    ));
    for param in &decl.params {
        add_field_symbol(map, param, SymbolKind::Parameter, symbols);
        add_type_reference(map, &param.ty, symbols);
    }
    add_type_reference(map, &decl.ret, symbols);
}

fn add_verifier_symbols(map: &SourceMap, decl: &VerifierDecl, symbols: &mut Vec<EditorSymbol>) {
    symbols.push(symbol_from_ident(
        map,
        &decl.name,
        decl.span,
        SymbolKind::Verifier,
        format!("verifier {}({})", decl.name.text, params_text(&decl.params)),
        decl.doc.clone(),
    ));
    for param in &decl.params {
        add_field_symbol(map, param, SymbolKind::Parameter, symbols);
        add_type_reference(map, &param.ty, symbols);
    }
}

fn add_agent_symbols(map: &SourceMap, decl: &AgentDecl, symbols: &mut Vec<EditorSymbol>) {
    symbols.push(symbol_from_ident(
        map,
        &decl.name,
        decl.span,
        SymbolKind::Agent,
        format!(
            "agent {}({}){}",
            decl.name.text,
            params_text(&decl.params),
            decl.output
                .as_ref()
                .map(|output| format!(" -> {}<{}>", output.name.text, output.content.text))
                .unwrap_or_default()
        ),
        decl.doc.clone(),
    ));
    for param in &decl.params {
        add_field_symbol(map, param, SymbolKind::Parameter, symbols);
        add_type_reference(map, &param.ty, symbols);
    }
    if let Some(output) = &decl.output {
        add_output_symbol(map, output, symbols);
    }
    if let Some(model) = &decl.model {
        match model {
            ingot_syntax::ModelBlock::Requires { requirements, .. } => {
                for requirement in requirements {
                    if let ModelRequirement::Capability(ident) = requirement {
                        symbols.push(symbol_from_ident(
                            map,
                            ident,
                            ident.span,
                            SymbolKind::ModelCapability,
                            format!("model capability {}", ident.text),
                            Some("Capability the runtime model must provide.".to_string()),
                        ));
                    }
                }
            }
            ingot_syntax::ModelBlock::Exact { .. } => {}
        }
    }
    if let Some(tools) = &decl.tools {
        for grant in &tools.grants {
            symbols.push(symbol_from_dotted(
                map,
                &grant.name,
                grant.span,
                SymbolKind::Tool,
                format!(
                    "tool grant {} via {}",
                    grant.name.text(),
                    grant.transport.text
                ),
                None,
            ));
        }
    }
    if let Some(memory) = &decl.memory {
        if let Some(working) = &memory.working {
            for field in &working.fields {
                add_field_symbol(map, field, SymbolKind::State, symbols);
            }
        }
    }
    if let Some(budget) = &decl.budget {
        for limit in &budget.limits {
            add_budget_limit_symbol(map, limit, symbols);
        }
    }
    if let Some(policy) = &decl.policy {
        for rule in &policy.rules {
            add_policy_rule_symbol(map, rule, symbols);
        }
    }
    if let Some(flow) = &decl.flow {
        add_flow_symbols(map, flow, symbols);
    }
}

fn add_flow_symbols(map: &SourceMap, flow: &FlowBlock, symbols: &mut Vec<EditorSymbol>) {
    add_statement_symbols(map, &flow.statements, symbols);
}

fn add_statement_symbols(map: &SourceMap, statements: &[Stmt], symbols: &mut Vec<EditorSymbol>) {
    for statement in statements {
        match statement {
            Stmt::Bind { name, value, span } => {
                symbols.push(symbol_from_ident(
                    map,
                    name,
                    *span,
                    SymbolKind::Binding,
                    format!("binding {}", name.text),
                    Some("Value bound inside this flow.".to_string()),
                ));
                add_expr_symbols(map, value, symbols);
            }
            Stmt::StateWrite { field, value, .. } => {
                symbols.push(symbol_from_ident(
                    map,
                    field,
                    field.span,
                    SymbolKind::State,
                    format!("state.{}", field.text),
                    None,
                ));
                add_expr_symbols(map, value, symbols);
            }
            Stmt::Expr { value, .. } => add_expr_symbols(map, value, symbols),
            Stmt::Verify {
                validator, args, ..
            } => {
                symbols.push(symbol_from_ident(
                    map,
                    validator,
                    validator.span,
                    SymbolKind::Verifier,
                    format!("verify {}", validator.text),
                    None,
                ));
                add_arg_symbols(map, args, symbols);
            }
            Stmt::Emit { output, value, .. } => {
                symbols.push(symbol_from_ident(
                    map,
                    output,
                    output.span,
                    SymbolKind::Output,
                    format!("emit {}", output.text),
                    None,
                ));
                add_expr_symbols(map, value, symbols);
            }
            Stmt::If {
                condition,
                then_branch,
                else_branch,
                ..
            } => {
                add_expr_symbols(map, condition, symbols);
                add_statement_symbols(map, then_branch, symbols);
                if let Some(else_branch) = else_branch {
                    add_statement_symbols(map, else_branch, symbols);
                }
            }
            Stmt::Loop { guard, body, .. } => {
                if let Some(guard) = guard {
                    add_expr_symbols(map, guard, symbols);
                }
                add_statement_symbols(map, body, symbols);
            }
            Stmt::Checkpoint { label, .. } => add_string_symbols(map, label, symbols),
            Stmt::Error { .. } => {}
        }
    }
}

fn add_expr_symbols(map: &SourceMap, expr: &Expr, symbols: &mut Vec<EditorSymbol>) {
    match expr {
        Expr::Str(literal) => add_string_symbols(map, literal, symbols),
        Expr::List { items, .. } => {
            for item in items {
                add_expr_symbols(map, item, symbols);
            }
        }
        Expr::Path(path) => {
            match &path.root {
                PathRoot::Binding(ident) => symbols.push(symbol_from_ident(
                    map,
                    ident,
                    path.span,
                    SymbolKind::Binding,
                    format!("binding {}", ident.text),
                    None,
                )),
                PathRoot::State { span } => symbols.push(EditorSymbol {
                    name: "state".to_string(),
                    kind: SymbolKind::State,
                    detail: "state".to_string(),
                    documentation: Some("Agent working memory root.".to_string()),
                    name_range: source_range(map, *span),
                    declaration_range: source_range(map, path.span),
                }),
            }
            for segment in &path.segments {
                symbols.push(symbol_from_ident(
                    map,
                    segment,
                    segment.span,
                    SymbolKind::Field,
                    format!("field {}", segment.text),
                    None,
                ));
            }
        }
        Expr::Ask { result, args, .. } => {
            add_type_reference(map, result, symbols);
            add_arg_symbols(map, args, symbols);
        }
        Expr::Call { callee, args, span } => {
            symbols.push(symbol_from_dotted(
                map,
                callee,
                *span,
                SymbolKind::Tool,
                format!("call {}", callee.text()),
                None,
            ));
            add_arg_symbols(map, args, symbols);
        }
        Expr::ParallelMap {
            source,
            binder,
            body,
            span,
        } => {
            add_expr_symbols(map, source, symbols);
            symbols.push(symbol_from_ident(
                map,
                binder,
                *span,
                SymbolKind::LoopVariable,
                format!("loop variable {}", binder.text),
                None,
            ));
            add_statement_symbols(map, body, symbols);
        }
        Expr::Builtin { name, args, .. } => {
            symbols.push(symbol_from_ident(
                map,
                name,
                name.span,
                SymbolKind::Binding,
                format!("builtin {}", name.text),
                builtin_doc(&name.text).map(str::to_string),
            ));
            for arg in args {
                add_expr_symbols(map, arg, symbols);
            }
        }
        Expr::Unary { operand, .. } => add_expr_symbols(map, operand, symbols),
        Expr::Binary { lhs, rhs, .. } => {
            add_expr_symbols(map, lhs, symbols);
            add_expr_symbols(map, rhs, symbols);
        }
        Expr::Int { .. } | Expr::Float { .. } | Expr::Bool { .. } | Expr::Error { .. } => {}
    }
}

fn add_arg_symbols(map: &SourceMap, args: &[Arg], symbols: &mut Vec<EditorSymbol>) {
    for arg in args {
        if let Some(name) = &arg.name {
            symbols.push(symbol_from_ident(
                map,
                name,
                arg.span,
                SymbolKind::Parameter,
                format!("argument {}", name.text),
                None,
            ));
        }
        add_expr_symbols(map, &arg.value, symbols);
    }
}

fn add_string_symbols(map: &SourceMap, literal: &StringLit, symbols: &mut Vec<EditorSymbol>) {
    for part in &literal.parts {
        if let StringPart::Interpolation(path) = part {
            add_interpolation_symbols(map, path, symbols);
        }
    }
}

fn add_interpolation_symbols(
    map: &SourceMap,
    path: &InterpolationPath,
    symbols: &mut Vec<EditorSymbol>,
) {
    match &path.root {
        PathRoot::Binding(ident) => symbols.push(symbol_from_ident(
            map,
            ident,
            path.span,
            SymbolKind::Binding,
            format!("binding {}", ident.text),
            None,
        )),
        PathRoot::State { span } => symbols.push(EditorSymbol {
            name: "state".to_string(),
            kind: SymbolKind::State,
            detail: "state".to_string(),
            documentation: Some("Agent working memory root.".to_string()),
            name_range: source_range(map, *span),
            declaration_range: source_range(map, path.span),
        }),
    }
    for segment in &path.segments {
        symbols.push(symbol_from_ident(
            map,
            segment,
            segment.span,
            SymbolKind::Field,
            format!("field {}", segment.text),
            None,
        ));
    }
}

fn add_type_reference(map: &SourceMap, ty: &TypeExpr, symbols: &mut Vec<EditorSymbol>) {
    match ty {
        TypeExpr::Named(ident) => symbols.push(symbol_from_ident(
            map,
            ident,
            ident.span,
            SymbolKind::Type,
            format!("type {}", ident.text),
            primitive_type_doc(&ident.text).map(str::to_string),
        )),
        TypeExpr::List { element, .. } => add_type_reference(map, element, symbols),
    }
}

fn add_field_symbol(
    map: &SourceMap,
    field: &FieldDecl,
    kind: SymbolKind,
    symbols: &mut Vec<EditorSymbol>,
) {
    symbols.push(symbol_from_ident(
        map,
        &field.name,
        field.span,
        kind,
        format!("{}: {}", field.name.text, field.ty.text()),
        None,
    ));
}

fn add_output_symbol(map: &SourceMap, output: &OutputDecl, symbols: &mut Vec<EditorSymbol>) {
    symbols.push(symbol_from_ident(
        map,
        &output.name,
        output.span,
        SymbolKind::Output,
        format!("output {}<{}>", output.name.text, output.content.text),
        None,
    ));
    symbols.push(symbol_from_ident(
        map,
        &output.content,
        output.content.span,
        SymbolKind::Type,
        format!("content type {}", output.content.text),
        primitive_type_doc(&output.content.text).map(str::to_string),
    ));
}

fn add_budget_limit_symbol(map: &SourceMap, limit: &BudgetLimit, symbols: &mut Vec<EditorSymbol>) {
    symbols.push(symbol_from_ident(
        map,
        &limit.key,
        limit.span,
        SymbolKind::BudgetKey,
        format!("budget key {}", limit.key.text),
        Some("Budget limits constrain static steps, model tokens or cost.".to_string()),
    ));
}

fn add_policy_rule_symbol(map: &SourceMap, rule: &PolicyRule, symbols: &mut Vec<EditorSymbol>) {
    symbols.push(symbol_from_ident(
        map,
        &rule.subject,
        rule.span,
        SymbolKind::PolicySubject,
        format!("policy subject {}", rule.subject.text),
        Some("Policy subjects map capabilities to allow/deny/approval decisions.".to_string()),
    ));
    match &rule.action {
        PolicyAction::Allow {
            qualifier: Some(qualifier),
            ..
        }
        | PolicyAction::Deny {
            qualifier: Some(qualifier),
            ..
        } => symbols.push(symbol_from_ident(
            map,
            qualifier,
            qualifier.span,
            SymbolKind::PolicySubject,
            format!("policy qualifier {}", qualifier.text),
            None,
        )),
        PolicyAction::Allow { .. }
        | PolicyAction::Deny { .. }
        | PolicyAction::RequireApproval { .. } => {}
    }
}

fn symbol_from_ident(
    map: &SourceMap,
    ident: &Ident,
    declaration_span: Span,
    kind: SymbolKind,
    detail: String,
    documentation: Option<String>,
) -> EditorSymbol {
    EditorSymbol {
        name: ident.text.clone(),
        kind,
        detail,
        documentation,
        name_range: source_range(map, ident.span),
        declaration_range: source_range(map, declaration_span),
    }
}

fn symbol_from_dotted(
    map: &SourceMap,
    name: &DottedName,
    declaration_span: Span,
    kind: SymbolKind,
    detail: String,
    documentation: Option<String>,
) -> EditorSymbol {
    EditorSymbol {
        name: name.text(),
        kind,
        detail,
        documentation,
        name_range: source_range(map, name.span),
        declaration_range: source_range(map, declaration_span),
    }
}

fn params_text(params: &[FieldDecl]) -> String {
    params
        .iter()
        .map(|param| format!("{}: {}", param.name.text, param.ty.text()))
        .collect::<Vec<_>>()
        .join(", ")
}

fn effects_text(effects: &[Ident]) -> String {
    if effects.is_empty() {
        String::new()
    } else {
        format!(
            " {}",
            effects
                .iter()
                .map(|effect| format!("!{}", effect.text))
                .collect::<Vec<_>>()
                .join(" ")
        )
    }
}

fn builtin_completion_items() -> Vec<CompletionItem> {
    let mut items = Vec::new();
    for (label, detail, doc, insert_text) in KEYWORD_COMPLETIONS {
        items.push(CompletionItem {
            label: (*label).to_string(),
            kind: CompletionKind::Keyword,
            detail: Some((*detail).to_string()),
            documentation: Some((*doc).to_string()),
            insert_text: insert_text.map(str::to_string),
        });
    }
    for ty in PRIMITIVE_TYPES {
        items.push(CompletionItem {
            label: (*ty).to_string(),
            kind: CompletionKind::Type,
            detail: Some(format!("built-in type {ty}")),
            documentation: primitive_type_doc(ty).map(str::to_string),
            insert_text: None,
        });
    }
    for capability in MODEL_CAPABILITIES {
        items.push(CompletionItem {
            label: (*capability).to_string(),
            kind: CompletionKind::Value,
            detail: Some("model capability".to_string()),
            documentation: Some("Capability a selected model must support.".to_string()),
            insert_text: None,
        });
    }
    for subject in PolicySubject::all() {
        items.push(CompletionItem {
            label: subject.as_str().to_string(),
            kind: CompletionKind::Value,
            detail: Some("policy subject".to_string()),
            documentation: Some(format!(
                "Policy subject for the `{}` capability.",
                subject.effect()
            )),
            insert_text: None,
        });
    }
    for (label, detail, doc) in BUILTIN_FUNCTIONS {
        items.push(CompletionItem {
            label: (*label).to_string(),
            kind: CompletionKind::Builtin,
            detail: Some((*detail).to_string()),
            documentation: Some((*doc).to_string()),
            insert_text: None,
        });
    }
    items
}

fn completion_kind_for_symbol(kind: SymbolKind) -> CompletionKind {
    match kind {
        SymbolKind::Package => CompletionKind::Section,
        SymbolKind::Type => CompletionKind::Type,
        SymbolKind::Tool => CompletionKind::Tool,
        SymbolKind::Verifier => CompletionKind::Verifier,
        SymbolKind::Agent => CompletionKind::Agent,
        SymbolKind::Field => CompletionKind::Field,
        SymbolKind::Parameter => CompletionKind::Binding,
        SymbolKind::State => CompletionKind::Field,
        SymbolKind::Output => CompletionKind::Output,
        SymbolKind::Binding | SymbolKind::LoopVariable => CompletionKind::Binding,
        SymbolKind::ModelCapability | SymbolKind::BudgetKey | SymbolKind::PolicySubject => {
            CompletionKind::Value
        }
    }
}

fn completion_sort_key(kind: CompletionKind, label: &str) -> (u8, String) {
    let rank = match kind {
        CompletionKind::Keyword => 0,
        CompletionKind::Section => 1,
        CompletionKind::Agent => 2,
        CompletionKind::Tool => 3,
        CompletionKind::Verifier => 4,
        CompletionKind::Type => 5,
        CompletionKind::Binding => 6,
        CompletionKind::Field => 7,
        CompletionKind::Output => 8,
        CompletionKind::Builtin => 9,
        CompletionKind::Value => 10,
    };
    (rank, label.to_string())
}

fn hover_markdown(symbol: &EditorSymbol) -> String {
    let mut contents = format!("```ingot\n{}\n```", symbol.detail);
    if let Some(doc) = &symbol.documentation {
        contents.push_str("\n\n");
        contents.push_str(doc);
    }
    contents
}

fn keyword_hover(word: &str, map: &SourceMap, text: &str, offset: u32) -> Option<HoverItem> {
    let (_, detail, doc, _) = KEYWORD_COMPLETIONS
        .iter()
        .find(|(label, _, _, _)| *label == word)?;
    let range = word_range_at_byte_offset(text, offset).unwrap_or((offset, offset));
    Some(HoverItem {
        contents: format!("```ingot\n{}\n```\n\n{}", detail, doc),
        range: source_range(map, Span::new(first_file(map).id(), range.0, range.1)),
    })
}

fn word_at_position(text: &str, position: EditorPosition) -> Option<String> {
    let offset = byte_offset_for_position(text, position);
    word_at_byte_offset(text, offset)
}

fn word_at_byte_offset(text: &str, offset: u32) -> Option<String> {
    let (start, end) = word_range_at_byte_offset(text, offset)?;
    Some(text[start as usize..end as usize].to_string())
}

fn word_range_at_byte_offset(text: &str, offset: u32) -> Option<(u32, u32)> {
    if text.is_empty() {
        return None;
    }
    let mut offset = (offset as usize).min(text.len());
    while offset > 0 && !text.is_char_boundary(offset) {
        offset -= 1;
    }
    if offset == text.len() && offset > 0 {
        offset -= 1;
        while offset > 0 && !text.is_char_boundary(offset) {
            offset -= 1;
        }
    }
    if !is_symbol_char(text[offset..].chars().next()?) && offset > 0 {
        let previous = previous_char_start(text, offset)?;
        if is_symbol_char(text[previous..].chars().next()?) {
            offset = previous;
        }
    }
    if !is_symbol_char(text[offset..].chars().next()?) {
        return None;
    }

    let mut start = offset;
    while let Some(previous) = previous_char_start(text, start) {
        let ch = text[previous..].chars().next()?;
        if !is_symbol_char(ch) {
            break;
        }
        start = previous;
    }

    let mut end = offset;
    for (relative, ch) in text[offset..].char_indices() {
        if !is_symbol_char(ch) {
            break;
        }
        end = offset + relative + ch.len_utf8();
    }
    Some((start as u32, end as u32))
}

fn previous_char_start(text: &str, offset: usize) -> Option<usize> {
    text[..offset]
        .char_indices()
        .next_back()
        .map(|(index, _)| index)
}

fn is_symbol_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.')
}

fn contains_byte(range: &SourceRange, offset: u32) -> bool {
    range.start_byte <= offset && offset <= range.end_byte
}

fn is_definition_target(symbol: &EditorSymbol) -> bool {
    match symbol.kind {
        SymbolKind::Package => symbol.detail.starts_with("package "),
        SymbolKind::Type => symbol
            .detail
            .starts_with(&format!("type {} {{", symbol.name)),
        SymbolKind::Tool => symbol.detail.starts_with(&format!("tool {}(", symbol.name)),
        SymbolKind::Verifier => symbol
            .detail
            .starts_with(&format!("verifier {}(", symbol.name)),
        SymbolKind::Agent => symbol
            .detail
            .starts_with(&format!("agent {}(", symbol.name)),
        SymbolKind::Field
        | SymbolKind::Parameter
        | SymbolKind::State
        | SymbolKind::Output
        | SymbolKind::Binding
        | SymbolKind::LoopVariable
        | SymbolKind::ModelCapability
        | SymbolKind::BudgetKey
        | SymbolKind::PolicySubject => false,
    }
}

const PRIMITIVE_TYPES: &[&str] = &[
    "string", "int", "float", "bool", "json", "bytes", "text", "markdown", "file",
];

const BUILTIN_FUNCTIONS: &[(&str, &str, &str)] = &[(
    "len",
    "len(value) -> int",
    "Returns the length of a string, list or object-like value.",
)];

const KEYWORD_COMPLETIONS: &[(&str, &str, &str, Option<&str>)] = &[
    (
        "language",
        "language 0.1",
        "Declares the Ingot language version.",
        Some("language 0.1"),
    ),
    (
        "package",
        "package name",
        "Groups declarations under a namespace.",
        None,
    ),
    (
        "import",
        "import \"./shared.ing\" { ... }",
        "Imports shared type, tool or verifier declarations from another Ingot source file.",
        Some("import \"./shared.ing\" {\n  type Name\n}"),
    ),
    (
        "type",
        "type Name { ... }",
        "Declares a typed record shape.",
        None,
    ),
    (
        "tool",
        "tool name(args) -> type",
        "Declares a callable external capability.",
        None,
    ),
    (
        "verifier",
        "verifier Name(args)",
        "Declares a deterministic check.",
        None,
    ),
    (
        "agent",
        "agent Name(args) -> output<content> { ... }",
        "Declares an agent artifact.",
        None,
    ),
    (
        "model",
        "model requires { ... }",
        "Constrains or pins the runtime model.",
        None,
    ),
    (
        "requires",
        "requires { ... }",
        "Lists model capabilities.",
        None,
    ),
    (
        "exact",
        "exact \"provider/model\"",
        "Pins a model reference.",
        None,
    ),
    (
        "tools",
        "tools { ... }",
        "Grants declared tools to an agent.",
        None,
    ),
    (
        "mcp",
        "mcp tool.name",
        "Uses the MCP transport for a tool grant.",
        None,
    ),
    (
        "memory",
        "memory { ... }",
        "Declares agent working memory.",
        None,
    ),
    (
        "working",
        "working ephemeral { ... }",
        "Declares working memory fields.",
        None,
    ),
    (
        "ephemeral",
        "ephemeral",
        "Keeps working memory local to the run.",
        None,
    ),
    (
        "budget",
        "budget { ... }",
        "Constrains steps, tokens or cost.",
        None,
    ),
    (
        "policy",
        "policy { ... }",
        "Declares capability decisions.",
        None,
    ),
    ("allow", "allow", "Allows a policy subject.", None),
    ("deny", "deny", "Denies a policy subject.", None),
    (
        "require",
        "require approval",
        "Requires approval before a capability is used.",
        Some("require approval"),
    ),
    (
        "approval",
        "approval",
        "The second word in `require approval`.",
        None,
    ),
    (
        "flow",
        "flow { ... }",
        "Declares executable agent steps.",
        None,
    ),
    (
        "ask",
        "ask<type>(prompt)",
        "Requests model output of a specific type.",
        None,
    ),
    (
        "call",
        "call name(args)",
        "Calls a granted tool or sub-agent.",
        None,
    ),
    (
        "parallel",
        "parallel map items as item { ... }",
        "Runs a map body over a list.",
        None,
    ),
    ("map", "map", "Part of a parallel map expression.", None),
    ("as", "as", "Names the loop item in a parallel map.", None),
    (
        "verify",
        "verify Name(value)",
        "Runs a declared verifier.",
        None,
    ),
    (
        "emit",
        "emit output = value",
        "Emits the agent's declared output.",
        None,
    ),
    (
        "if",
        "if condition { ... }",
        "Branches flow execution.",
        None,
    ),
    (
        "else",
        "else { ... }",
        "Fallback branch for an if statement.",
        None,
    ),
    (
        "loop",
        "loop max N while condition { ... }",
        "Runs a bounded loop.",
        None,
    ),
    (
        "max",
        "max N",
        "Sets a static upper bound for a loop.",
        None,
    ),
    ("while", "while condition", "Adds a loop guard.", None),
    (
        "checkpoint",
        "checkpoint \"label\"",
        "Marks a named flow checkpoint.",
        None,
    ),
    (
        "state",
        "state.field",
        "Reads or writes working memory.",
        None,
    ),
    ("true", "true", "Boolean literal.", None),
    ("false", "false", "Boolean literal.", None),
];

fn primitive_type_doc(name: &str) -> Option<&'static str> {
    Some(match name {
        "string" => "UTF-8 string data.",
        "int" => "Signed integer value.",
        "float" => "Floating-point numeric value.",
        "bool" => "Boolean value.",
        "json" => "JSON-compatible structured data.",
        "bytes" => "Opaque byte data.",
        "text" => "Plain text artifact content.",
        "markdown" => "Markdown artifact content.",
        "file" => "File handle produced or consumed by a tool.",
        _ => return None,
    })
}

fn builtin_doc(name: &str) -> Option<&'static str> {
    BUILTIN_FUNCTIONS
        .iter()
        .find(|(label, _, _)| *label == name)
        .map(|(_, _, doc)| *doc)
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

    const AUTHORING_SOURCE: &str = r#"language 0.1
package demo

/// Search result returned by the web tool.
type search_result { title: string url: string }

/// Search the web.
tool web.search(query: string) -> search_result[] !network

verifier CitationCheck(draft: markdown)

agent Research(topic: string) -> report<markdown> {
  tools { mcp web.search }
  memory { working ephemeral { notes: string[] } }
  budget { steps <= 10 tokens <= 1000 }
  policy { network allow }
  flow {

    hits = call web.search(topic)
    emit report = ask<markdown>("Report ${topic}")
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

    fn position_for(text: &str, needle: &str) -> EditorPosition {
        let offset = text
            .find(needle)
            .unwrap_or_else(|| panic!("test source must contain {needle:?}"));
        position_at_byte_offset(text, offset as u32)
    }

    fn position_after(text: &str, needle: &str) -> EditorPosition {
        let offset = text
            .find(needle)
            .unwrap_or_else(|| panic!("test source must contain {needle:?}"))
            + needle.len();
        position_at_byte_offset(text, offset as u32)
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
    fn completion_includes_language_keywords_and_declared_symbols() {
        let service = LanguageService::new();
        let result = service.completion_items(
            "main.ing",
            AUTHORING_SOURCE,
            position_after(AUTHORING_SOURCE, "flow {\n"),
        );
        let labels: Vec<_> = result
            .items
            .iter()
            .map(|item| item.label.as_str())
            .collect();

        assert!(labels.contains(&"agent"));
        assert!(labels.contains(&"search_result"));
        assert!(labels.contains(&"web.search"));
        assert!(labels.contains(&"CitationCheck"));
        assert!(labels.contains(&"len"));
    }

    #[test]
    fn hover_returns_symbol_detail_and_doc_comment() {
        let service = LanguageService::new();
        let hover = service
            .hover(
                "main.ing",
                AUTHORING_SOURCE,
                position_for(AUTHORING_SOURCE, "search_result {"),
            )
            .hover
            .expect("type hover should be available");

        assert!(hover.contents.contains("type search_result"));
        assert!(hover
            .contents
            .contains("Search result returned by the web tool."));
        assert_eq!(
            &AUTHORING_SOURCE[hover.range.start_byte as usize..hover.range.end_byte as usize],
            "search_result"
        );
    }

    #[test]
    fn definition_goes_from_tool_use_to_tool_declaration() {
        let service = LanguageService::new();
        let definition = service
            .definition(
                "main.ing",
                AUTHORING_SOURCE,
                position_for(AUTHORING_SOURCE, "web.search(topic)"),
            )
            .definition
            .expect("tool definition should be available");

        assert_eq!(definition.name, "web.search");
        assert_eq!(definition.kind, SymbolKind::Tool);
        assert_eq!(
            &AUTHORING_SOURCE
                [definition.target.start_byte as usize..definition.target.end_byte as usize],
            "web.search"
        );
        assert!(definition.declaration.start_byte < definition.target.start_byte);
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
