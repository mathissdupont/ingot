//! Source management for the Ingot toolchain.
//!
//! Every compiler stage carries [`Span`]s that point back into a [`SourceMap`].
//! Spans are byte offsets into a single file; they are resolved to human
//! readable line/column pairs only when a diagnostic is rendered.

use std::fmt;
use std::path::{Path, PathBuf};

/// Handle to a file registered in a [`SourceMap`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FileId(u32);

impl FileId {
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// A half-open byte range `[start, end)` inside a single source file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Span {
    pub file: FileId,
    pub start: u32,
    pub end: u32,
}

impl Span {
    pub fn new(file: FileId, start: u32, end: u32) -> Self {
        debug_assert!(start <= end, "span start must not exceed end");
        Span { file, start, end }
    }

    /// A zero-width span at `offset`, used for "expected token here" errors.
    pub fn empty_at(file: FileId, offset: u32) -> Self {
        Span {
            file,
            start: offset,
            end: offset,
        }
    }

    /// Smallest span covering both `self` and `other`.
    ///
    /// Spans from different files cannot be merged; `self` is returned instead.
    pub fn merge(self, other: Span) -> Span {
        if self.file != other.file {
            return self;
        }
        Span {
            file: self.file,
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }

    pub fn len(self) -> u32 {
        self.end - self.start
    }

    pub fn is_empty(self) -> bool {
        self.start == self.end
    }
}

/// A value paired with the source range it was parsed from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Spanned<T> {
    pub value: T,
    pub span: Span,
}

impl<T> Spanned<T> {
    pub fn new(value: T, span: Span) -> Self {
        Spanned { value, span }
    }
}

/// One-based line and column, suitable for display and for LSP conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineCol {
    pub line: u32,
    pub column: u32,
}

impl fmt::Display for LineCol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.line, self.column)
    }
}

/// A single registered source file plus its precomputed line index.
#[derive(Debug)]
pub struct SourceFile {
    id: FileId,
    name: String,
    path: Option<PathBuf>,
    text: String,
    line_starts: Vec<u32>,
}

impl SourceFile {
    pub fn id(&self) -> FileId {
        self.id
    }

    /// Display name used in diagnostics (usually a relative path).
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn line_count(&self) -> u32 {
        self.line_starts.len() as u32
    }

    /// Resolve a byte offset to a one-based line/column pair.
    ///
    /// Columns count Unicode scalar values, not bytes, so that diagnostics line
    /// up for non-ASCII source text.
    pub fn line_col(&self, offset: u32) -> LineCol {
        let offset = offset.min(self.text.len() as u32);
        let line_index = match self.line_starts.binary_search(&offset) {
            Ok(exact) => exact,
            Err(next) => next - 1,
        };
        let line_start = self.line_starts[line_index] as usize;
        let column = self.text[line_start..offset as usize].chars().count() as u32 + 1;
        LineCol {
            line: line_index as u32 + 1,
            column,
        }
    }

    /// Text of a one-based line, without its trailing newline.
    pub fn line_text(&self, line: u32) -> &str {
        if line == 0 || line > self.line_count() {
            return "";
        }
        let start = self.line_starts[(line - 1) as usize] as usize;
        let end = self
            .line_starts
            .get(line as usize)
            .map(|&next| next as usize)
            .unwrap_or(self.text.len());
        self.text[start..end].trim_end_matches(['\n', '\r'])
    }

    /// Byte offset at which a one-based line begins.
    pub fn line_start(&self, line: u32) -> u32 {
        if line == 0 || line > self.line_count() {
            return 0;
        }
        self.line_starts[(line - 1) as usize]
    }

    fn compute_line_starts(text: &str) -> Vec<u32> {
        let mut starts = vec![0u32];
        for (index, byte) in text.bytes().enumerate() {
            if byte == b'\n' {
                starts.push(index as u32 + 1);
            }
        }
        starts
    }
}

/// Registry of every file the current compilation touched.
#[derive(Debug, Default)]
pub struct SourceMap {
    files: Vec<SourceFile>,
}

impl SourceMap {
    pub fn new() -> Self {
        SourceMap::default()
    }

    /// Register in-memory source text. Used by tests and by the language server.
    pub fn add_virtual(&mut self, name: impl Into<String>, text: impl Into<String>) -> FileId {
        self.insert(name.into(), None, text.into())
    }

    /// Read a file from disk and register it.
    pub fn load(&mut self, path: impl AsRef<Path>) -> std::io::Result<FileId> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path)?;
        let name = path.to_string_lossy().replace('\\', "/");
        Ok(self.insert(name, Some(path.to_path_buf()), text))
    }

    fn insert(&mut self, name: String, path: Option<PathBuf>, text: String) -> FileId {
        let id = FileId(self.files.len() as u32);
        let line_starts = SourceFile::compute_line_starts(&text);
        self.files.push(SourceFile {
            id,
            name,
            path,
            text,
            line_starts,
        });
        id
    }

    pub fn file(&self, id: FileId) -> &SourceFile {
        &self.files[id.index()]
    }

    pub fn files(&self) -> impl Iterator<Item = &SourceFile> {
        self.files.iter()
    }

    /// The source text a span covers.
    pub fn snippet(&self, span: Span) -> &str {
        let file = self.file(span.file);
        let text = file.text();
        let start = (span.start as usize).min(text.len());
        let end = (span.end as usize).min(text.len());
        &text[start..end]
    }

    pub fn line_col(&self, span: Span) -> LineCol {
        self.file(span.file).line_col(span.start)
    }

    /// `path:line:column`, the form editors and terminals can jump to.
    pub fn location(&self, span: Span) -> String {
        let file = self.file(span.file);
        format!("{}:{}", file.name(), file.line_col(span.start))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_line_and_column() {
        let mut map = SourceMap::new();
        let file = map.add_virtual("test.ing", "agent A()\n  flow {\n  }\n");
        let source = map.file(file);

        assert_eq!(source.line_col(0), LineCol { line: 1, column: 1 });
        assert_eq!(source.line_col(10), LineCol { line: 2, column: 1 });
        assert_eq!(source.line_col(12), LineCol { line: 2, column: 3 });
        assert_eq!(source.line_text(2), "  flow {");
    }

    #[test]
    fn counts_columns_in_characters_not_bytes() {
        let mut map = SourceMap::new();
        let file = map.add_virtual("test.ing", "// ölçüm\nagent A()\n");
        let source = map.file(file);
        let newline = "// ölçüm".len() as u32;

        assert_eq!(source.line_col(newline).line, 1);
        assert_eq!(source.line_col(newline).column, 9);
    }

    #[test]
    fn merges_spans_within_a_file() {
        let file = FileId(0);
        let merged = Span::new(file, 4, 8).merge(Span::new(file, 10, 12));
        assert_eq!(merged, Span::new(file, 4, 12));
    }

    #[test]
    fn handles_offset_at_end_of_input() {
        let mut map = SourceMap::new();
        let file = map.add_virtual("test.ing", "abc");
        let source = map.file(file);
        assert_eq!(source.line_col(3), LineCol { line: 1, column: 4 });
        assert_eq!(source.line_col(99), LineCol { line: 1, column: 4 });
    }
}
