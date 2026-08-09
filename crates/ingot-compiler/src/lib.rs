//! The compilation driver.
//!
//! One entry point runs the whole front end — parse, check, lower — and returns
//! everything a caller needs to report on it: the source map for rendering
//! diagnostics, the analysis for tooling, and the IR for each agent.
//!
//! IR is produced only when the program has no errors. A partially checked
//! program cannot be lowered into an artifact anyone should run, so the build
//! stops instead of emitting something that merely looks valid.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use ingot_diagnostics::{codes, ColorChoice, Diagnostic, DiagnosticBag};
use ingot_ir::AgentIr;
use ingot_semantic::{analyze, Analysis};
use ingot_source::{FileId, SourceMap};
use ingot_syntax::{printer::print_program, ImportKind, Program};

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

fn compile_registered(mut sources: SourceMap, file: FileId) -> Compilation {
    let parsed = ingot_parser::parse(sources.file(file));

    let mut diagnostics = DiagnosticBag::new();
    diagnostics.extend(parsed.diagnostics);
    let program = expand_imports(&mut sources, file, parsed.program, &mut diagnostics);

    let analysis = analyze(&program);
    diagnostics.extend(analysis.diagnostics.iter().cloned());
    diagnostics.sort_by_position();

    let agents = if diagnostics.has_errors() {
        Vec::new()
    } else {
        analysis
            .agents
            .iter()
            .map(|agent| lower_agent(&program, &analysis, agent))
            .collect()
    };

    Compilation {
        sources,
        file,
        program,
        analysis,
        diagnostics,
        agents,
    }
}

fn expand_imports(
    sources: &mut SourceMap,
    file: FileId,
    program: Program,
    diagnostics: &mut DiagnosticBag,
) -> Program {
    let mut resolver = ImportResolver {
        sources,
        diagnostics,
        cache: HashMap::new(),
        stack: Vec::new(),
    };
    resolver.expand_program(file, program)
}

struct ImportResolver<'a> {
    sources: &'a mut SourceMap,
    diagnostics: &'a mut DiagnosticBag,
    cache: HashMap<PathBuf, (FileId, Program)>,
    stack: Vec<PathBuf>,
}

impl<'a> ImportResolver<'a> {
    fn expand_program(&mut self, file: FileId, mut program: Program) -> Program {
        let current_key = self
            .sources
            .file(file)
            .path()
            .and_then(|path| path.canonicalize().ok());
        if let Some(key) = &current_key {
            self.stack.push(key.clone());
        }

        if program.imports.is_empty() {
            if current_key.is_some() {
                self.stack.pop();
            }
            return program;
        }

        let imports = std::mem::take(&mut program.imports);
        let mut expanded = Program {
            language: program.language,
            package: program.package.clone(),
            imports: Vec::new(),
            types: Vec::new(),
            tools: Vec::new(),
            verifiers: Vec::new(),
            agents: program.agents.clone(),
            span: program.span,
        };

        for import in &imports {
            let Some((imported_file, imported_program)) = self.load_import(file, import) else {
                continue;
            };
            let imported_program = self.expand_program(imported_file, imported_program);
            for item in &import.items {
                let wanted = item.name.text();
                match item.kind {
                    ImportKind::Type => match imported_program
                        .types
                        .iter()
                        .find(|decl| decl.name.text == wanted)
                    {
                        Some(decl) => expanded.types.push(decl.clone()),
                        None => self.missing_item(
                            item.span,
                            "type",
                            &wanted,
                            imported_program
                                .types
                                .iter()
                                .map(|decl| decl.name.text.clone()),
                        ),
                    },
                    ImportKind::Tool => match imported_program
                        .tools
                        .iter()
                        .find(|decl| decl.name.text() == wanted)
                    {
                        Some(decl) => expanded.tools.push(decl.clone()),
                        None => self.missing_item(
                            item.span,
                            "tool",
                            &wanted,
                            imported_program.tools.iter().map(|decl| decl.name.text()),
                        ),
                    },
                    ImportKind::Verifier => match imported_program
                        .verifiers
                        .iter()
                        .find(|decl| decl.name.text == wanted)
                    {
                        Some(decl) => expanded.verifiers.push(decl.clone()),
                        None => self.missing_item(
                            item.span,
                            "verifier",
                            &wanted,
                            imported_program
                                .verifiers
                                .iter()
                                .map(|decl| decl.name.text.clone()),
                        ),
                    },
                }
            }
        }

        expanded.types.extend(program.types);
        expanded.tools.extend(program.tools);
        expanded.verifiers.extend(program.verifiers);
        if current_key.is_some() {
            self.stack.pop();
        }
        expanded
    }

    fn load_import(
        &mut self,
        importing_file: FileId,
        import: &ingot_syntax::ImportDecl,
    ) -> Option<(FileId, Program)> {
        let import_text = import.path.plain_text();
        let target = self.resolve_import_path(importing_file, &import_text, import.path.span)?;
        let key = match target.canonicalize() {
            Ok(path) => path,
            Err(error) => {
                self.diagnostics.push(
                    Diagnostic::error(
                        codes::IMPORT_RESOLUTION_ERROR,
                        format!("cannot read import `{import_text}`: {error}"),
                    )
                    .with_primary(import.path.span, "import target is not readable"),
                );
                return None;
            }
        };

        if self.stack.contains(&key) {
            self.diagnostics.push(
                Diagnostic::error(
                    codes::IMPORT_RESOLUTION_ERROR,
                    format!("import cycle reaches `{}`", key.display()),
                )
                .with_primary(import.path.span, "this import completes a cycle"),
            );
            return None;
        }

        if let Some((file, program)) = self.cache.get(&key) {
            return Some((*file, program.clone()));
        }

        let file = match self.sources.load(&key) {
            Ok(file) => file,
            Err(error) => {
                self.diagnostics.push(
                    Diagnostic::error(
                        codes::IMPORT_RESOLUTION_ERROR,
                        format!("cannot read import `{import_text}`: {error}"),
                    )
                    .with_primary(import.path.span, "import target is not readable"),
                );
                return None;
            }
        };
        let parsed = ingot_parser::parse(self.sources.file(file));
        self.diagnostics.extend(parsed.diagnostics);
        let program = parsed.program;
        self.cache.insert(key, (file, program.clone()));
        Some((file, program))
    }

    fn resolve_import_path(
        &mut self,
        importing_file: FileId,
        import_text: &str,
        span: ingot_source::Span,
    ) -> Option<PathBuf> {
        let path = Path::new(import_text);
        let valid = !import_text.trim().is_empty()
            && path.extension().and_then(|ext| ext.to_str()) == Some("ing")
            && !path.is_absolute()
            && path
                .components()
                .all(|component| matches!(component, Component::CurDir | Component::Normal(_)));

        if !valid {
            self.diagnostics.push(
                Diagnostic::error(
                    codes::IMPORT_RESOLUTION_ERROR,
                    format!("invalid import path `{import_text}`"),
                )
                .with_primary(span, "expected a relative `.ing` path below this directory")
                .with_help(
                    "use a path such as `./shared/web.ing`; `..` and absolute paths are rejected",
                ),
            );
            return None;
        }

        let Some(base) = self
            .sources
            .file(importing_file)
            .path()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
        else {
            self.diagnostics.push(
                Diagnostic::error(
                    codes::IMPORT_RESOLUTION_ERROR,
                    "virtual sources cannot resolve imports",
                )
                .with_primary(span, "compile from a file path to use imports"),
            );
            return None;
        };

        Some(base.join(path))
    }

    fn missing_item(
        &mut self,
        span: ingot_source::Span,
        kind: &str,
        name: &str,
        candidates: impl Iterator<Item = String>,
    ) {
        let mut candidates = candidates.collect::<Vec<_>>();
        candidates.sort();
        let mut diagnostic = Diagnostic::error(
            codes::IMPORT_RESOLUTION_ERROR,
            format!("imported {kind} `{name}` is not declared by the target file"),
        )
        .with_primary(span, "not exported by this import target");
        if !candidates.is_empty() {
            diagnostic =
                diagnostic.with_note(format!("available {kind}s: {}", candidates.join(", ")));
        }
        self.diagnostics.push(diagnostic);
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
