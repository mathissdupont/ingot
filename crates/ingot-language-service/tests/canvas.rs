//! The conformance tests [RFC-0016](../../../rfcs/0016-the-canvas.md) names.
//!
//! The property under test is not "the canvas draws correctly". It is that
//! **the file survives** — that everything outside an edited span is still
//! there afterwards, byte for byte, including the things the canvas cannot see.

use std::path::{Path, PathBuf};

use ingot_compiler::compile_source;
use ingot_language_service::canvas::{
    apply, canvas_of, delete_block, insert_at, move_block, replace_leaf, Block, BlockKind, Canvas,
    CanvasEdit, EditRefused, Leaf,
};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the crate lives two levels below the repository root")
        .to_path_buf()
}

/// Render the first agent of `source`.
fn canvas(source: &str) -> Canvas {
    let compilation = compile_source("main.ing", source.to_string());
    canvas_of(&compilation.sources, &compilation.program, source, None)
        .expect("the source declares an agent with a flow")
}

/// Every block, containers flattened, in order.
fn flatten(blocks: &[Block]) -> Vec<&Block> {
    let mut out = Vec::new();
    for block in blocks {
        out.push(block);
        out.extend(flatten(&block.children));
    }
    out
}

fn find(canvas: &Canvas, needle: &str) -> Block {
    flatten(&canvas.blocks)
        .into_iter()
        .find(|block| block.source.contains(needle))
        .unwrap_or_else(|| panic!("no block containing `{needle}`"))
        .clone()
}

fn leaf(block: &Block, role: &str) -> Leaf {
    block
        .leaves
        .iter()
        .find(|leaf| leaf.role == role)
        .unwrap_or_else(|| panic!("no `{role}` leaf on `{}`", block.source))
        .clone()
}

fn diagnostics_of(source: &str) -> String {
    let compilation = compile_source("main.ing", source.to_string());
    compilation
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code.to_string())
        .collect::<Vec<_>>()
        .join(" ")
}

// --- 1 ----------------------------------------------------------------------

#[test]
fn an_identity_edit_to_every_statement_leaves_every_example_byte_identical() {
    // The whole design in one assertion. If a rendering ever computed a span
    // that was off by a byte, this is what would catch it — on real source
    // rather than on a fixture written to be easy.
    let examples = repo_root().join("examples");
    let mut checked = 0usize;

    for entry in std::fs::read_dir(&examples).expect("examples/ must exist") {
        let path = entry.expect("a readable entry").path().join("main.ing");
        if !path.is_file() {
            continue;
        }
        let source = std::fs::read_to_string(&path).expect("a readable example");
        let rendered = canvas(&source);

        for block in flatten(&rendered.blocks) {
            let edit = CanvasEdit {
                start_byte: block.span.start_byte,
                end_byte: block.span.end_byte,
                expected: block.source.clone(),
                new_text: block.source.clone(),
            };
            let after = apply(&source, &edit).expect("an identity edit must apply");
            assert_eq!(
                after,
                source,
                "{}: replacing `{}` with itself changed the file",
                path.display(),
                block.source
            );
            checked += 1;
        }

        // And every leaf, which is the span a person actually edits.
        for block in flatten(&rendered.blocks) {
            for leaf in &block.leaves {
                let edit = replace_leaf(&source, leaf, leaf.text.clone());
                let after = apply(&source, &edit).expect("an identity leaf edit must apply");
                assert_eq!(after, source, "{}: leaf `{}`", path.display(), leaf.role);
                checked += 1;
            }
        }
    }

    assert!(checked > 40, "only {checked} identity edits were made");
}

// --- 2 ----------------------------------------------------------------------

const WITH_COMMENT: &str = r#"language 0.1

agent Ordered(topic: string) -> report<markdown> {
  budget {
    steps <= 6
    tokens <= 20000
  }

  policy {
    network deny
  }

  flow {
    first = ask<string>("One about ${topic}.")
    // The queries decide everything downstream, so they are worth a slow model.
    queries = ask<string>("Two about ${topic}.")
    emit report = ask<markdown>("Three from ${first} and ${queries}.")
  }
}
"#;

#[test]
fn a_comment_above_a_statement_moves_with_it() {
    // The tree cannot answer this: the lexer skips a comment and keeps no span,
    // so moving by the AST span alone would leave it behind describing whatever
    // moved into its place. That is the quiet loss the move unit exists to stop.
    let rendered = canvas(WITH_COMMENT);
    let queries = find(&rendered, "queries = ask");

    assert!(
        queries.move_unit.start_byte < queries.span.start_byte,
        "the move unit must reach back over the comment"
    );
    let unit =
        &WITH_COMMENT[queries.move_unit.start_byte as usize..queries.move_unit.end_byte as usize];
    assert!(unit.contains("// The queries decide"), "{unit}");

    // Move it to the front of the flow.
    let edit = move_block(
        WITH_COMMENT,
        &rendered.blocks,
        &queries,
        &rendered.boundaries[0],
    )
    .expect("a move to the first boundary is a real move");
    let after = apply(WITH_COMMENT, &edit).expect("the move applies");

    let comment_line = after
        .lines()
        .position(|line| line.contains("// The queries decide"))
        .expect("the comment survived");
    let statement_line = after
        .lines()
        .position(|line| line.contains("queries = ask"))
        .expect("the statement survived");
    assert_eq!(
        comment_line + 1,
        statement_line,
        "the comment must still sit immediately above its statement:\n{after}"
    );
    // Nothing else moved: the same lines, reordered.
    let mut before_lines: Vec<&str> = WITH_COMMENT.lines().collect();
    let mut after_lines: Vec<&str> = after.lines().collect();
    before_lines.sort_unstable();
    after_lines.sort_unstable();
    assert_eq!(before_lines, after_lines, "a move may only reorder lines");
}

// --- 3 ----------------------------------------------------------------------

#[test]
fn a_move_past_a_reader_is_caught_by_the_compiler_rather_than_by_the_canvas() {
    // The canvas refusing a bad drop is an ergonomic convenience and nothing
    // depends on it being right. This bypasses it entirely and asserts the
    // backstop: the edit produces source, the source is compiled, and the
    // mistake is an ordinary diagnostic with a span.
    let rendered = canvas(WITH_COMMENT);
    let first = find(&rendered, "first = ask");
    let last = rendered.boundaries.last().expect("a final boundary");

    let edit = move_block(WITH_COMMENT, &rendered.blocks, &first, last)
        .expect("moving to the end is a real move");
    let after = apply(WITH_COMMENT, &edit).expect("the move applies");

    let codes = diagnostics_of(&after);
    assert!(
        // RFC-0016 wrote `ING2003: 'queries' is not in scope`. ING2003 is
        // `UNKNOWN_TYPE`; an unresolved name is ING2001, and one read through a
        // string interpolation — which is how a prompt reads a binding — is
        // ING2009. The code the RFC named never fires for this case, and only
        // writing the test found it.
        codes.contains("ING2009") || codes.contains("ING2001"),
        "moving a binding below its reader must be an unresolved name: {codes}\n{after}"
    );
}

// --- 4 ----------------------------------------------------------------------

const WITH_UNDRAWABLE: &str = r#"language 0.1

agent Mixed(topic: string) -> report<markdown> {
  budget {
    steps <= 8
    tokens <= 20000
  }

  policy {
    network deny
  }

  flow {
    draft = ask<markdown>("Write about ${topic}.")
    loop max 3 {
      _ignored = ask<string>("Refine.")
    }
    emit report = draft
  }
}
"#;

#[test]
fn editing_a_leaf_leaves_a_construct_the_canvas_cannot_draw_byte_identical() {
    let rendered = canvas(WITH_UNDRAWABLE);
    let draft = find(&rendered, "draft = ask");
    let prompt = leaf(&draft, "prompt");

    let edit = replace_leaf(
        WITH_UNDRAWABLE,
        &prompt,
        "\"Write a brief about ${topic}.\"",
    );
    let after = apply(WITH_UNDRAWABLE, &edit).expect("a leaf edit applies");

    let loop_before = WITH_UNDRAWABLE
        .find("loop max 3 {")
        .map(|at| &WITH_UNDRAWABLE[at..WITH_UNDRAWABLE[at..].find('}').unwrap() + at]);
    let loop_after = after
        .find("loop max 3 {")
        .map(|at| &after[at..after[at..].find('}').unwrap() + at]);
    assert_eq!(
        loop_before, loop_after,
        "the construct the edit did not touch must be identical"
    );
    assert!(after.contains("Write a brief about"), "{after}");
}

// --- 5 ----------------------------------------------------------------------

#[test]
fn an_edit_whose_expected_text_no_longer_matches_is_refused() {
    // The one way this design can lose somebody's work: a range computed
    // against bytes that have since changed. It is turned into a message.
    let rendered = canvas(WITH_COMMENT);
    let first = find(&rendered, "first = ask");
    let prompt = leaf(&first, "prompt");
    let edit = replace_leaf(WITH_COMMENT, &prompt, "\"Something else.\"");

    let moved_on = WITH_COMMENT.replace("One about", "Something entirely different about");
    let refused = apply(&moved_on, &edit).expect_err("a stale edit must be refused");

    assert!(matches!(refused, EditRefused::Stale { .. }), "{refused:?}");
    assert!(
        refused.to_string().contains("reload"),
        "the message has to say what to do: {refused}"
    );
    // And it changed nothing.
    assert!(moved_on.contains("Something entirely different about"));
}

// --- 6 ----------------------------------------------------------------------

const BROKEN: &str = r#"language 0.1

agent Broken(topic: string) -> report<markdown> {
  budget {
    steps <= 6
    tokens <= 20000
  }

  policy {
    network deny
  }

  flow {
    draft = ask<markdown>("Write about ${topic}.")
    @@@ ???
    emit report = draft
  }
}
"#;

#[test]
fn a_file_that_does_not_parse_still_has_a_canvas_with_the_bad_statement_marked() {
    // Refusing to draw anything because one statement is unreadable would mean
    // the canvas never opens for the file that most needs looking at.
    let rendered = canvas(BROKEN);
    let blocks = flatten(&rendered.blocks);

    assert!(
        blocks
            .iter()
            .any(|block| block.kind == BlockKind::ModelCall),
        "the statements around the bad one still draw"
    );
    // The essential property is not which of the two non-drawing kinds it gets
    // — it is that the canvas still opened, the bad statement is *there*, and
    // the canvas will not rewrite it. A construct it cannot render is never one
    // it will rewrite; the two are the same rule.
    let undrawn: Vec<_> = blocks
        .iter()
        .filter(|block| matches!(block.kind, BlockKind::Unreadable | BlockKind::Unknown))
        .collect();
    assert!(!undrawn.is_empty(), "the bad statement must be drawn");
    assert!(
        undrawn.iter().all(|block| !block.editable),
        "a construct the canvas cannot render is never one it will rewrite"
    );
    assert!(
        blocks
            .iter()
            .any(|block| block.kind == BlockKind::Unreadable),
        "a statement the parser could not read at all is marked unreadable"
    );
}

// --- 7 ----------------------------------------------------------------------

const WITH_REACH: &str = r#"language 0.2

/// Full-text search, declared to reach exactly one host.
tool web.search(query: string) -> string[] !network("arxiv.org")

agent Reaching(topic: string) -> report<markdown> {
  tools {
    mcp web.search
  }

  budget {
    steps <= 6
    tokens <= 20000
  }

  policy {
    network allow ["arxiv.org"]
  }

  flow {
    hits = call web.search(topic)
    emit report = ask<markdown>("Write from ${hits}.")
  }
}
"#;

#[test]
fn a_policy_widened_through_the_canvas_is_compiled_like_any_other_edit() {
    // The canvas may widen a policy, and it cannot write its way past a check:
    // all it can do is write source, and the source is compiled.
    let rendered = canvas(WITH_REACH);
    let rule = rendered
        .policy
        .iter()
        .find(|rule| rule.subject == "network")
        .expect("a network rule");
    let action = rule
        .leaves
        .iter()
        .find(|leaf| leaf.role == "action")
        .expect("an action leaf");

    assert_eq!(diagnostics_of(WITH_REACH), "", "the file starts clean");

    let edit = replace_leaf(WITH_REACH, action, "allow [\"example.com\"]");
    let after = apply(WITH_REACH, &edit).expect("a policy edit applies");

    let codes = diagnostics_of(&after);
    assert!(
        codes.contains("ING4009"),
        "a tool's declared reach outside the policy is still refused: {codes}\n{after}"
    );
}

// --- 8 ----------------------------------------------------------------------

const POLICY_WITH_COMMENTS: &str = r#"language 0.1

agent Commented(topic: string) -> report<markdown> {
  budget {
    steps <= 4
    tokens <= 20000
  }

  policy {
    // Nothing here should reach the network.
    network deny
    // Writing outside the workspace is never right for this one.
    external_write deny
    secrets deny export
  }

  flow {
    emit report = ask<markdown>("Write about ${topic}.")
  }
}
"#;

#[test]
fn editing_one_policy_rule_leaves_every_comment_between_the_rules_untouched() {
    let rendered = canvas(POLICY_WITH_COMMENTS);
    let rule = rendered
        .policy
        .iter()
        .find(|rule| rule.subject == "external_write")
        .expect("an external_write rule");
    let action = rule
        .leaves
        .iter()
        .find(|leaf| leaf.role == "action")
        .expect("an action leaf");

    let edit = replace_leaf(POLICY_WITH_COMMENTS, action, "allow");
    let after = apply(POLICY_WITH_COMMENTS, &edit).expect("a policy edit applies");

    for comment in [
        "// Nothing here should reach the network.",
        "// Writing outside the workspace is never right for this one.",
    ] {
        assert!(after.contains(comment), "`{comment}` was lost:\n{after}");
    }
    assert!(after.contains("external_write allow"), "{after}");
    assert!(after.contains("network deny"), "{after}");
}

// --- the other two gestures -------------------------------------------------

#[test]
fn an_insert_is_a_replacement_of_an_empty_range() {
    // Which is why the canvas needs no second mechanism for it.
    let rendered = canvas(WITH_COMMENT);
    let boundary = &rendered.boundaries[1];

    let edit = insert_at(boundary, "checkpoint \"half-way\"");
    assert_eq!(
        edit.start_byte, edit.end_byte,
        "an insert covers no existing bytes"
    );
    assert!(edit.expected.is_empty());

    let after = apply(WITH_COMMENT, &edit).expect("an insert applies");
    assert!(after.contains("checkpoint \"half-way\""), "{after}");
    // Everything that was there is still there.
    for line in WITH_COMMENT.lines() {
        assert!(after.contains(line), "`{line}` was lost:\n{after}");
    }
}

#[test]
fn deleting_a_block_takes_its_comment_and_nothing_else() {
    let rendered = canvas(WITH_COMMENT);
    let queries = find(&rendered, "queries = ask");

    let edit = delete_block(WITH_COMMENT, &queries);
    let after = apply(WITH_COMMENT, &edit).expect("a delete applies");

    assert!(!after.contains("queries = ask"), "{after}");
    assert!(
        !after.contains("// The queries decide"),
        "a statement's comment goes with it:\n{after}"
    );
    assert!(after.contains("first = ask"), "{after}");
    assert!(after.contains("emit report"), "{after}");
}

#[test]
fn an_edge_is_derived_from_the_text_and_names_the_binding_that_ties_it() {
    let rendered = canvas(WITH_COMMENT);
    let first = find(&rendered, "first = ask");
    let emit = find(&rendered, "emit report");

    let edge = rendered
        .edges
        .iter()
        .find(|edge| edge.from == first.id && edge.to == emit.id)
        .expect("the emit reads `first`, so there is an edge");
    assert_eq!(edge.name, "first");
}

#[test]
fn a_name_that_is_only_a_prefix_of_another_draws_no_edge() {
    // `topic` must not match `topics`, or the canvas would show a dependency
    // the program does not have.
    let source = WITH_COMMENT.replace("first = ask", "firstly = ask");
    let rendered = canvas(&source);
    let emit = find(&rendered, "emit report");

    assert!(
        !rendered
            .edges
            .iter()
            .any(|edge| edge.to == emit.id && edge.name == "firstly"),
        "`firstly` is not read by an emit that mentions `first`"
    );
}
