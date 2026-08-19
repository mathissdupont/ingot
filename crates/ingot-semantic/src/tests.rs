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

// --- a capability's reach (RFC-0014) --------------------------------------

/// An agent that calls one scoped tool, so a test varies only the two
/// declarations the containment check compares.
fn reaching(effects: &str, policy: &str) -> Analysis {
    check(&format!(
        r#"
tool feed.fetch(url: string) -> string {effects}

agent A(topic: string) -> report<markdown> {{
  tools {{ mcp feed.fetch }}
  policy {{ {policy} }}
  flow {{
    page = call feed.fetch(topic)
    emit report = ask<markdown>("done", context: page)
  }}
}}
"#
    ))
}

#[test]
fn a_tool_that_reaches_beyond_the_policy_is_a_compile_error() {
    // The check the values in a policy never had. Before it, adding a host to
    // the list changed nothing a compiler could see.
    let analysis = reaching(
        r#"!network("arxiv.org", "github.com")"#,
        r#"network allow ["arxiv.org"]"#,
    );
    assert!(
        codes_of(&analysis).contains(&codes::REACH_BEYOND_POLICY),
        "{:?}",
        codes_of(&analysis)
    );
    let reported = analysis
        .diagnostics
        .iter()
        .find(|d| d.code == codes::REACH_BEYOND_POLICY)
        .expect("the containment failure");
    assert!(reported.message.contains("github.com"), "{reported:?}");
    assert!(
        !reported.message.contains("arxiv.org"),
        "only the ungranted host is the problem: {reported:?}"
    );
}

#[test]
fn a_tool_within_the_policy_compiles() {
    let analysis = reaching(
        r#"!network("arxiv.org")"#,
        r#"network allow ["arxiv.org", "github.com"]"#,
    );
    assert!(
        !codes_of(&analysis).contains(&codes::REACH_BEYOND_POLICY),
        "{:?}",
        codes_of(&analysis)
    );
}

#[test]
fn an_unbounded_grant_contains_any_reach() {
    // A policy that names no value is unbounded, which is what keeps every
    // artifact written before RFC-0014 compiling.
    let analysis = reaching(r#"!network("anywhere.example")"#, "network allow");
    assert!(
        !codes_of(&analysis).contains(&codes::REACH_BEYOND_POLICY),
        "{:?}",
        codes_of(&analysis)
    );
}

#[test]
fn a_denied_effect_is_reported_once_rather_than_twice() {
    // Saying the effect is denied *and* that its reach is ungranted buries the
    // answer under the elaboration.
    let analysis = reaching(r#"!network("arxiv.org")"#, "network deny");
    assert!(codes_of(&analysis).contains(&codes::DENIED_CAPABILITY));
    assert!(!codes_of(&analysis).contains(&codes::REACH_BEYOND_POLICY));
}

#[test]
fn an_empty_reach_is_refused() {
    let analysis = reaching("!network()", r#"network allow ["arxiv.org"]"#);
    let reported = analysis
        .diagnostics
        .iter()
        .find(|d| d.code == codes::INVALID_EFFECT_REACH)
        .expect("an empty reach must be refused");
    assert!(reported.message.contains("empty reach"), "{reported:?}");
}

#[test]
fn an_effect_with_no_value_vocabulary_takes_no_reach() {
    let analysis = check(
        r#"
tool vault.read(name: string) -> string !secret_access("PROD_TOKEN")

agent A(topic: string) -> report<markdown> {
  tools { mcp vault.read }
  policy { secret_access allow }
  flow {
    value = call vault.read(topic)
    emit report = ask<markdown>("done", context: value)
  }
}
"#,
    );
    let reported = analysis
        .diagnostics
        .iter()
        .find(|d| d.code == codes::INVALID_EFFECT_REACH)
        .expect("secret_access names no resource");
    assert!(reported.message.contains("no resource"), "{reported:?}");
}

#[test]
fn a_reach_path_may_not_leave_the_workspace() {
    // A path that means something different on two machines would make
    // containment a check against a moving target.
    for escape in ["/etc", "../secrets", "src/../.."] {
        let analysis = reaching(
            &format!("!filesystem_read({escape:?})"),
            r#"filesystem_read allow ["src"]"#,
        );
        assert!(
            codes_of(&analysis).contains(&codes::INVALID_EFFECT_REACH),
            "`{escape}` must be refused: {:?}",
            codes_of(&analysis)
        );
    }
}

#[test]
fn a_host_reach_is_a_host_rather_than_a_url_or_a_pattern() {
    for bad in ["*.arxiv.org", "https://arxiv.org/abs"] {
        let analysis = reaching(
            &format!("!network({bad:?})"),
            r#"network allow ["arxiv.org"]"#,
        );
        assert!(
            codes_of(&analysis).contains(&codes::INVALID_EFFECT_REACH),
            "`{bad}` must be refused: {:?}",
            codes_of(&analysis)
        );
    }
}

#[test]
fn a_reach_is_sorted_and_deduplicated_at_the_declaration() {
    // So the canonical IR encoding is a property of the document rather than
    // of whoever wrote the declaration.
    let analysis = reaching(
        r#"!network("github.com", "arxiv.org", "github.com")"#,
        r#"network allow ["arxiv.org", "github.com"]"#,
    );
    let tool = analysis.tools.get("feed.fetch").expect("the tool");
    let values: Vec<&str> = tool.reach.iter().map(|r| r.value.as_str()).collect();
    assert_eq!(values, vec!["arxiv.org", "github.com"]);
}

// --- persistent memory ------------------------------------------------------
//
// RFC-0018. Two stores, told apart at every use site, and a persistent field
// that always has a value.

/// The same as [`check`], with a language 0.2 header.
///
/// `persistent` is gated in the parser, so a 0.1 header would fail the
/// parses-cleanly assertion before the checker saw anything.
fn check_v2(body: &str) -> Analysis {
    let source = format!(
        "{}\n{body}",
        PRELUDE.replacen("language 0.1", "language 0.2", 1)
    );
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

const REMEMBERS: &str = r#"
agent Remembers(note: text) -> log<text> {
  model requires { tool_calling }

  memory {
    working ephemeral { scratch: text }
    persistent { seen: text[] = [], depth: int = 0 }
  }

  budget { steps <= 3 }
  policy { network deny }

  flow {
    state.scratch = note
    memory.seen = [state.scratch]
    memory.depth = memory.depth + 1
    emit log = state.scratch
  }
}
"#;

#[test]
fn the_two_stores_are_separate_and_both_resolve() {
    let analysis = check_v2(REMEMBERS);
    assert_clean(&analysis);
    let agent = analysis
        .agents
        .iter()
        .find(|agent| agent.name == "Remembers")
        .expect("the agent is checked");
    assert_eq!(
        agent
            .state
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>(),
        vec!["scratch"]
    );
    assert_eq!(
        agent
            .persistent
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>(),
        vec!["seen", "depth"]
    );
}

#[test]
fn a_persistent_field_without_an_initial_value_is_refused() {
    let analysis = check_v2(
        r#"
agent Forgets() -> log<json> {
  memory { persistent { seen: string[] } }
  flow { emit log = memory.seen }
}
"#,
    );
    assert!(
        codes_of(&analysis).contains(&codes::MISSING_INITIAL_VALUE),
        "{:?}",
        codes_of(&analysis)
    );
}

#[test]
fn an_initial_value_that_is_not_a_literal_is_refused() {
    // Nothing is bound when an initial value is resolved, so an expression here
    // would have nothing to read even if it looked reasonable.
    let analysis = check_v2(
        r#"
agent Forgets(seed: int) -> log<json> {
  memory { persistent { depth: int = seed } }
  flow { emit log = memory.depth }
}
"#,
    );
    assert!(
        codes_of(&analysis).contains(&codes::INITIAL_VALUE_NOT_LITERAL),
        "{:?}",
        codes_of(&analysis)
    );
}

#[test]
fn an_initial_value_is_checked_against_the_declared_type() {
    let analysis = check_v2(
        r#"
agent Forgets() -> log<json> {
  memory { persistent { depth: int = "none" } }
  flow { emit log = memory.depth }
}
"#,
    );
    assert!(
        codes_of(&analysis).contains(&codes::TYPE_MISMATCH),
        "{:?}",
        codes_of(&analysis)
    );
}

#[test]
fn an_empty_list_starts_a_persistent_list_of_any_element_type() {
    // `= []` is how most persistent collections start, so `list<unknown>` has
    // to be assignable to the declared list rather than a mismatch.
    let analysis = check_v2(
        r#"
agent Collects(note: text) -> log<text> {
  memory { persistent { seen: search_result[] = [] } }
  flow {
    memory.seen = memory.seen
    emit log = note
  }
}
"#,
    );
    assert_clean(&analysis);
}

#[test]
fn naming_the_wrong_store_points_at_the_other_one() {
    // The likeliest mistake: the two roots look alike and hold the same kind of
    // thing, so a field that exists on the other side is worth more than a
    // spelling suggestion.
    let analysis = check_v2(
        r#"
agent Confused() -> log<json> {
  memory {
    working ephemeral { scratch: string }
    persistent { seen: string[] = [] }
  }
  flow {
    state.scratch = "x"
    emit log = memory.scratch
  }
}
"#,
    );
    let help: Vec<String> = analysis
        .diagnostics
        .iter()
        .filter_map(|d| d.help.clone())
        .collect();
    assert!(
        help.iter().any(|text| text.contains("state.scratch")),
        "expected a pointer at the other store, got {help:?}"
    );
}

#[test]
fn a_persistent_write_inside_parallel_map_is_refused() {
    // Same rule as an ephemeral write, and for the same reason: iterations run
    // concurrently, so a write one of them can observe is not well defined.
    let analysis = check_v2(
        r#"
agent Fans(topics: string[]) -> log<json> {
  memory { persistent { seen: string[] = [] } }
  flow {
    all = parallel map topics as topic {
      memory.seen = [topic]
    }
    emit log = memory.seen
  }
}
"#,
    );
    assert!(
        codes_of(&analysis).contains(&codes::INVALID_IN_PARALLEL),
        "{:?}",
        codes_of(&analysis)
    );
}

#[test]
fn the_last_expression_of_a_map_body_is_not_a_discarded_value() {
    // It is the iteration's value — the most used expression in the agent. The
    // warning fired on it because only a `call` was exempt, and the examples all
    // used one. Found writing a conformance case for `parallel map`.
    let analysis = check(
        r#"
agent Fanned(topics: string[]) -> digest<markdown> {
  flow {
    written = parallel map topics as topic {
      ask<markdown>("Write about ${topic}.")
    }
    emit digest = ask<markdown>("Join these.", context: written)
  }
}
"#,
    );
    let discarded: Vec<&str> = analysis
        .diagnostics
        .iter()
        .filter(|d| d.message.contains("discarded"))
        .map(|d| d.message.as_str())
        .collect();
    assert!(discarded.is_empty(), "{discarded:?}");
}

#[test]
fn an_expression_that_really_is_discarded_still_warns() {
    let analysis = check(
        r#"
agent Wasteful(topic: string) -> digest<markdown> {
  flow {
    ask<markdown>("Write about ${topic}.")
    emit digest = ask<markdown>("Write again.")
  }
}
"#,
    );
    assert!(
        analysis
            .diagnostics
            .iter()
            .any(|d| d.message.contains("discarded")),
        "the warning still has to fire where the value goes nowhere"
    );
}

// --- a failure an iteration can absorb (RFC-0022) -------------------------

/// The prelude, at the version `else` needs.
///
/// A separate constant rather than moving `PRELUDE`: every other test in this
/// file asserts something about language 0.1, and a version bump that quietly
/// re-versioned all of them would stop them testing what they were written for.
const PRELUDE_V4: &str = r#"language 0.4
package heptapus.test

type search_result {
  title: string
  url: string
}

type rating {
  score: int
  note: string
}

tool web.search(query: string) -> search_result[] !network
tool fs.read_file(path: string) -> string !filesystem_read
tool fs.read_text(path: string) -> text !filesystem_read

verifier CitationCheck(draft: markdown, min_sources: int)
"#;

/// Check a 0.4 program, allowing parse errors through so a version gate or a
/// syntax rule can be asserted on rather than tripping the harness.
fn check_v4_raw(body: &str) -> (Vec<String>, Option<Analysis>) {
    let source = format!("{PRELUDE_V4}\n{body}");
    let mut map = SourceMap::new();
    let file = map.add_virtual("test.ing", source);
    let parsed = ingot_parser::parse(map.file(file));
    let parse_codes: Vec<String> = parsed
        .diagnostics
        .iter()
        .map(|d| d.code.to_string())
        .collect();
    if parsed.diagnostics.has_errors() {
        return (parse_codes, None);
    }
    (parse_codes, Some(analyze(&parsed.program)))
}

fn check_v4(body: &str) -> Analysis {
    let (parse_codes, analysis) = check_v4_raw(body);
    analysis.unwrap_or_else(|| panic!("test source must parse cleanly: {parse_codes:?}"))
}

/// An agent whose flow is `body`, with everything a fallback needs granted.
fn absorbing(body: &str) -> String {
    format!(
        r#"
agent Digester(items: string[]) -> digest<markdown> {{
  model requires {{ structured_output }}

  tools {{
    mcp fs.read_file
    mcp fs.read_text
    mcp web.search
  }}

  budget {{ steps <= 40 }}

  policy {{
    filesystem_read allow
    network allow
  }}

  flow {{
{body}
  }}
}}
"#
    )
}

#[test]
fn a_fallback_over_a_pure_expression_is_accepted() {
    let analysis = check_v4(&absorbing(
        r#"    entries = parallel map items as item {
      writeup = call fs.read_file(item) else "no write-up was filed"
      ask<markdown>("Summarise this.", context: writeup)
    }
    emit digest = ask<markdown>("Join these.", context: entries)"#,
    ));
    assert_clean(&analysis);
}

#[test]
fn a_fallback_that_reaches_anything_does_not_compile() {
    // The restriction is the whole design: a fallback that reached something
    // would spend a second step and would make what the agent may reach the
    // union over two paths rather than the one sequence an operator reads.
    for reaching in [
        r#"call fs.read_file(item) else call web.search(item)"#,
        r#"call fs.read_file(item) else ask<string>("Make something up.")"#,
        r#"ask<string>("Summarise.") else consult("What should this say?")"#,
        r#"ask<string>("Summarise.") else parallel map items as x { ask<string>("hm") }"#,
    ] {
        let analysis = check_v4(&absorbing(&format!(
            "    entries = parallel map items as item {{\n      value = {reaching}\n      value\n    }}\n    emit digest = ask<markdown>(\"Join.\", context: entries)"
        )));
        assert!(
            codes_of(&analysis).contains(&codes::FALLBACK_NOT_PURE),
            "expected ING6008 for `{reaching}`, got {:?}",
            codes_of(&analysis)
        );
    }
}

#[test]
fn a_pure_fallback_may_read_something_already_bound() {
    // Pure does not mean literal. What it means is *reaches nothing*.
    let analysis = check_v4(&absorbing(
        r#"    placeholder = "nothing was filed"
    entries = parallel map items as item {
      call fs.read_file(item) else placeholder
    }
    emit digest = ask<markdown>("Join these.", context: entries)"#,
    ));
    assert_clean(&analysis);
}

#[test]
fn else_on_something_that_cannot_fail_does_not_compile() {
    let analysis = check_v4(&absorbing(
        r#"    count = len(items) else 0
    emit digest = ask<markdown>("Report ${count}.")"#,
    ));
    assert!(
        codes_of(&analysis).contains(&codes::ELSE_NOT_APPLICABLE),
        "expected ING6009, got {:?}",
        codes_of(&analysis)
    );
}

#[test]
fn else_on_a_consult_does_not_compile() {
    // A consultation *can* fail, and it is still refused: a person was asked and
    // did not answer, so continuing with a default is continuing without them.
    let analysis = check_v4(&absorbing(
        r#"    framing = consult("Which framing?") else "executive"
    emit digest = ask<markdown>("Write it ${framing}.")"#,
    ));
    assert!(
        codes_of(&analysis).contains(&codes::ELSE_NOT_APPLICABLE),
        "expected ING6009, got {:?}",
        codes_of(&analysis)
    );
}

#[test]
fn else_on_a_whole_fan_out_does_not_compile() {
    // A fallback for a fan-out is a handler around a block, which is the larger
    // feature RFC-0022 deliberately left out. The `else` belongs on the
    // statement inside the body, where the other elements' work survives.
    let analysis = check_v4(&absorbing(
        r#"    entries = parallel map items as item {
      call fs.read_file(item)
    } else []
    emit digest = ask<markdown>("Join.", context: entries)"#,
    ));
    assert!(
        codes_of(&analysis).contains(&codes::ELSE_NOT_APPLICABLE),
        "expected ING6009, got {:?}",
        codes_of(&analysis)
    );
}

#[test]
fn both_sides_of_an_else_must_share_a_type() {
    // Whatever reads the binding must not have to ask which path ran.
    let analysis = check_v4(&absorbing(
        r#"    entries = parallel map items as item {
      call fs.read_file(item) else 0
    }
    emit digest = ask<markdown>("Join.", context: entries)"#,
    ));
    assert!(
        codes_of(&analysis).contains(&codes::TYPE_MISMATCH),
        "expected ING3001, got {:?}",
        codes_of(&analysis)
    );
}

#[test]
fn a_fallback_reaches_exactly_the_types_the_language_can_write() {
    // What `else` covers is decided by which types have a literal, and that is
    // narrower than RFC-0022 assumed. `string`, `int`, `float`, `bool` and lists
    // of those can be written; `markdown`, `text`, `json` and a declared record
    // cannot, because the language has no literal for one and `string` is not
    // assignable to any of them.
    //
    // So `else` covers the case both real programs needed -- a tool returning
    // `string` -- and not the second example the RFC wrote. See GAP-045.
    let writable = check_v4(&absorbing(
        r#"    scores = parallel map items as item {
      ask<int>("Score this out of ten.", context: item) else 0
    }
    emit digest = ask<markdown>("Report these.", context: scores)"#,
    ));
    assert_clean(&writable);

    // A prose answer has no literal to fall back to, so the type check refuses
    // every fallback an author can currently write for one.
    let prose = check_v4(&absorbing(
        r#"    entries = parallel map items as item {
      ask<markdown>("Summarise this.", context: item) else "nothing to say"
    }
    emit digest = ask<markdown>("Join.", context: entries)"#,
    ));
    assert!(
        codes_of(&prose).contains(&codes::TYPE_MISMATCH),
        "a string literal is not `markdown`, and this is the wall GAP-045 records: {:?}",
        codes_of(&prose)
    );
    assert!(
        !codes_of(&prose).contains(&codes::ELSE_NOT_APPLICABLE),
        "the `else` itself is fine -- it is the fallback's type that is not"
    );

    // Nor does a record, for the same reason: there is no record literal.
    let (parse_codes, _) = check_v4_raw(&absorbing(
        r#"    entries = parallel map items as item {
      ask<rating>("Rate this.", context: item) else rating { score: 0, note: "unrated" }
    }
    emit digest = ask<markdown>("Join.", context: entries)"#,
    ));
    assert!(
        !parse_codes.is_empty(),
        "the RFC's second example does not parse, because record construction is          not in the language: {parse_codes:?}"
    );
}

#[test]
fn a_fallback_does_not_move_the_static_step_bound() {
    // The attempt is the step and the fallback is an expression, so a `steps`
    // budget that fits without the `else` still fits with it. If a fallback ever
    // counted, this is the test that would fail rather than a bound quietly
    // doubling.
    let with = check_v4(
        r#"
agent Tight(items: string[]) -> digest<markdown> {
  model requires { structured_output }
  tools { mcp fs.read_file }
  budget { steps <= 2 }
  policy {
    filesystem_read allow
  }
  flow {
    writeup = call fs.read_file("a.md") else "nothing"
    emit digest = ask<markdown>("Summarise.", context: writeup)
  }
}
"#,
    );
    assert_clean(&with);
    assert!(
        !codes_of(&with).contains(&codes::STATIC_STEPS_EXCEED_BUDGET),
        "two calls under a bound of two: {:?}",
        codes_of(&with)
    );
}

#[test]
fn a_fallback_requires_language_0_4() {
    // Before 0.4 every failure ends the run, which is the right behaviour for
    // work whose job is to produce something correct or nothing.
    let source = r#"language 0.3
package heptapus.test

tool fs.read_file(path: string) -> string !filesystem_read

agent Old() -> digest<markdown> {
  model requires { structured_output }
  tools { mcp fs.read_file }
  budget { steps <= 4 }
  policy {
    model_access allow
    filesystem_read allow
  }
  flow {
    writeup = call fs.read_file("a.md") else "nothing"
    emit digest = ask<markdown>("Summarise.", context: writeup)
  }
}
"#;
    let mut map = SourceMap::new();
    let file = map.add_virtual("old.ing", source.to_string());
    let parsed = ingot_parser::parse(map.file(file));
    let found: Vec<&str> = parsed.diagnostics.iter().map(|d| d.code).collect();
    assert!(
        found.contains(&codes::UNSUPPORTED_LANGUAGE_VERSION),
        "expected the version gate to fire, got {found:?}"
    );
}

#[test]
fn else_does_not_chain() {
    // A fallback is pure, so it cannot fail and cannot need a fallback of its
    // own. Refused rather than silently grouped.
    let (parse_codes, _) = check_v4_raw(&absorbing(
        r#"    writeup = call fs.read_file("a.md") else "one" else "two"
    emit digest = ask<markdown>("Summarise.", context: writeup)"#,
    ));
    assert!(
        parse_codes
            .iter()
            .any(|code| code == codes::UNEXPECTED_TOKEN),
        "expected a syntax error for a chained `else`, got {parse_codes:?}"
    );
}

#[test]
fn else_is_allowed_on_a_sub_agent_call_and_has_no_writable_fallback() {
    // `else` may follow a sub-agent call -- it is one of the three attempts that
    // can fail. But an agent's output is always an artifact content type
    // (`text`, `markdown`, `json`, `file`), and the language has a literal for
    // none of them, so there is currently nothing an author can put after the
    // `else`. The permission is real and the coverage is not; GAP-045 is that
    // gap, and this test is what will start passing when it closes.
    let analysis = check_v4(
        r##"
agent Child(topic: string) -> note<markdown> {
  model requires { structured_output }
  budget { steps <= 2 }
  flow {
    emit note = ask<markdown>("Write about ${topic}.")
  }
}

agent Parent(topic: string) -> digest<markdown> {
  model requires { structured_output }
  budget { steps <= 4 }
  flow {
    child = call Child(topic) else "# nothing to report"
    emit digest = ask<markdown>("Wrap it.", context: child)
  }
}
"##,
    );
    let found = codes_of(&analysis);
    assert!(
        !found.contains(&codes::ELSE_NOT_APPLICABLE),
        "a sub-agent call may carry an `else`: {found:?}"
    );
    assert!(
        found.contains(&codes::TYPE_MISMATCH),
        "and the only fallback that can be written for one is the wrong type: {found:?}"
    );
}

#[test]
fn the_motivating_example_compiles_against_a_text_returning_tool() {
    // The line RFC-0022 was written to make possible, against the signature the
    // shipped filesystem server actually declares: `read_file` returns `text`,
    // not `string`. Without `string` -> `text` in the assignability table this is
    // `ING3001`, and the only workaround is to declare the tool `-> string` so
    // that a *flow* can have a fallback -- which gets a declaration backwards.
    let analysis = check_v4(&absorbing(
        r#"    entries = parallel map items as item {
      writeup = call fs.read_text("${item}.md") else "no write-up was filed"
      ask<string>("Summarise this incident.", context: writeup)
    }
    emit digest = ask<markdown>("Collect these.", context: entries)"#,
    ));
    assert_clean(&analysis);
}

#[test]
fn a_string_fallback_does_not_reach_a_markdown_attempt() {
    // The other side of the same line. `markdown` is the more specific type, and
    // a bare string does not get to claim it -- see GAP-045 for why admitting it
    // would collapse `markdown` and `text` into one type with two names.
    let analysis = check_v4(&absorbing(
        r#"    entries = parallel map items as item {
      ask<markdown>("Summarise this.", context: item) else "nothing to say"
    }
    emit digest = ask<markdown>("Join.", context: entries)"#,
    ));
    assert!(
        codes_of(&analysis).contains(&codes::TYPE_MISMATCH),
        "expected ING3001, got {:?}",
        codes_of(&analysis)
    );
}
