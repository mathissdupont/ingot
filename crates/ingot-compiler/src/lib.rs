//! The compilation driver.
//!
//! One entry point runs the whole front end — parse, check, lower — and returns
//! everything a caller needs to report on it: the source map for rendering
//! diagnostics, the analysis for tooling, and the IR for each agent.
//!
//! IR is produced only when the program has no errors. A partially checked
//! program cannot be lowered into an artifact anyone should run, so the build
//! stops instead of emitting something that merely looks valid.

use std::path::Path;

use ingot_diagnostics::{ColorChoice, DiagnosticBag};
use ingot_ir::AgentIr;
use ingot_semantic::{analyze, Analysis};
use ingot_source::{FileId, SourceMap};
use ingot_syntax::{printer::print_program, Program};

mod lower;

pub use ingot_diagnostics::Severity;
pub use lower::lower_agent;

/// Everything one compilation produced.
pub struct Compilation {
    pub sources: SourceMap,
    pub file: FileId,
    pub program: Program,
    pub analysis: Analysis,
    pub diagnostics: DiagnosticBag,
    /// One IR document per agent, in declaration order. Empty when the program
    /// has errors.
    pub agents: Vec<AgentIr>,
}

impl Compilation {
    pub fn has_errors(&self) -> bool {
        self.diagnostics.has_errors()
    }

    pub fn error_count(&self) -> usize {
        self.diagnostics.error_count()
    }

    pub fn warning_count(&self) -> usize {
        self.diagnostics.warning_count()
    }

    /// The agent an artifact defaults to: the only one, or the first declared.
    pub fn primary_agent(&self) -> Option<&AgentIr> {
        self.agents.first()
    }

    pub fn agent(&self, name: &str) -> Option<&AgentIr> {
        self.agents
            .iter()
            .find(|agent| agent.agent == name || agent.agent.ends_with(&format!(".{name}")))
    }

    /// Render every diagnostic for a terminal.
    pub fn render_diagnostics(&self, color: ColorChoice) -> String {
        ingot_diagnostics::render_all(&self.sources, &self.diagnostics, color)
    }

    /// The canonical formatting of the parsed source.
    pub fn formatted_source(&self) -> String {
        print_program(&self.program)
    }
}

/// Compile source text that is already in memory.
pub fn compile_source(name: impl Into<String>, text: impl Into<String>) -> Compilation {
    let mut sources = SourceMap::new();
    let file = sources.add_virtual(name, text);
    compile_registered(sources, file)
}

/// Read a file from disk and compile it.
pub fn compile_path(path: impl AsRef<Path>) -> std::io::Result<Compilation> {
    let mut sources = SourceMap::new();
    let file = sources.load(path)?;
    Ok(compile_registered(sources, file))
}

fn compile_registered(sources: SourceMap, file: FileId) -> Compilation {
    let parsed = ingot_parser::parse(sources.file(file));
    let analysis = analyze(&parsed.program);

    let mut diagnostics = DiagnosticBag::new();
    diagnostics.extend(parsed.diagnostics);
    diagnostics.extend(analysis.diagnostics.iter().cloned());
    diagnostics.sort_by_position();

    let agents = if diagnostics.has_errors() {
        Vec::new()
    } else {
        analysis
            .agents
            .iter()
            .map(|agent| lower_agent(&parsed.program, &analysis, agent))
            .collect()
    };

    Compilation {
        sources,
        file,
        program: parsed.program,
        analysis,
        diagnostics,
        agents,
    }
}

/// Parse and format source without checking it.
///
/// Formatting must work on programs that do not yet type-check, so this stops
/// after parsing. It returns `None` when the source could not be parsed at all.
pub fn format_source(name: impl Into<String>, text: impl Into<String>) -> FormatResult {
    let mut sources = SourceMap::new();
    let file = sources.add_virtual(name, text);
    let parsed = ingot_parser::parse(sources.file(file));
    let formatted = if parsed.diagnostics.has_errors() {
        None
    } else {
        Some(print_program(&parsed.program))
    };
    FormatResult {
        sources,
        diagnostics: parsed.diagnostics,
        formatted,
    }
}

pub struct FormatResult {
    pub sources: SourceMap,
    pub diagnostics: DiagnosticBag,
    /// `None` when the source has syntax errors.
    pub formatted: Option<String>,
}

#[cfg(test)]
mod tests;
