//! Behavioural tests for the checker.
//!
//! Each test names the diagnostic code it pins down, so a rule can never be
//! silently dropped: removing the check breaks the test that documents it.

use ingot_diagnostics::codes;
use ingot_source::SourceMap;
use ingot_types::{Effect, PolicyDecision, Ty};

use crate::{analyze, Analysis, CallTarget, ModelInfo};

const PRELUDE: &str = r#"language 0.1
package heptapus.test

type search_result {
  title: string
  url: string
}

tool web.search(query: string) -> search_result[] !network
tool files.write(path: string, data: bytes) -> file !filesystem_write
tool mailer.send(to: string, body: string) -> bool !external_write

verifier CitationCheck(draft: markdown, min_sources: int)
"#;

fn check(body: &str) -> Analysis {
    let source = format!("{PRELUDE}\n{body}");
    let mut map = SourceMap::new();
    let file = map.add_virtual("test.ing", source);
    let parsed = ingot_parser::parse(map.file(file));
    assert!(
        !parsed.diagnostics.has_errors(),
        "test source must parse cleanly: {:?}",
        parsed
            .diagnostics
            .iter()
            .map(|d| &d.message)
            .collect::<Vec<_>>()
    );
    analyze(&parsed.program)
}

fn codes_of(analysis: &Analysis) -> Vec<&str> {
    analysis.diagnostics.iter().map(|d| d.code).collect()
}

fn assert_clean(analysis: &Analysis) {
    let errors: Vec<_> = analysis
        .diagnostics
        .iter()
        .filter(|d| d.severity == ingot_diagnostics::Severity::Error)
        .map(|d| format!("{}: {}", d.code, d.message))
        .collect();
    assert!(errors.is_empty(), "expected no errors, got {errors:?}");
}

const GOOD_AGENT: &str = r#"
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
    queries = ask<string[]>("Create research queries for: ${topic}")
    state.seen = queries
    sources = parallel map queries as query {
      call web.search(query)
    }
    draft = ask<markdown>("Write a grounded report.", context: sources)
    verify CitationCheck(draft, min_sources: 8)
    emit report = draft
  }
}
"#;

#[test]
fn accepts_the_reference_agent() {
    let analysis = check(GOOD_AGENT);
    assert_clean(&analysis);

    let agent = analysis
        .agent("ResearchAgent")
        .expect("agent must be analysed");
    assert_eq!(agent.qualified_name, "heptapus.test.ResearchAgent");
    assert_eq!(
        agent.inputs,
        vec![crate::Param {
            name: "topic".into(),
            ty: Ty::String
        }]
    );
    assert_eq!(agent.output.as_ref().unwrap().content, Ty::Markdown);
    assert_eq!(
        agent.state,
        vec![crate::Param {
            name: "seen".into(),
            ty: Ty::list_of(Ty::String)
        }]
    );
    assert_eq!(agent.budget.steps, Some(60));
    assert_eq!(agent.budget.cost.as_ref().unwrap().currency, "usd");
    assert!(agent.effects.contains(Effect::Network));
    assert!(agent.effects.contains(Effect::ModelAccess));

    let ModelInfo::Requires {
        capabilities,
        context_tokens,
    } = &agent.model
    else {
        panic!("expected capability requirements");
    };
    assert_eq!(
        capabilities,
        &["structured_output".to_string(), "tool_calling".to_string()]
    );
    assert_eq!(*context_tokens, Some(131_072));
}

#[test]
fn parallel_map_yields_a_list_of_the_body_type() {
    let analysis = check(GOOD_AGENT);
    let agent_decl_types: Vec<String> = analysis
        .exprs
        .values()
        .map(|info| info.ty.to_string())
        .collect();
    assert!(
        agent_decl_types.contains(&"search_result[][]".to_string()),
        "mapping a tool returning search_result[] must yield search_result[][], got {agent_decl_types:?}"
    );
}

#[test]
fn resolves_calls_to_tools() {
    let analysis = check(GOOD_AGENT);
    let target = analysis
        .calls
        .values()
        .map(|info| info.target.clone())
        .next()
        .expect("expected one resolved call");
    assert_eq!(target, CallTarget::Tool("web.search".into()));
}

// --- the three checks the research document names as success criteria -----

#[test]
fn rejects_a_wrong_tool_argument_type() {
    let analysis = check(
        r#"
agent A(topic: string) -> report<markdown> {
  tools { mcp web.search }
  policy { network allow ["example.com"] }
  flow {
    hits = call web.search(42)
    emit report = ask<markdown>("done", context: hits)
  }
}
"#,
    );
    assert!(codes_of(&analysis).contains(&codes::TYPE_MISMATCH));
}

#[test]
fn rejects_a_denied_network_effect() {
    let analysis = check(
        r#"
agent A(topic: string) -> report<markdown> {
  tools { mcp web.search }
  policy { network deny }
  flow {
    hits = call web.search(topic)
    emit report = ask<markdown>("done", context: hits)
  }
}
"#,
    );
    assert!(codes_of(&analysis).contains(&codes::DENIED_CAPABILITY));
}

#[test]
fn rejects_a_missing_output() {
    let analysis = check(
        r#"
agent A(topic: string) -> report<markdown> {
  flow {
    draft = ask<markdown>("write about ${topic}")
    verify CitationCheck(draft, min_sources: 1)
  }
}
"#,
    );
    assert!(codes_of(&analysis).contains(&codes::OUTPUT_NEVER_EMITTED));
}

// --- default-deny ----------------------------------------------------------

#[test]
fn an_absent_policy_rule_denies_the_effect() {
    let analysis = check(
        r#"
agent A(topic: string) -> report<markdown> {
  tools { mcp web.search }
  flow {
    hits = call web.search(topic)
    emit report = ask<markdown>("done", context: hits)
  }
}
"#,
    );
    assert!(
        codes_of(&analysis).contains(&codes::MISSING_POLICY_RULE),
        "a missing rule must be reported distinctly from an explicit deny"
    );
}

#[test]
fn require_approval_is_reported_as_an_inserted_checkpoint() {
    let analysis = check(
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
    assert_clean(&analysis);
    assert!(codes_of(&analysis).contains(&codes::APPROVAL_INSERTED));

    let agent = analysis.agent("A").unwrap();
    assert_eq!(
        agent.decision_for(Effect::ExternalWrite),
        PolicyDecision::RequireApproval
    );
}

#[test]
fn model_access_never_needs_a_policy_rule() {
    let analysis = check(
        r#"
agent A(topic: string) -> report<markdown> {
  flow {
    emit report = ask<markdown>("write about ${topic}")
  }
}
"#,
    );
    assert_clean(&analysis);
}

// --- capability discipline -------------------------------------------------

#[test]
fn a_tool_must_be_granted_before_it_is_called() {
    let analysis = check(
        r#"
agent A(topic: string) -> report<markdown> {
  policy { network allow ["example.com"] }
  flow {
    hits = call web.search(topic)
    emit report = ask<markdown>("done", context: hits)
  }
}
"#,
    );
    assert!(codes_of(&analysis).contains(&codes::TOOL_NOT_GRANTED));
}

#[test]
fn an_unused_grant_is_a_warning() {
    let analysis = check(
        r#"
agent A(topic: string) -> report<markdown> {
  tools { mcp web.search }
  policy { network allow ["example.com"] }
  flow {
    emit report = ask<markdown>("write about ${topic}")
  }
}
"#,
    );
    assert!(codes_of(&analysis).contains(&codes::UNUSED_TOOL_GRANT));
    assert!(
        !analysis.has_errors(),
        "an unused grant must not fail the build"
    );
}

// --- prompts ---------------------------------------------------------------

#[test]
fn a_typo_in_a_prompt_placeholder_is_an_error() {
    let analysis = check(
        r#"
agent A(topic: string) -> report<markdown> {
  flow {
    emit report = ask<markdown>("write about ${topci}")
  }
}
"#,
    );
    assert!(codes_of(&analysis).contains(&codes::UNRESOLVED_INTERPOLATION));
}

#[test]
fn a_valid_placeholder_marks_its_binding_as_used() {
    let analysis = check(
        r#"
agent A(topic: string) -> report<markdown> {
  flow {
    emit report = ask<markdown>("write about ${topic}")
  }
}
"#,
    );
    assert!(!codes_of(&analysis).contains(&codes::UNUSED_BINDING));
}

// --- bounds ----------------------------------------------------------------

#[test]
fn an_unbounded_loop_is_rejected() {
    let analysis = check(
        r#"
agent A(topic: string) -> report<markdown> {
  flow {
    loop {
      _x = ask<string>("again")
    }
    emit report = ask<markdown>("done")
  }
}
"#,
    );
    assert!(codes_of(&analysis).contains(&codes::UNBOUNDED_LOOP));
}

#[test]
fn a_flow_that_cannot_fit_its_step_budget_is_rejected() {
    let analysis = check(
        r#"
agent A(topic: string) -> report<markdown> {
  budget { steps <= 2 }
  flow {
    a = ask<string>("one")
    b = ask<string>("two")
    c = ask<string>("three")
    emit report = ask<markdown>("four", context: a, system: b, max_tokens: 1)
  }
}
"#,
    );
    let messages: Vec<_> = analysis.diagnostics.iter().map(|d| d.code).collect();
    assert!(
        messages.contains(&codes::STATIC_STEPS_EXCEED_BUDGET),
        "expected a step budget error, got {messages:?}"
    );
}

#[test]
fn recursion_is_rejected() {
    let analysis = check(
        r#"
agent A(topic: string) -> report<markdown> {
  flow {
    inner = call A(topic)
    emit report = inner
  }
}
"#,
    );
    assert!(codes_of(&analysis).contains(&codes::RECURSIVE_AGENT));
}

// --- concurrency -----------------------------------------------------------

#[test]
fn state_writes_are_rejected_inside_parallel_map() {
    let analysis = check(
        r#"
agent A(topic: string) -> report<markdown> {
  tools { mcp web.search }
  memory { working ephemeral { seen: string } }
  policy { network allow ["example.com"] }
  flow {
    queries = ask<string[]>("queries for ${topic}")
    hits = parallel map queries as query {
      state.seen = query
      call web.search(query)
    }
    emit report = ask<markdown>("done", context: hits)
  }
}
"#,
    );
    assert!(codes_of(&analysis).contains(&codes::INVALID_IN_PARALLEL));
}

#[test]
fn a_parallel_body_must_end_in_an_expression() {
    let analysis = check(
        r#"
agent A(topic: string) -> report<markdown> {
  memory { working ephemeral { seen: string } }
  flow {
    queries = ask<string[]>("queries for ${topic}")
    hits = parallel map queries as query {
      inner = ask<string>("look at ${query}")
    }
    emit report = ask<markdown>("done", context: hits)
  }
}
"#,
    );
    assert!(codes_of(&analysis).contains(&codes::PARALLEL_BODY_MUST_YIELD_VALUE));
}

// --- outputs ---------------------------------------------------------------

#[test]
fn emitting_the_wrong_content_type_is_rejected() {
    let analysis = check(
        r#"
agent A(topic: string) -> report<markdown> {
  flow {
    draft = ask<string>("write about ${topic}")
    emit report = draft
  }
}
"#,
    );
    assert!(codes_of(&analysis).contains(&codes::EMIT_TYPE_MISMATCH));
}

#[test]
fn emitting_only_inside_a_branch_is_a_warning_not_an_error() {
    let analysis = check(
        r#"
agent A(n: int) -> report<markdown> {
  flow {
    if n > 1 {
      emit report = ask<markdown>("big")
    }
  }
}
"#,
    );
    assert!(codes_of(&analysis).contains(&codes::OUTPUT_NOT_ON_ALL_PATHS));
    assert!(!analysis.has_errors());
}

#[test]
fn emitting_in_both_branches_satisfies_the_check() {
    let analysis = check(
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
    assert_clean(&analysis);
    assert!(!codes_of(&analysis).contains(&codes::OUTPUT_NOT_ON_ALL_PATHS));
}

// --- scoping ---------------------------------------------------------------

#[test]
fn rebinding_a_name_in_the_same_block_is_rejected() {
    let analysis = check(
        r#"
agent A(topic: string) -> report<markdown> {
  flow {
    draft = ask<markdown>("one")
    draft = ask<markdown>("two")
    emit report = draft
  }
}
"#,
    );
    assert!(codes_of(&analysis).contains(&codes::DUPLICATE_DECLARATION));
}

#[test]
fn a_binding_does_not_escape_its_block() {
    let analysis = check(
        r#"
agent A(n: int) -> report<markdown> {
  flow {
    if n > 1 {
      inner = ask<markdown>("inner")
      emit report = inner
    } else {
      emit report = inner
    }
  }
}
"#,
    );
    assert!(codes_of(&analysis).contains(&codes::UNRESOLVED_NAME));
}

#[test]
fn reading_a_state_field_without_the_state_prefix_suggests_it() {
    let analysis = check(
        r#"
agent A(topic: string) -> report<markdown> {
  memory { working ephemeral { notes: string } }
  flow {
    state.notes = topic
    emit report = ask<markdown>("done", context: notes)
  }
}
"#,
    );
    let hint = analysis
        .diagnostics
        .iter()
        .find(|d| d.code == codes::UNRESOLVED_NAME)
        .and_then(|d| d.help.clone())
        .unwrap_or_default();
    assert!(
        hint.contains("state.notes"),
        "expected a `state.` hint, got {hint:?}"
    );
}

#[test]
fn an_unused_binding_is_a_warning() {
    let analysis = check(
        r#"
agent A(topic: string) -> report<markdown> {
  flow {
    scratch = ask<string>("unused")
    emit report = ask<markdown>("write about ${topic}")
  }
}
"#,
    );
    assert!(codes_of(&analysis).contains(&codes::UNUSED_BINDING));
    assert!(!analysis.has_errors());
}

#[test]
fn an_underscore_prefix_silences_the_unused_warning() {
    let analysis = check(
        r#"
agent A(topic: string) -> report<markdown> {
  flow {
    _scratch = ask<string>("deliberately unused")
    emit report = ask<markdown>("write about ${topic}")
  }
}
"#,
    );
    assert!(!codes_of(&analysis).contains(&codes::UNUSED_BINDING));
}

// --- records ---------------------------------------------------------------

#[test]
fn record_fields_are_typed() {
    let analysis = check(
        r#"
agent A(topic: string) -> report<markdown> {
  tools { mcp web.search }
  policy { network allow ["example.com"] }
  flow {
    hits = call web.search(topic)
    titles = parallel map hits as hit {
      ask<string>("summarise ${hit.title}")
    }
    emit report = ask<markdown>("done", context: titles)
  }
}
"#,
    );
    assert_clean(&analysis);
}

#[test]
fn an_unknown_record_field_is_rejected() {
    let analysis = check(
        r#"
agent A(topic: string) -> report<markdown> {
  tools { mcp web.search }
  policy { network allow ["example.com"] }
  flow {
    hits = call web.search(topic)
    titles = parallel map hits as hit {
      ask<string>("summarise ${hit.headline}")
    }
    emit report = ask<markdown>("done", context: titles)
  }
}
"#,
    );
    assert!(codes_of(&analysis).contains(&codes::UNKNOWN_FIELD));
}

// --- declarations ----------------------------------------------------------

#[test]
fn an_unknown_policy_subject_is_rejected_with_a_suggestion() {
    let analysis = check(
        r#"
agent A(topic: string) -> report<markdown> {
  policy { netwrok allow ["example.com"] }
  flow {
    emit report = ask<markdown>("write about ${topic}")
  }
}
"#,
    );
    let diagnostic = analysis
        .diagnostics
        .iter()
        .find(|d| d.code == codes::UNKNOWN_POLICY_SUBJECT)
        .expect("expected an unknown-subject error");
    assert_eq!(diagnostic.help.as_deref(), Some("did you mean `network`?"));
}

#[test]
fn a_cost_budget_needs_a_currency() {
    let analysis = check(
        r#"
agent A(topic: string) -> report<markdown> {
  budget { cost <= 5 }
  flow {
    emit report = ask<markdown>("write about ${topic}")
  }
}
"#,
    );
    assert!(codes_of(&analysis).contains(&codes::MISSING_COST_CURRENCY));
}

#[test]
fn only_mcp_tools_can_be_granted_in_v0_1() {
    let analysis = check(
        r#"
agent A(topic: string) -> report<markdown> {
  tools { grpc web.search }
  policy { network allow ["example.com"] }
  flow {
    hits = call web.search(topic)
    emit report = ask<markdown>("done", context: hits)
  }
}
"#,
    );
    assert!(codes_of(&analysis).contains(&codes::UNSUPPORTED_TRANSPORT));
}

#[test]
fn a_flowless_agent_is_rejected() {
    let analysis = check(
        r#"
agent A(topic: string) -> report<markdown> {
  budget { steps <= 1 }
}
"#,
    );
    assert!(codes_of(&analysis).contains(&codes::MISSING_FLOW_BLOCK));
}

#[test]
fn diagnostics_come_back_in_source_order() {
    let analysis = check(
        r#"
agent A(topic: string) -> report<markdown> {
  policy { bogus allow }
  flow {
    x = call nowhere.at_all(topic)
    emit report = ask<markdown>("done", context: x)
  }
}
"#,
    );
    let positions: Vec<u32> = analysis
        .diagnostics
        .iter()
        .filter_map(|d| d.primary_span())
        .map(|span| span.start)
        .collect();
    let mut sorted = positions.clone();
    sorted.sort_unstable();
    assert_eq!(positions, sorted, "diagnostics must be ordered by position");
}
