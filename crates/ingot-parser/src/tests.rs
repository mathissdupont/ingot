use super::*;
use ingot_source::SourceMap;
use ingot_syntax::printer::print_program;

fn parse_text(text: &str) -> ParseResult {
    let mut map = SourceMap::new();
    let file = map.add_virtual("test.ing", text);
    parse(map.file(file))
}

const RESEARCH_AGENT: &str = r#"
language 0.1
package heptapus.research

type search_result {
  title: string
  url: string
  snippet: string
}

tool web.search(query: string) -> search_result[] !network

verifier CitationCheck(draft: markdown, min_sources: int)

/// Produces a source-grounded report on a topic.
agent ResearchAgent(topic: string) -> report<markdown> {
  model requires {
    tool_calling
    structured_output
    context >= 128k
  }

  tools {
    mcp web.search
  }

  memory {
    working ephemeral {
      seen: string[]
    }
  }

  budget {
    steps <= 60
    tokens <= 120000
    cost <= 5 usd
  }

  policy {
    network allow ["arxiv.org", "github.com"]
    secrets deny export
  }

  flow {
    queries = ask<string[]>("Create diverse research queries for: ${topic}")
    state.seen = queries
    sources = parallel map queries as query {
      call web.search(query)
    }
    draft = ask<markdown>("Produce a source-grounded report.", context: sources)
    verify CitationCheck(draft, min_sources: 8)
    emit report = draft
  }
}
"#;

#[test]
fn parses_the_reference_research_agent() {
    let result = parse_text(RESEARCH_AGENT);
    assert!(
        !result.diagnostics.has_errors(),
        "unexpected errors: {:?}",
        result
            .diagnostics
            .iter()
            .map(|d| &d.message)
            .collect::<Vec<_>>()
    );

    let program = result.program;
    assert_eq!(
        program.language.map(|version| version.text()).as_deref(),
        Some("0.1")
    );
    assert_eq!(
        program.package.map(|name| name.text()).as_deref(),
        Some("heptapus.research")
    );
    assert_eq!(program.types.len(), 1);
    assert_eq!(program.tools.len(), 1);
    assert_eq!(program.verifiers.len(), 1);
    assert_eq!(program.agents.len(), 1);

    let agent = &program.agents[0];
    assert_eq!(agent.name.text, "ResearchAgent");
    assert_eq!(
        agent.doc.as_deref(),
        Some("Produces a source-grounded report on a topic.")
    );
    assert_eq!(agent.output.as_ref().unwrap().name.text, "report");
    assert_eq!(agent.output.as_ref().unwrap().content.text, "markdown");
    assert_eq!(agent.flow.as_ref().unwrap().statements.len(), 6);
}

#[test]
fn records_declared_tool_effects() {
    let result = parse_text(RESEARCH_AGENT);
    let tool = &result.program.tools[0];
    assert_eq!(tool.name.text(), "web.search");
    assert_eq!(tool.ret.text(), "search_result[]");
    assert_eq!(tool.effects.len(), 1);
    assert_eq!(tool.effects[0].name.text, "network");
}

#[test]
fn parses_import_declarations() {
    let source = r#"
language 0.2
package heptapus.research

import "./shared/web.ing" {
  type search_result
  tool web.search
  verifier CitationCheck
}

agent ResearchAgent(topic: string) -> report<markdown> {
  flow {
    emit report = ask<markdown>("done")
  }
}
"#;
    let result = parse_text(source);
    assert!(!result.diagnostics.has_errors(), "{:?}", result.diagnostics);
    assert_eq!(result.program.imports.len(), 1);
    let import = &result.program.imports[0];
    assert_eq!(import.path.plain_text(), "./shared/web.ing");
    assert_eq!(import.items.len(), 3);
    assert_eq!(import.items[0].kind, ImportKind::Type);
    assert_eq!(import.items[0].name.text(), "search_result");
    assert_eq!(import.items[1].kind, ImportKind::Tool);
    assert_eq!(import.items[1].name.text(), "web.search");
    assert_eq!(import.items[2].kind, ImportKind::Verifier);
    assert_eq!(import.items[2].name.text(), "CitationCheck");
}

#[test]
fn parses_optional_and_union_types_in_language_0_2() {
    let source = r#"
language 0.2

type maybe_page {
  title: string?
  body: markdown | text
  attachments: (file | bytes)[]
}

tool web.fetch(url: string) -> maybe_page?

agent A(topic: string) -> report<markdown> {
  flow {
    draft = ask<markdown | text>("write")
    emit report = ask<markdown>("done", context: draft)
  }
}
"#;
    let result = parse_text(source);
    assert!(!result.diagnostics.has_errors(), "{:?}", result.diagnostics);
    let fields = &result.program.types[0].fields;
    assert_eq!(fields[0].ty.text(), "string?");
    assert_eq!(fields[1].ty.text(), "markdown | text");
    assert_eq!(fields[2].ty.text(), "(file | bytes)[]");
    assert_eq!(result.program.tools[0].ret.text(), "maybe_page?");
}

#[test]
fn parses_pure_function_declarations_and_calls() {
    let source = r#"
language 0.2

fn headline(title: string, fallback: string) -> string = title

agent A(topic: string) -> report<markdown> {
  flow {
    subject = headline(topic, fallback: "Untitled")
    emit report = ask<markdown>("Write ${subject}")
  }
}
"#;
    let result = parse_text(source);
    assert!(!result.diagnostics.has_errors(), "{:?}", result.diagnostics);
    assert_eq!(result.program.functions.len(), 1);
    let function = &result.program.functions[0];
    assert_eq!(function.name.text, "headline");
    assert_eq!(function.params.len(), 2);
    assert_eq!(function.ret.text(), "string");

    let Stmt::Bind {
        value: Expr::FunctionCall { callee, args, .. },
        ..
    } = &result.program.agents[0].flow.as_ref().unwrap().statements[0]
    else {
        panic!("expected a helper call binding");
    };
    assert_eq!(callee.text, "headline");
    assert_eq!(args.len(), 2);
    assert_eq!(args[1].name.as_ref().unwrap().text, "fallback");
}

#[test]
fn function_declarations_require_language_0_2() {
    let result = parse_text(
        r#"
language 0.1
fn id(value: string) -> string = value
"#,
    );
    assert!(result
        .diagnostics
        .iter()
        .any(|d| d.code == codes::UNSUPPORTED_LANGUAGE_VERSION));
}

#[test]
fn verifier_bodies_require_language_0_2() {
    let result = parse_text(
        r#"
language 0.1
verifier Enough(n: int) = n >= 3
"#,
    );
    assert!(result
        .diagnostics
        .iter()
        .any(|d| d.code == codes::UNSUPPORTED_LANGUAGE_VERSION));
}

#[test]
fn a_verifier_without_a_body_is_still_language_0_1() {
    let result = parse_text(
        r#"
language 0.1
verifier Enough(n: int)
"#,
    );
    assert!(
        !result
            .diagnostics
            .iter()
            .any(|d| d.code == codes::UNSUPPORTED_LANGUAGE_VERSION),
        "a name and a signature is what 0.1 always had"
    );
    assert!(result.program.verifiers[0].body.is_none());
}

#[test]
fn a_verifier_body_parses_as_an_expression() {
    let result = parse_text(
        r#"
language 0.2
verifier Enough(items: string[], min: int) = len(items) >= min
"#,
    );
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let body = result.program.verifiers[0]
        .body
        .as_ref()
        .expect("the declaration carried a body");
    assert!(matches!(body, ingot_syntax::Expr::Binary { .. }));
}

#[test]
fn optional_and_union_types_require_language_0_2() {
    let result = parse_text(
        r#"
language 0.1
type page {
  title: string?
}
"#,
    );
    assert!(result
        .diagnostics
        .iter()
        .any(|d| d.code == codes::UNSUPPORTED_LANGUAGE_VERSION));
}

#[test]
fn import_requires_language_0_2() {
    let result = parse_text(
        r#"
language 0.1
import "./shared.ing" {
  type shared
}
"#,
    );
    assert!(result
        .diagnostics
        .iter()
        .any(|d| d.code == codes::UNSUPPORTED_LANGUAGE_VERSION));
}

#[test]
fn reads_context_requirement_in_tokens() {
    let result = parse_text(RESEARCH_AGENT);
    let Some(ModelBlock::Requires { requirements, .. }) = &result.program.agents[0].model else {
        panic!("expected a `model requires` block");
    };
    let context = requirements
        .iter()
        .find_map(|requirement| match requirement {
            ModelRequirement::ContextAtLeast { tokens, .. } => Some(*tokens),
            _ => None,
        })
        .expect("expected a context requirement");
    assert_eq!(context, 131_072);
}

#[test]
fn formatting_is_idempotent() {
    let result = parse_text(RESEARCH_AGENT);
    assert!(!result.diagnostics.has_errors());
    let once = print_program(&result.program);

    let mut map = SourceMap::new();
    let file = map.add_virtual("formatted.ing", once.clone());
    let reparsed = parse(map.file(file));
    assert!(
        !reparsed.diagnostics.has_errors(),
        "formatted output must reparse cleanly: {:?}",
        reparsed
            .diagnostics
            .iter()
            .map(|d| &d.message)
            .collect::<Vec<_>>()
    );

    let twice = print_program(&reparsed.program);
    assert_eq!(once, twice, "formatting must reach a fixed point");
}

#[test]
fn requires_a_language_declaration() {
    let result = parse_text("package a\nagent A() -> out<markdown> { flow { } }\n");
    assert!(result
        .diagnostics
        .iter()
        .any(|d| d.code == codes::MISSING_LANGUAGE_DECLARATION));
}

#[test]
fn rejects_an_unknown_language_version() {
    let result = parse_text("language 9.4\nagent A() -> out<markdown> { flow { } }\n");
    assert!(result
        .diagnostics
        .iter()
        .any(|d| d.code == codes::UNSUPPORTED_LANGUAGE_VERSION));
}

#[test]
fn reports_several_independent_errors_in_one_run() {
    let source = r#"
language 0.1

agent First( -> out<markdown> {
  flow { }
}

agent Second() -> out<markdown> {
  bogus { }
  flow { }
}
"#;
    let result = parse_text(source);
    assert!(
        result.diagnostics.len() >= 2,
        "expected multiple diagnostics"
    );
    // Recovery must still reach the second agent.
    assert!(result
        .program
        .agents
        .iter()
        .any(|agent| agent.name.text == "Second"));
}

#[test]
fn reports_duplicate_sections() {
    let source = r#"
language 0.1
agent A() -> out<markdown> {
  flow { }
  flow { }
}
"#;
    let result = parse_text(source);
    assert!(result
        .diagnostics
        .iter()
        .any(|d| d.code == codes::DUPLICATE_SECTION));
}

#[test]
fn distinguishes_state_writes_from_state_reads() {
    let source = r#"
language 0.1
agent A() -> out<markdown> {
  memory { working ephemeral { note: string } }
  flow {
    state.note = "hello"
    copy = state.note
    emit out = ask<markdown>("done")
  }
}
"#;
    let result = parse_text(source);
    assert!(!result.diagnostics.has_errors(), "{:?}", result.diagnostics);
    let statements = &result.program.agents[0].flow.as_ref().unwrap().statements;
    assert!(matches!(statements[0], Stmt::StateWrite { .. }));
    assert!(matches!(statements[1], Stmt::Bind { .. }));
}

#[test]
fn parses_branches_and_bounded_loops() {
    let source = r#"
language 0.1
agent A(n: int) -> out<markdown> {
  flow {
    if n > 3 {
      a = ask<string>("big")
    } else {
      a = ask<string>("small")
    }
    loop max 5 while n > 0 {
      b = ask<string>("again")
    }
    emit out = ask<markdown>("done")
  }
}
"#;
    let result = parse_text(source);
    assert!(!result.diagnostics.has_errors(), "{:?}", result.diagnostics);
    let statements = &result.program.agents[0].flow.as_ref().unwrap().statements;
    assert!(matches!(
        statements[0],
        Stmt::If {
            else_branch: Some(_),
            ..
        }
    ));
    let Stmt::Loop { max, guard, .. } = &statements[1] else {
        panic!("expected a loop")
    };
    assert_eq!(max.unwrap().value, 5);
    assert!(guard.is_some());
}

#[test]
fn parses_named_and_positional_arguments() {
    let source = r#"
language 0.1
agent A() -> out<markdown> {
  flow {
    draft = ask<markdown>("write", context: 3, temperature: 0.2)
    emit out = draft
  }
}
"#;
    let result = parse_text(source);
    assert!(!result.diagnostics.has_errors(), "{:?}", result.diagnostics);
    let Stmt::Bind {
        value: Expr::Ask { args, .. },
        ..
    } = &result.program.agents[0].flow.as_ref().unwrap().statements[0]
    else {
        panic!("expected an ask binding");
    };
    assert_eq!(args.len(), 3);
    assert!(args[0].name.is_none());
    assert_eq!(args[1].name.as_ref().unwrap().text, "context");
    assert_eq!(args[2].name.as_ref().unwrap().text, "temperature");
}

#[test]
fn never_loops_forever_on_truncated_input() {
    for source in [
        "language 0.1\nagent",
        "language 0.1\nagent A(",
        "language 0.1\nagent A() -> out<markdown> {",
        "language 0.1\nagent A() -> out<markdown> { flow {",
        "language 0.1\ntool",
        "language 0.1\ntype T {",
    ] {
        let result = parse_text(source);
        assert!(
            result.diagnostics.has_errors(),
            "expected errors for {source:?}"
        );
    }
}
