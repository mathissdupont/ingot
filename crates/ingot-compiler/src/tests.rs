//! Lowering tests.
//!
//! These pin down the structural guarantees the IR promises: deterministic ids,
//! flat regions, hoisted calls, explicit state reads and inserted approvals.

use ingot_ir::{NodeKind, RefScope, Value};

use std::fs;
use std::path::PathBuf;

use crate::{compile_path, compile_source};

const PRELUDE: &str = r#"language 0.1
package heptapus.test

type search_result {
  title: string
  url: string
}

tool web.search(query: string) -> search_result[] !network
tool mailer.send(to: string, body: string) -> bool !external_write

verifier CitationCheck(draft: markdown, min_sources: int)
"#;

fn compile(body: &str) -> crate::Compilation {
    let compilation = compile_source("test.ing", format!("{PRELUDE}\n{body}"));
    assert!(
        !compilation.has_errors(),
        "expected a clean compile:\n{}",
        compilation.render_diagnostics(ingot_diagnostics::ColorChoice::Never)
    );
    compilation
}

fn temp_project(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ingot-{name}-{}", std::process::id()));
    if dir.exists() {
        fs::remove_dir_all(&dir).expect("removing stale test directory");
    }
    fs::create_dir_all(&dir).expect("creating test directory");
    dir
}

const RESEARCH: &str = r#"
agent ResearchAgent(topic: string) -> report<markdown> {
  model requires {
    tool_calling
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
    cost <= 5 usd
  }

  policy {
    network allow ["arxiv.org"]
  }

  flow {
    queries = ask<string[]>("Queries for: ${topic}")
    state.seen = queries
    sources = parallel map queries as query {
      call web.search(query)
    }
    draft = ask<markdown>("Write it up.", context: sources)
    verify CitationCheck(draft, min_sources: 8)
    emit report = draft
  }
}
"#;

#[test]
fn lowers_the_reference_agent() {
    let compilation = compile(RESEARCH);
    let ir = compilation
        .primary_agent()
        .expect("expected one lowered agent");

    assert_eq!(ir.ir_version, "0.1");
    assert_eq!(ir.language, "0.1");
    assert_eq!(ir.agent, "heptapus.test.ResearchAgent");
    assert_eq!(ir.inputs.get("topic").map(String::as_str), Some("string"));
    assert_eq!(
        ir.outputs.get("report").map(String::as_str),
        Some("artifact<markdown>")
    );
    assert_eq!(ir.state.get("seen").map(String::as_str), Some("string[]"));
    assert_eq!(ir.budget.steps, Some(60));
    assert_eq!(ir.budget.cost.as_ref().unwrap().amount, "5");
    assert_eq!(
        ir.effects,
        vec!["model_access".to_string(), "network".to_string()]
    );
    assert_eq!(ir.tools.len(), 1);
    assert_eq!(ir.tools[0].reference, "mcp:web.search");
}

#[test]
fn node_ids_are_assigned_in_source_order() {
    let compilation = compile(RESEARCH);
    let ir = compilation.primary_agent().unwrap();
    let ids: Vec<&str> = ir.nodes.iter().map(|node| node.id.as_str()).collect();
    let expected: Vec<String> = (0..ir.nodes.len())
        .map(|index| format!("n{index}"))
        .collect();
    assert_eq!(ids, expected.iter().map(String::as_str).collect::<Vec<_>>());
}

#[test]
fn the_main_path_follows_next_pointers_and_terminates() {
    let compilation = compile(RESEARCH);
    let ir = compilation.primary_agent().unwrap();

    let kinds: Vec<NodeKind> = ir.main_path().iter().map(|node| node.kind).collect();
    assert_eq!(
        kinds,
        vec![
            NodeKind::LlmCall,    // queries
            NodeKind::StateWrite, // state.seen = queries
            NodeKind::Parallel,   // sources
            NodeKind::LlmCall,    // draft
            NodeKind::Verify,
            NodeKind::ArtifactEmit,
        ]
    );
    assert!(ir.main_path().last().unwrap().next.is_none());
}

#[test]
fn a_parallel_body_lives_in_the_flat_array_and_self_terminates() {
    let compilation = compile(RESEARCH);
    let ir = compilation.primary_agent().unwrap();

    let parallel = ir
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Parallel)
        .expect("expected a parallel node");
    assert_eq!(parallel.mode.as_deref(), Some("map"));
    assert_eq!(parallel.binder.as_deref(), Some("query"));

    let body_id = parallel
        .body
        .as_deref()
        .expect("parallel must point at its body");
    let body = ir
        .node(body_id)
        .expect("body node must exist in the flat array");
    assert_eq!(body.kind, NodeKind::ToolCall);
    assert_eq!(body.tool.as_deref(), Some("mcp:web.search"));
    assert!(
        body.next.is_none(),
        "the last node of a region terminates it"
    );
}

#[test]
fn prompts_become_typed_templates() {
    let compilation = compile(RESEARCH);
    let ir = compilation.primary_agent().unwrap();

    let first = ir.main_path()[0];
    let Some(Value::Template { parts }) = &first.prompt else {
        panic!("expected an interpolated prompt to lower to a template");
    };
    assert_eq!(parts.len(), 2);
    let ingot_ir::TemplatePart::Value { value, ty } = &parts[1] else {
        panic!("expected the placeholder to resolve to a value");
    };
    assert_eq!(
        *value,
        Value::Ref {
            scope: RefScope::Input,
            path: vec!["topic".to_string()]
        }
    );
    assert_eq!(ty, "string");
}

#[test]
fn a_prompt_without_placeholders_stays_a_literal() {
    let compilation = compile(RESEARCH);
    let ir = compilation.primary_agent().unwrap();
    let draft = ir.main_path()[3];
    assert!(matches!(draft.prompt, Some(Value::Literal { .. })));
}

#[test]
fn reading_state_emits_an_explicit_read_node() {
    let compilation = compile(
        r#"
agent A(topic: string) -> report<markdown> {
  memory { working ephemeral { notes: string } }
  flow {
    state.notes = topic
    emit report = ask<markdown>("Continue from ${state.notes}")
  }
}
"#,
    );
    let ir = compilation.primary_agent().unwrap();
    let read = ir
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::StateRead)
        .expect("expected a state.read node");
    assert_eq!(read.field.as_deref(), Some("notes"));
    assert_eq!(read.binding.as_deref(), Some("$state.notes"));
}

#[test]
fn one_state_read_is_emitted_per_field_per_statement() {
    let compilation = compile(
        r#"
agent A(topic: string) -> report<markdown> {
  memory { working ephemeral { notes: string } }
  flow {
    state.notes = topic
    emit report = ask<markdown>("${state.notes} and again ${state.notes}")
  }
}
"#,
    );
    let ir = compilation.primary_agent().unwrap();
    let reads = ir
        .nodes
        .iter()
        .filter(|node| node.kind == NodeKind::StateRead)
        .count();
    assert_eq!(reads, 1, "repeated reads in one statement share a node");
}

#[test]
fn nested_calls_are_hoisted_into_their_own_nodes() {
    let compilation = compile(
        r#"
agent A(topic: string) -> report<markdown> {
  tools { mcp web.search }
  policy { network allow ["example.com"] }
  flow {
    emit report = ask<markdown>("Summarise", context: call web.search(topic))
  }
}
"#,
    );
    let ir = compilation.primary_agent().unwrap();
    let path = ir.main_path();
    assert_eq!(
        path[0].kind,
        NodeKind::ToolCall,
        "the nested call runs first"
    );
    let binding = path[0]
        .binding
        .clone()
        .expect("a hoisted call binds its result");
    assert!(binding.starts_with("$tmp"));

    assert_eq!(path[1].kind, NodeKind::LlmCall);
    let context = &path[1].args[0];
    assert_eq!(context.name, "context");
    assert_eq!(
        context.value,
        Value::Ref {
            scope: RefScope::Binding,
            path: vec![binding]
        }
    );
}

#[test]
fn an_approval_node_is_inserted_before_a_gated_call() {
    let compilation = compile(
        r#"
agent A(topic: string) -> report<markdown> {
  tools { mcp mailer.send }
  policy { external_write require approval }
  flow {
    sent = call mailer.send(topic, "body")
    emit report = ask<markdown>("done", context: sent)
  }
}
"#,
    );
    let ir = compilation.primary_agent().unwrap();
    let path = ir.main_path();
    assert_eq!(path[0].kind, NodeKind::Approval);
    assert_eq!(path[0].effects, vec!["external_write".to_string()]);
    assert!(path[0].label.as_deref().unwrap().contains("mailer.send"));
    assert_eq!(path[1].kind, NodeKind::ToolCall);
}

#[test]
fn no_approval_node_appears_when_the_policy_simply_allows() {
    let compilation = compile(RESEARCH);
    let ir = compilation.primary_agent().unwrap();
    assert!(!ir.nodes.iter().any(|node| node.kind == NodeKind::Approval));
}

#[test]
fn pure_bindings_are_inlined_instead_of_becoming_nodes() {
    let compilation = compile(
        r#"
agent A(topic: string) -> report<markdown> {
  flow {
    subject = topic
    emit report = ask<markdown>("Write about ${subject}")
  }
}
"#,
    );
    let ir = compilation.primary_agent().unwrap();
    assert_eq!(ir.nodes.len(), 2, "the alias must not become a node");

    let Some(Value::Template { parts }) = &ir.main_path()[0].prompt else {
        panic!("expected a template prompt");
    };
    let ingot_ir::TemplatePart::Value { value, .. } = &parts[1] else {
        panic!("expected a substitution part");
    };
    assert_eq!(
        *value,
        Value::Ref {
            scope: RefScope::Input,
            path: vec!["topic".to_string()]
        },
        "the alias resolves back to the input it aliased"
    );
}

#[test]
fn branches_reference_their_arms_by_node_id() {
    let compilation = compile(
        r#"
agent A(n: int) -> report<markdown> {
  flow {
    if n > 1 {
      emit report = ask<markdown>("big")
    } else {
      emit report = ask<markdown>("small")
    }
  }
}
"#,
    );
    let ir = compilation.primary_agent().unwrap();
    let branch = &ir.nodes[0];
    assert_eq!(branch.kind, NodeKind::Branch);

    let then_id = branch.then.clone().expect("expected a then arm");
    let else_id = branch.otherwise.clone().expect("expected an else arm");
    assert_ne!(then_id, else_id);
    assert_eq!(ir.node(&then_id).unwrap().kind, NodeKind::LlmCall);
    assert_eq!(ir.node(&else_id).unwrap().kind, NodeKind::LlmCall);
}

#[test]
fn loops_carry_their_static_bound() {
    let compilation = compile(
        r#"
agent A(topic: string) -> report<markdown> {
  memory { working ephemeral { notes: string } }
  flow {
    loop max 3 {
      state.notes = topic
    }
    emit report = ask<markdown>("done")
  }
}
"#,
    );
    let ir = compilation.primary_agent().unwrap();
    let loop_node = ir
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Loop)
        .unwrap();
    assert_eq!(loop_node.max_iterations, Some(3));
    assert!(loop_node.body.is_some());
}

#[test]
fn compiling_the_same_source_twice_gives_identical_bytes() {
    let first = compile(RESEARCH)
        .primary_agent()
        .unwrap()
        .to_canonical_json();
    let second = compile(RESEARCH)
        .primary_agent()
        .unwrap()
        .to_canonical_json();
    assert_eq!(first, second);
}

#[test]
fn a_program_with_errors_produces_no_ir() {
    let compilation = compile_source(
        "broken.ing",
        format!("{PRELUDE}\nagent A(topic: string) -> report<markdown> {{ flow {{ }} }}\n"),
    );
    assert!(compilation.has_errors());
    assert!(
        compilation.agents.is_empty(),
        "a failed build must not emit an artifact"
    );
}

#[test]
fn arguments_are_normalised_into_declaration_order() {
    let compilation = compile(
        r#"
agent A(topic: string) -> report<markdown> {
  tools { mcp mailer.send }
  policy { external_write allow }
  flow {
    sent = call mailer.send(body: "hello", to: topic)
    emit report = ask<markdown>("done", context: sent)
  }
}
"#,
    );
    let ir = compilation.primary_agent().unwrap();
    let call = ir
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::ToolCall)
        .unwrap();
    let names: Vec<&str> = call.args.iter().map(|arg| arg.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["to", "body"],
        "named arguments follow the declaration order"
    );
}

#[test]
fn imported_type_tool_and_verifier_are_available_to_the_entry_file() {
    let dir = temp_project("imports-work");
    let shared_dir = dir.join("shared");
    fs::create_dir_all(&shared_dir).expect("creating shared directory");
    fs::write(
        shared_dir.join("web.ing"),
        r#"
language 0.2

type search_result {
  title: string
  url: string
}

tool web.search(query: string) -> search_result[] !network

verifier CitationCheck(draft: markdown, min_sources: int)
"#,
    )
    .expect("writing imported file");
    let entry = dir.join("main.ing");
    fs::write(
        &entry,
        r#"
language 0.2
package heptapus.test

import "./shared/web.ing" {
  type search_result
  tool web.search
  verifier CitationCheck
}

agent A(topic: string) -> report<markdown> {
  tools { mcp web.search }
  policy { network allow ["example.com"] }
  flow {
    sources = call web.search(topic)
    draft = ask<markdown>("done", context: sources)
    verify CitationCheck(draft, min_sources: 1)
    emit report = draft
  }
}
"#,
    )
    .expect("writing entry file");

    let compilation = compile_path(&entry).expect("entry must be readable");
    assert!(
        !compilation.has_errors(),
        "expected imported declarations to compile:\n{}",
        compilation.render_diagnostics(ingot_diagnostics::ColorChoice::Never)
    );
    let ir = compilation.primary_agent().expect("expected one agent");
    assert_eq!(ir.language, "0.2");
    assert!(ir.types.contains_key("search_result"));
    assert_eq!(ir.tools[0].name, "web.search");
}

#[test]
fn optional_and_union_types_lower_as_canonical_type_text() {
    let compilation = compile_source(
        "optional.ing",
        r#"
language 0.2
package heptapus.test

type page {
  title: string?
  body: markdown | text
}

tool web.fetch(url: string) -> page? !network

agent A(url: string) -> report<markdown> {
  tools { mcp web.fetch }
  policy { network allow ["example.com"] }
  flow {
    page = call web.fetch(url)
    emit report = ask<markdown>("summarise", context: page)
  }
}
"#,
    );
    assert!(
        !compilation.has_errors(),
        "expected optional/union signatures to compile:\n{}",
        compilation.render_diagnostics(ingot_diagnostics::ColorChoice::Never)
    );
    let ir = compilation.primary_agent().expect("expected an agent");
    assert_eq!(ir.language, "0.2");
    let page = ir.types.get("page").expect("record should lower");
    let fields: Vec<(&str, &str)> = page
        .fields
        .iter()
        .map(|field| (field.name.as_str(), field.ty.as_str()))
        .collect();
    assert_eq!(
        fields,
        vec![("title", "string?"), ("body", "markdown | text")]
    );
    assert_eq!(ir.tools[0].signature.result, "page?");
}

#[test]
fn optional_values_do_not_assign_to_required_slots_without_narrowing() {
    let compilation = compile_source(
        "optional-error.ing",
        r#"
language 0.2
agent A() -> report<markdown> {
  flow {
    draft = ask<markdown?>("maybe")
    emit report = draft
  }
}
"#,
    );
    assert!(compilation.has_errors());
    assert!(compilation
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.code == ingot_diagnostics::codes::EMIT_TYPE_MISMATCH));
}

#[test]
fn parent_traversal_import_paths_are_rejected() {
    let dir = temp_project("imports-parent-traversal");
    let entry = dir.join("main.ing");
    fs::write(
        &entry,
        r#"
language 0.2
import "../shared.ing" {
  type shared
}
"#,
    )
    .expect("writing entry file");

    let compilation = compile_path(&entry).expect("entry must be readable");
    assert!(compilation
        .diagnostics
        .iter()
        .any(|d| d.code == ingot_diagnostics::codes::IMPORT_RESOLUTION_ERROR));
}

#[test]
fn import_cycles_are_rejected() {
    let dir = temp_project("imports-cycle");
    fs::write(
        dir.join("a.ing"),
        r#"
language 0.2
import "./b.ing" {
  type B
}
type A {
  value: string
}
"#,
    )
    .expect("writing a.ing");
    fs::write(
        dir.join("b.ing"),
        r#"
language 0.2
import "./a.ing" {
  type A
}
type B {
  value: string
}
"#,
    )
    .expect("writing b.ing");

    let compilation = compile_path(dir.join("a.ing")).expect("entry must be readable");
    assert!(compilation
        .diagnostics
        .iter()
        .any(|d| d.message.contains("import cycle")));
}
