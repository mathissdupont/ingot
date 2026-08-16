//! The canvas: a two-way view of a flow that cannot become the source of truth.
//!
//! > The canvas never produces a file. It produces a **byte range and a
//! > replacement**, and everything outside that range is untouched by
//! > construction.
//!
//! That rule is the whole design, and it lives here rather than in a surface,
//! so that every surface keeps it. See
//! [RFC-0016](../../../rfcs/0016-the-canvas.md).
//!
//! Three consequences follow, and they are why this is not the hosted no-code
//! editor [RFC-0007](../../../rfcs/0007-the-ingot-product-loop.md) refused:
//!
//! * **Nothing outside the edited span can be lost**, because nothing outside it
//!   is written. Not a comment, not a construct the canvas cannot draw, not a
//!   blank line somebody left deliberately.
//! * **The canvas is never authoritative about correctness.** It proposes an
//!   edit; `ingot check` decides. A bug here is a compile error rather than a
//!   corrupted program.
//! * **The canvas may be partial.** It draws what it understands and hands back
//!   the rest as read-only source, in place. A surface that owned the model
//!   could not be partial; one that owns nothing can.

use ingot_source::{SourceMap, Span};
use ingot_syntax::{Expr, PolicyAction, Program, Stmt, StringLit};
use serde::Serialize;

use crate::{source_range, SourceRange};

/// A rendering of one agent's flow and policy.
///
/// A *rendering*, never a second model: everything here is derived from spans
/// into the file that produced it, and nothing here is authoritative about
/// anything.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Canvas {
    pub agent: String,
    pub blocks: Vec<Block>,
    /// Derived, never drawn. See [`Edge`].
    pub edges: Vec<Edge>,
    /// Where a new statement may be inserted at the top level.
    pub boundaries: Vec<Boundary>,
    /// The agent's policy rules, which are edited by the same three gestures.
    pub policy: Vec<PolicyBlockView>,
}

/// One statement, drawn.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Block {
    /// Stable within one rendering. Not an identity the file carries — the file
    /// carries no ids, which is the point.
    pub id: String,
    pub kind: BlockKind,
    /// The statement's exact span.
    pub span: SourceRange,
    /// What a move cuts. See [`move_unit`] — this is **not** the span above,
    /// and the difference is a comment somebody wrote.
    pub move_unit: SourceRange,
    /// The name this statement binds, when it binds one. What an edge is made of.
    pub binding: Option<String>,
    /// The exact source of [`Block::span`], for a surface that has to show a
    /// construct rather than draw it.
    pub source: String,
    /// Spans a surface may offer to replace directly.
    pub leaves: Vec<Leaf>,
    /// Blocks inside a container. Empty for everything else.
    pub children: Vec<Block>,
    /// Where a new statement may go inside a container.
    pub boundaries: Vec<Boundary>,
    /// Whether the canvas understands this well enough to rewrite it.
    ///
    /// **A construct it cannot render is never one it will rewrite — the two are
    /// the same rule.**
    pub editable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum BlockKind {
    /// `queries = ask<string[]>("…")`
    ModelCall,
    /// `framing = consult("…")` — a question put to a person.
    Question,
    /// `hits = call web.search(query)`
    ToolCall,
    /// `state.notes = queries`
    MemoryWrite,
    /// `verify CitationCheck(draft, …)`
    Check,
    /// `checkpoint "sources-collected"`
    Marker,
    /// `emit report = draft`
    Output,
    /// `if` / `loop`, holding blocks.
    Container,
    /// A binding whose value is `parallel map`.
    ///
    /// Drawn as **one** container. How many times it will run is a fact about a
    /// run, not about the program, so a fan-out of N would be a picture of
    /// something the source does not say.
    FanOut,
    /// `Stmt::Error` — the parser kept the shape and not the meaning.
    ///
    /// Drawn, deliberately: a file that does not compile still has a canvas,
    /// with one block marked unreadable rather than an empty page.
    Unreadable,
    /// Something the canvas has no drawing for. Shown as source, in place.
    Unknown,
}

/// A span a surface may offer to replace on its own.
///
/// This is where most of the work is for somebody who does not write code: a
/// textarea for a prompt is a textarea over one `StringLit` span.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Leaf {
    /// What this leaf is for, e.g. `prompt`, `tool`, `label`, `type`.
    pub role: String,
    pub kind: LeafKind,
    pub span: SourceRange,
    /// The exact source of the span, so a surface never has to re-read the file.
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LeafKind {
    /// A string literal. Replaced with its quotes.
    Text,
    /// An identifier or dotted name.
    Name,
    /// A type expression.
    Type,
    /// A number.
    Number,
    /// A policy action, e.g. `deny` or `allow ["arxiv.org"]`.
    PolicyAction,
}

/// A place a new statement may be inserted.
///
/// An insert is a replacement of an **empty** range, which is why the canvas
/// needs no second mechanism for it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Boundary {
    /// How many statements precede this point.
    pub index: usize,
    pub byte: u32,
    /// The indentation of the statements around it, so an inserted template
    /// lands looking like its neighbours.
    pub indent: String,
}

/// A fact *about* the text, computed rather than stored.
///
/// There is an edge from A to B when B mentions a name that A binds. That is
/// all. In an ordinary node editor the edges *are* the program; here the
/// program is a sequence of named bindings and an edge is derived — so there is
/// nothing to store, and nothing that can disagree with the file.
///
/// It follows that **you cannot drag one**.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Edge {
    pub from: String,
    pub to: String,
    /// The name that ties them.
    pub name: String,
}

/// One policy rule, drawn.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PolicyBlockView {
    pub subject: String,
    pub span: SourceRange,
    pub move_unit: SourceRange,
    pub source: String,
    pub leaves: Vec<Leaf>,
}

// --- edits ------------------------------------------------------------------

/// One byte range, what the canvas expects to find there, and what replaces it.
///
/// The expectation is not ceremony. A canvas holds a rendering of bytes it read
/// some time ago, and the file may have changed since — an editor saved,
/// `ingot fmt` ran, a branch switched. Applying a range computed against the old
/// text to the new text is **the one way this design can lose somebody's work**,
/// so the edit carries the text it was computed against and the application
/// compares before replacing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanvasEdit {
    pub start_byte: u32,
    pub end_byte: u32,
    pub expected: String,
    pub new_text: String,
}

/// Why an edit was not applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditRefused {
    /// The range does not lie inside the file, or is inverted.
    OutsideFile { start: u32, end: u32, len: u32 },
    /// The file changed under the view.
    Stale { expected: String, found: String },
}

impl std::fmt::Display for EditRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EditRefused::OutsideFile { start, end, len } => write!(
                f,
                "the edit covers bytes {start}..{end} of a {len}-byte file"
            ),
            EditRefused::Stale { .. } => f.write_str(
                "this file changed since the canvas read it; reload before editing again",
            ),
        }
    }
}

impl std::error::Error for EditRefused {}

/// Apply one edit, or refuse.
///
/// Everything outside `start_byte..end_byte` is copied through untouched. That
/// is not a promise this function keeps by being careful — it is the only thing
/// it can do, because the range is all it is given.
pub fn apply(source: &str, edit: &CanvasEdit) -> Result<String, EditRefused> {
    let len = source.len() as u32;
    if edit.start_byte > edit.end_byte
        || edit.end_byte > len
        || !source.is_char_boundary(edit.start_byte as usize)
        || !source.is_char_boundary(edit.end_byte as usize)
    {
        return Err(EditRefused::OutsideFile {
            start: edit.start_byte,
            end: edit.end_byte,
            len,
        });
    }

    let found = &source[edit.start_byte as usize..edit.end_byte as usize];
    if found != edit.expected {
        return Err(EditRefused::Stale {
            expected: edit.expected.clone(),
            found: found.to_string(),
        });
    }

    let mut out = String::with_capacity(source.len() + edit.new_text.len());
    out.push_str(&source[..edit.start_byte as usize]);
    out.push_str(&edit.new_text);
    out.push_str(&source[edit.end_byte as usize..]);
    Ok(out)
}

/// The edit that replaces one leaf.
pub fn replace_leaf(source: &str, leaf: &Leaf, new_text: impl Into<String>) -> CanvasEdit {
    CanvasEdit {
        start_byte: leaf.span.start_byte,
        end_byte: leaf.span.end_byte,
        expected: slice(source, leaf.span.start_byte, leaf.span.end_byte),
        new_text: new_text.into(),
    }
}

/// The edit that removes a block, comments and all.
pub fn delete_block(source: &str, block: &Block) -> CanvasEdit {
    CanvasEdit {
        start_byte: block.move_unit.start_byte,
        end_byte: block.move_unit.end_byte,
        expected: slice(source, block.move_unit.start_byte, block.move_unit.end_byte),
        new_text: String::new(),
    }
}

/// The edit that inserts `text` at a boundary.
///
/// A replacement of an empty range: the same mechanism as every other gesture,
/// which is why there is no second one.
pub fn insert_at(boundary: &Boundary, text: impl AsRef<str>) -> CanvasEdit {
    CanvasEdit {
        start_byte: boundary.byte,
        end_byte: boundary.byte,
        expected: String::new(),
        new_text: format!("{}{}\n", boundary.indent, text.as_ref().trim_end()),
    }
}

/// The edit that moves a block to a boundary in the same list.
///
/// **Still one range and one replacement.** A move is naturally two operations,
/// and expressing it as two would break the rule the whole design rests on. So
/// the range spans from the earlier of the two positions to the later, and the
/// replacement is that stretch of the *original bytes*, reordered. Everything
/// inside it that is neither the moved block nor the boundary is copied through
/// verbatim, character for character.
pub fn move_block(
    source: &str,
    blocks: &[Block],
    block: &Block,
    to: &Boundary,
) -> Option<CanvasEdit> {
    let from = blocks.iter().position(|other| other.id == block.id)?;
    // Moving to the boundary immediately before or after itself is a gesture
    // that lands where it started.
    if to.index == from || to.index == from + 1 {
        return None;
    }

    let unit_start = block.move_unit.start_byte;
    let unit_end = block.move_unit.end_byte;
    let cut = slice(source, unit_start, unit_end);

    let (start, end, new_text) = if to.index < from {
        let start = to.byte;
        let between = slice(source, start, unit_start);
        (start, unit_end, format!("{cut}{between}"))
    } else {
        let end = to.byte;
        let between = slice(source, unit_end, end);
        (unit_start, end, format!("{between}{cut}"))
    };

    Some(CanvasEdit {
        start_byte: start,
        end_byte: end,
        expected: slice(source, start, end),
        new_text,
    })
}

// --- the move unit ----------------------------------------------------------

/// The bytes a person means by "this block".
///
/// A statement's span is exact, and it is **not** what somebody means when they
/// drag one. Given:
///
/// ```ingot
/// // The queries decide everything downstream, so they are worth a slow model.
/// queries = ask<string[]>("Diverse queries for ${topic}.")
/// ```
///
/// the comment is trivia. The lexer skips it and keeps no span, so the tree
/// cannot say it exists, let alone that it belongs to the statement below.
/// Moving by the AST span alone would leave it behind, now describing whatever
/// moved into its place — exactly the quiet loss this design exists to prevent,
/// caused by the mechanism meant to prevent it.
///
/// **So this reads the source text, not the tree**, and that is worth stating
/// plainly: the tree cannot answer the question, because the information was
/// discarded before the tree existed.
///
/// From the statement's first line, walk backwards over whole lines while they
/// are blank or hold only a comment, stopping at the first line that is neither
/// or at the start of the enclosing block. The unit runs from there through the
/// end of the statement's last line.
pub fn move_unit(source: &str, start: u32, end: u32, floor: u32) -> (u32, u32) {
    let mut unit_start = line_start(source, start);
    while unit_start > floor {
        let previous = line_start(source, unit_start.saturating_sub(1));
        if previous < floor {
            break;
        }
        let line = slice(source, previous, unit_start);
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("//") {
            unit_start = previous;
        } else {
            break;
        }
    }
    (unit_start, line_end(source, end))
}

fn line_start(source: &str, byte: u32) -> u32 {
    source[..byte as usize]
        .rfind('\n')
        .map(|index| index as u32 + 1)
        .unwrap_or(0)
}

/// Through the newline that ends the line `byte` sits on, so a unit is whole
/// lines and moving one never welds two statements onto one line.
fn line_end(source: &str, byte: u32) -> u32 {
    match source[byte as usize..].find('\n') {
        Some(offset) => byte + offset as u32 + 1,
        None => source.len() as u32,
    }
}

fn slice(source: &str, start: u32, end: u32) -> String {
    source
        .get(start as usize..end as usize)
        .unwrap_or_default()
        .to_string()
}

fn indent_of(source: &str, byte: u32) -> String {
    let start = line_start(source, byte);
    source[start as usize..]
        .chars()
        .take_while(|character| *character == ' ' || *character == '\t')
        .collect()
}

// --- rendering --------------------------------------------------------------

/// Render one agent's flow, or the first agent when `agent` is `None`.
pub fn canvas_of(
    map: &SourceMap,
    program: &Program,
    source: &str,
    agent: Option<&str>,
) -> Option<Canvas> {
    let decl = match agent {
        Some(name) => program.agents.iter().find(|decl| decl.name.text == name)?,
        None => program.agents.first()?,
    };
    let flow = decl.flow.as_ref()?;

    let mut next = 0usize;
    let floor = line_start(source, flow.span.start);
    let blocks = render_block_list(map, source, &flow.statements, floor, &mut next);
    let boundaries = boundaries_for(source, &blocks, flow.span);
    let edges = derive_edges(&blocks);

    let policy = decl
        .policy
        .as_ref()
        .map(|block| {
            block
                .rules
                .iter()
                .map(|rule| {
                    let span = rule.span;
                    let floor = line_start(source, block.span.start);
                    let (unit_start, unit_end) = move_unit(source, span.start, span.end, floor);
                    PolicyBlockView {
                        subject: rule.subject.text.clone(),
                        span: source_range(map, span),
                        move_unit: source_range(map, Span::new(span.file, unit_start, unit_end)),
                        source: slice(source, span.start, span.end),
                        leaves: policy_leaves(map, source, rule),
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    Some(Canvas {
        agent: decl.name.text.clone(),
        blocks,
        edges,
        boundaries,
        policy,
    })
}

fn render_block_list(
    map: &SourceMap,
    source: &str,
    statements: &[Stmt],
    floor: u32,
    next: &mut usize,
) -> Vec<Block> {
    statements
        .iter()
        .map(|statement| render_block(map, source, statement, floor, next))
        .collect()
}

fn render_block(
    map: &SourceMap,
    source: &str,
    statement: &Stmt,
    floor: u32,
    next: &mut usize,
) -> Block {
    let span = statement_span(statement);
    let id = format!("b{}", *next);
    *next += 1;

    let (kind, binding, children_source) = classify(statement);
    let (unit_start, unit_end) = move_unit(source, span.start, span.end, floor);

    let children = match children_source {
        Some(inner) => {
            let inner_floor = line_start(source, span.start);
            render_block_list(map, source, inner, inner_floor, next)
        }
        None => Vec::new(),
    };
    let boundaries = if children.is_empty() {
        Vec::new()
    } else {
        boundaries_for(source, &children, span)
    };

    Block {
        id,
        kind,
        span: source_range(map, span),
        move_unit: source_range(map, Span::new(span.file, unit_start, unit_end)),
        binding,
        source: slice(source, span.start, span.end),
        leaves: leaves_of(map, source, statement),
        children,
        boundaries,
        // Everything the canvas has a kind for, it can also rewrite. An
        // unreadable statement is shown and left alone.
        editable: !matches!(kind, BlockKind::Unreadable | BlockKind::Unknown),
    }
}

fn classify(statement: &Stmt) -> (BlockKind, Option<String>, Option<&[Stmt]>) {
    match statement {
        Stmt::Bind { name, value, .. } => {
            let binding = Some(name.text.clone());
            match value {
                Expr::Ask { .. } => (BlockKind::ModelCall, binding, None),
                Expr::Consult { .. } => (BlockKind::Question, binding, None),
                Expr::Call { .. } => (BlockKind::ToolCall, binding, None),
                // The first place the view and the grammar differ, and the view
                // loses: `parallel map` is an *expression*, so a fan-out is a
                // binding whose value happens to contain a statement list. What
                // is drawn is a rendering of the statement, never a second model
                // of it.
                Expr::ParallelMap { body, .. } => (BlockKind::FanOut, binding, Some(body)),
                _ => (BlockKind::Unknown, binding, None),
            }
        }
        Stmt::StateWrite { .. } => (BlockKind::MemoryWrite, None, None),
        Stmt::Verify { .. } => (BlockKind::Check, None, None),
        Stmt::Emit { output, .. } => (BlockKind::Output, Some(output.text.clone()), None),
        Stmt::Checkpoint { .. } => (BlockKind::Marker, None, None),
        Stmt::If { then_branch, .. } => (BlockKind::Container, None, Some(then_branch)),
        Stmt::Loop { body, .. } => (BlockKind::Container, None, Some(body)),
        Stmt::Expr { value, .. } => match value {
            Expr::Call { .. } => (BlockKind::ToolCall, None, None),
            _ => (BlockKind::Unknown, None, None),
        },
        Stmt::Error { .. } => (BlockKind::Unreadable, None, None),
    }
}

fn statement_span(statement: &Stmt) -> Span {
    match statement {
        Stmt::Bind { span, .. }
        | Stmt::StateWrite { span, .. }
        | Stmt::Expr { span, .. }
        | Stmt::Verify { span, .. }
        | Stmt::Emit { span, .. }
        | Stmt::If { span, .. }
        | Stmt::Loop { span, .. }
        | Stmt::Checkpoint { span, .. }
        | Stmt::Error { span } => *span,
    }
}

fn boundaries_for(source: &str, blocks: &[Block], container: Span) -> Vec<Boundary> {
    let mut boundaries = Vec::with_capacity(blocks.len() + 1);
    for (index, block) in blocks.iter().enumerate() {
        boundaries.push(Boundary {
            index,
            byte: block.move_unit.start_byte,
            indent: indent_of(source, block.span.start_byte),
        });
    }
    let (byte, indent) = match blocks.last() {
        Some(last) => (
            last.move_unit.end_byte,
            indent_of(source, last.span.start_byte),
        ),
        // An empty container: insert just inside its opening brace.
        None => (line_end(source, container.start), String::new()),
    };
    boundaries.push(Boundary {
        index: blocks.len(),
        byte,
        indent,
    });
    boundaries
}

/// Every edge, computed from the text rather than stored beside it.
fn derive_edges(blocks: &[Block]) -> Vec<Edge> {
    let mut edges = Vec::new();
    for (index, block) in blocks.iter().enumerate() {
        let Some(name) = &block.binding else { continue };
        for later in &blocks[index + 1..] {
            if mentions(&later.source, name) {
                edges.push(Edge {
                    from: block.id.clone(),
                    to: later.id.clone(),
                    name: name.clone(),
                });
            }
        }
    }
    edges
}

/// Whether `text` uses `name` as a word.
///
/// Word boundaries rather than a substring search, so `topic` does not match
/// `topics` and draw an edge the program does not have.
fn mentions(text: &str, name: &str) -> bool {
    let bytes = text.as_bytes();
    let mut from = 0usize;
    while let Some(found) = text[from..].find(name) {
        let at = from + found;
        let before_ok = at == 0 || !is_word_byte(bytes[at - 1]);
        let after = at + name.len();
        let after_ok = after >= bytes.len() || !is_word_byte(bytes[after]);
        if before_ok && after_ok {
            return true;
        }
        from = at + name.len().max(1);
    }
    false
}

fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

// --- leaves -----------------------------------------------------------------

fn leaves_of(map: &SourceMap, source: &str, statement: &Stmt) -> Vec<Leaf> {
    let mut leaves = Vec::new();
    match statement {
        Stmt::Bind { value, .. } => expr_leaves(map, source, value, &mut leaves),
        Stmt::Expr { value, .. } => expr_leaves(map, source, value, &mut leaves),
        Stmt::Emit { value, .. } => expr_leaves(map, source, value, &mut leaves),
        Stmt::StateWrite { value, field, .. } => {
            push(
                map,
                source,
                "field",
                LeafKind::Name,
                field.span,
                &mut leaves,
            );
            expr_leaves(map, source, value, &mut leaves);
        }
        Stmt::Verify {
            validator, args, ..
        } => {
            push(
                map,
                source,
                "verifier",
                LeafKind::Name,
                validator.span,
                &mut leaves,
            );
            for arg in args {
                expr_leaves(map, source, &arg.value, &mut leaves);
            }
        }
        Stmt::Checkpoint { label, .. } => {
            push_string(map, source, "label", label, &mut leaves);
        }
        Stmt::If { condition, .. } => expr_leaves(map, source, condition, &mut leaves),
        // A `loop` bound and an unreadable statement have nothing a surface
        // should offer to replace on its own.
        Stmt::Loop { .. } | Stmt::Error { .. } => {}
    }
    leaves
}

/// The leaves of an expression, one level deep.
///
/// Deliberately not recursive into every nested expression: a leaf is something
/// a person edits in a form, and an arbitrarily nested subexpression is not.
fn expr_leaves(map: &SourceMap, source: &str, expr: &Expr, leaves: &mut Vec<Leaf>) {
    match expr {
        Expr::Ask { result, args, .. } => {
            push(map, source, "type", LeafKind::Type, result.span(), leaves);
            for arg in args {
                let role = arg.name.as_ref().map(|name| name.text.as_str());
                arg_leaf(map, source, role.unwrap_or("prompt"), &arg.value, leaves);
            }
        }
        Expr::Consult { args, .. } => {
            for arg in args {
                let role = arg.name.as_ref().map(|name| name.text.as_str());
                arg_leaf(map, source, role.unwrap_or("question"), &arg.value, leaves);
            }
        }
        Expr::Call { callee, args, .. } => {
            push(map, source, "tool", LeafKind::Name, callee.span, leaves);
            for arg in args {
                let role = arg.name.as_ref().map(|name| name.text.as_str());
                arg_leaf(map, source, role.unwrap_or("argument"), &arg.value, leaves);
            }
        }
        Expr::Str(literal) => push_string(map, source, "text", literal, leaves),
        Expr::Path(path) => push(map, source, "name", LeafKind::Name, path.span, leaves),
        Expr::ParallelMap { source: over, .. } => {
            if let Expr::Path(path) = &**over {
                push(map, source, "over", LeafKind::Name, path.span, leaves);
            }
        }
        _ => {}
    }
}

fn arg_leaf(map: &SourceMap, source: &str, role: &str, value: &Expr, leaves: &mut Vec<Leaf>) {
    match value {
        Expr::Str(literal) => push_string(map, source, role, literal, leaves),
        Expr::Path(path) => push(map, source, role, LeafKind::Name, path.span, leaves),
        Expr::Int { span, .. } | Expr::Float { span, .. } => {
            push(map, source, role, LeafKind::Number, *span, leaves)
        }
        _ => {}
    }
}

/// A string literal's leaf covers its **quotes as well as its text**.
///
/// A surface replacing one has to write a literal, not prose — which keeps
/// escaping where the language defines it rather than inventing a second set of
/// rules in a form.
fn push_string(
    map: &SourceMap,
    source: &str,
    role: &str,
    literal: &StringLit,
    leaves: &mut Vec<Leaf>,
) {
    push(map, source, role, LeafKind::Text, literal.span, leaves);
}

fn push(
    map: &SourceMap,
    source: &str,
    role: &str,
    kind: LeafKind,
    span: Span,
    leaves: &mut Vec<Leaf>,
) {
    leaves.push(Leaf {
        role: role.to_string(),
        kind,
        span: source_range(map, span),
        text: slice(source, span.start, span.end),
    });
}

/// The leaves of one policy rule.
///
/// Mechanically this needed nothing new: `PolicyRule` and every `PolicyAction`
/// carry spans, so the three granularities cover a policy unchanged. Widening a
/// grant is compiled like any other edit — a tool whose declared reach no longer
/// sits inside the policy is still `ING4009`, because all the canvas can do is
/// write source.
fn policy_leaves(map: &SourceMap, source: &str, rule: &ingot_syntax::PolicyRule) -> Vec<Leaf> {
    let mut leaves = Vec::new();
    push(
        map,
        source,
        "subject",
        LeafKind::Name,
        rule.subject.span,
        &mut leaves,
    );
    let action = match &rule.action {
        PolicyAction::Allow { span, .. }
        | PolicyAction::Deny { span, .. }
        | PolicyAction::RequireApproval { span } => *span,
    };
    push(
        map,
        source,
        "action",
        LeafKind::PolicyAction,
        action,
        &mut leaves,
    );
    leaves
}
