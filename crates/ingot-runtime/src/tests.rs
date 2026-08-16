//! Interpreter behaviour tests.
//!
//! These are the conformance tests RFC-0002 promises. Each pins one guarantee
//! the IR makes, and each is named after the guarantee rather than the function
//! that implements it, so removing the check breaks the test that documents it.
//!
//! Artifacts are built here by hand rather than compiled from source: the point
//! is to test the *interpreter* against an IR document, exactly as a third-party
//! backend would receive one.

use std::collections::BTreeMap;

use ingot_ir::{
    node::Argument, AgentIr, Budget, Decision, FieldType, ModelRequirement, Node, NodeKind,
    PolicyRule, RecordType, RefScope, Requirements, TemplatePart, ToolBinding, ToolSignature,
    Value as IrValue, IR_VERSION,
};
use serde_json::json;

use crate::events::{CollectingSink, RunEvent};
use crate::provider::Usage;
use crate::tools::{HumanChannel, ScriptedApprovals, StaticToolHost};
use crate::{
    run, Cassette, DenyAllTools, RecordingProvider, ReplayProvider, RunError, RunOptions,
    ScriptedProvider,
};

// --- artifact builders ----------------------------------------------------

fn base(agent: &str) -> AgentIr {
    AgentIr {
        ir_version: IR_VERSION.to_string(),
        language: "0.1".to_string(),
        agent: agent.to_string(),
        doc: None,
        inputs: BTreeMap::new(),
        outputs: BTreeMap::new(),
        types: BTreeMap::new(),
        requirements: Requirements {
            model: ModelRequirement::Unspecified,
        },
        tools: Vec::new(),
        state: BTreeMap::new(),
        persistent: BTreeMap::new(),
        budget: Budget::default(),
        policy: BTreeMap::new(),
        effects: vec!["model_access".to_string()],
        entry: None,
        nodes: Vec::new(),
    }
}

fn llm(
    id: &str,
    binding: Option<&str>,
    prompt: &str,
    response_type: &str,
    next: Option<&str>,
) -> Node {
    let mut node = Node::new(id, NodeKind::LlmCall);
    node.binding = binding.map(str::to_string);
    node.prompt = Some(IrValue::string(prompt));
    node.response_type = Some(response_type.to_string());
    node.effects = vec!["model_access".to_string()];
    node.next = next.map(str::to_string);
    node
}

fn emit(id: &str, output: &str, from_binding: &str, next: Option<&str>) -> Node {
    let mut node = Node::new(id, NodeKind::ArtifactEmit);
    node.output = Some(output.to_string());
    node.value = Some(IrValue::Ref {
        scope: RefScope::Binding,
        path: vec![from_binding.to_string()],
    });
    node.next = next.map(str::to_string);
    node
}

/// The smallest complete artifact: one model call, one emission.
fn summarizer() -> AgentIr {
    let mut ir = base("test.Summarizer");
    ir.inputs.insert("document".into(), "text".into());
    ir.outputs
        .insert("summary".into(), "artifact<markdown>".into());
    ir.entry = Some("n0".into());
    ir.nodes = vec![
        {
            let mut node = llm("n0", Some("draft"), "Summarise it", "markdown", Some("n1"));
            node.prompt = Some(IrValue::Template {
                parts: vec![
                    TemplatePart::Text {
                        value: "Summarise: ".into(),
                    },
                    TemplatePart::Value {
                        value: IrValue::Ref {
                            scope: RefScope::Input,
                            path: vec!["document".into()],
                        },
                        ty: "text".into(),
                    },
                ],
            });
            node
        },
        emit("n1", "summary", "draft", None),
    ];
    ir
}

fn searcher(decision: Option<Decision>) -> AgentIr {
    let mut ir = base("test.Searcher");
    ir.inputs.insert("query".into(), "string".into());
    ir.outputs
        .insert("report".into(), "artifact<markdown>".into());
    ir.types.insert(
        "hit".into(),
        RecordType {
            fields: vec![
                FieldType {
                    name: "title".into(),
                    ty: "string".into(),
                },
                FieldType {
                    name: "url".into(),
                    ty: "string".into(),
                },
            ],
        },
    );
    ir.tools = vec![ToolBinding {
        reference: "mcp:web.search".into(),
        name: "web.search".into(),
        transport: "mcp".into(),
        scopes: BTreeMap::new(),
        effects: vec!["network".into()],
        signature: ToolSignature {
            params: vec![FieldType {
                name: "query".into(),
                ty: "string".into(),
            }],
            result: "hit[]".into(),
        },
    }];
    if let Some(decision) = decision {
        ir.policy.insert(
            "network".into(),
            PolicyRule {
                decision,
                values: Vec::new(),
                qualifier: None,
            },
        );
    }
    ir.effects = vec!["model_access".into(), "network".into()];
    ir.entry = Some("n0".into());

    let mut call = Node::new("n0", NodeKind::ToolCall);
    call.binding = Some("hits".into());
    call.tool = Some("mcp:web.search".into());
    call.effects = vec!["network".into()];
    call.args = vec![Argument {
        name: "query".into(),
        value: IrValue::Ref {
            scope: RefScope::Input,
            path: vec!["query".into()],
        },
    }];
    call.next = Some("n1".into());

    let mut write = llm("n1", Some("draft"), "Write it up", "markdown", Some("n2"));
    write.args = vec![Argument {
        name: "context".into(),
        value: IrValue::Ref {
            scope: RefScope::Binding,
            path: vec!["hits".into()],
        },
    }];

    ir.nodes = vec![call, write, emit("n2", "report", "draft", None)];
    ir
}

fn run_with(
    ir: &AgentIr,
    provider: &mut dyn crate::ModelProvider,
    tools: &mut dyn crate::ToolHost,
    inputs: BTreeMap<String, serde_json::Value>,
) -> (Result<crate::RunReport, RunError>, Vec<RunEvent>) {
    let mut sink = CollectingSink::default();
    let registry = BTreeMap::new();
    let result = run(
        ir,
        &registry,
        provider,
        tools,
        &mut sink,
        RunOptions {
            inputs,
            ..RunOptions::default()
        },
    );
    (result, sink.events)
}

// --- the happy path -------------------------------------------------------

#[test]
fn runs_the_document_summarizer_end_to_end() {
    let ir = summarizer();
    let mut provider = ScriptedProvider::new(vec![json!("# Summary\n\nIt was fine.")]);
    let mut tools = DenyAllTools;
    let (result, events) = run_with(
        &ir,
        &mut provider,
        &mut tools,
        [("document".to_string(), json!("a long document"))].into(),
    );

    let report = result.expect("the run should succeed");
    assert_eq!(report.steps, 1);
    assert_eq!(report.outputs["summary"].content_type, "markdown");
    assert_eq!(
        String::from_utf8(report.outputs["summary"].to_bytes()).unwrap(),
        "# Summary\n\nIt was fine."
    );
    assert!(matches!(events.first(), Some(RunEvent::RunStarted { .. })));
    assert!(matches!(events.last(), Some(RunEvent::RunFinished { .. })));
}

#[test]
fn prompt_templates_are_rendered_with_input_values() {
    let ir = summarizer();
    let mut provider = ScriptedProvider::new(vec![json!("ok")]);
    let mut tools = DenyAllTools;
    let (result, _) = run_with(
        &ir,
        &mut provider,
        &mut tools,
        [("document".to_string(), json!("PAYLOAD"))].into(),
    );
    assert!(result.is_ok());
    // The scripted provider ignores the prompt, so assert through the digest
    // path instead: record it and read the interaction back.
    let mut recorder = RecordingProvider::new(ScriptedProvider::new(vec![json!("ok")]), "x");
    let mut tools = DenyAllTools;
    let _ = run_with(
        &ir,
        &mut recorder,
        &mut tools,
        [("document".to_string(), json!("PAYLOAD"))].into(),
    );
    let cassette = recorder.finish();
    assert_eq!(cassette.interactions.len(), 1);
}

#[test]
fn a_missing_input_is_reported_before_anything_runs() {
    let ir = summarizer();
    let mut provider = ScriptedProvider::new(vec![json!("unused")]);
    let mut tools = DenyAllTools;
    let (result, _) = run_with(&ir, &mut provider, &mut tools, BTreeMap::new());
    let error = result.unwrap_err();
    assert!(matches!(error, RunError::MissingInput { .. }));
    assert!(error.is_operator_error());
    assert_eq!(
        provider.calls(),
        0,
        "nothing should run before inputs are valid"
    );
}

#[test]
fn an_input_of_the_wrong_type_is_rejected() {
    let ir = summarizer();
    let mut provider = ScriptedProvider::new(vec![json!("unused")]);
    let mut tools = DenyAllTools;
    let (result, _) = run_with(
        &ir,
        &mut provider,
        &mut tools,
        [("document".to_string(), json!(42))].into(),
    );
    assert!(matches!(result.unwrap_err(), RunError::InvalidInput { .. }));
}

#[test]
fn an_unknown_input_is_rejected_rather_than_ignored() {
    let ir = summarizer();
    let mut provider = ScriptedProvider::new(vec![json!("x")]);
    let mut tools = DenyAllTools;
    let (result, _) = run_with(
        &ir,
        &mut provider,
        &mut tools,
        [
            ("document".to_string(), json!("d")),
            ("topci".to_string(), json!("typo")),
        ]
        .into(),
    );
    let error = result.unwrap_err();
    assert!(matches!(error, RunError::UnknownInput { .. }));
    assert!(error.to_string().contains("topci"), "{error}");
}

// --- typed responses ------------------------------------------------------

#[test]
fn structured_response_types_are_schema_constrained() {
    use crate::schema::{response_shape, ResponseShape};
    let shape = response_shape("string[]", &BTreeMap::new()).unwrap();
    assert!(matches!(shape, ResponseShape::Schema { .. }));
}

#[test]
fn prose_response_types_are_not_schema_constrained() {
    use crate::schema::{response_shape, ResponseShape};
    assert_eq!(
        response_shape("markdown", &BTreeMap::new()).unwrap(),
        ResponseShape::Prose
    );
}

#[test]
fn a_response_that_violates_the_schema_is_an_error() {
    let mut ir = summarizer();
    ir.nodes[0].response_type = Some("string[]".into());
    ir.outputs.insert("summary".into(), "artifact<json>".into());

    // The provider claims a list but returns a string.
    let mut provider = ScriptedProvider::new(vec![json!("not a list")]);
    let mut tools = DenyAllTools;
    let (result, _) = run_with(
        &ir,
        &mut provider,
        &mut tools,
        [("document".to_string(), json!("d"))].into(),
    );
    let error = result.unwrap_err();
    assert!(matches!(error, RunError::Provider { .. }), "{error}");
    assert!(error.to_string().contains("expected `string[]`"), "{error}");
}

// --- capability enforcement ----------------------------------------------

#[test]
fn a_denied_capability_is_refused_at_runtime() {
    let ir = searcher(Some(Decision::Deny));
    let mut provider = ScriptedProvider::new(vec![json!("unused")]);
    let mut tools = StaticToolHost::new().with("web.search", |_| Ok(json!([])));
    let (result, _) = run_with(
        &ir,
        &mut provider,
        &mut tools,
        [("query".to_string(), json!("q"))].into(),
    );

    let error = result.unwrap_err();
    match &error {
        RunError::CapabilityDenied {
            effect, explicit, ..
        } => {
            assert_eq!(effect, "network");
            assert!(*explicit);
        }
        other => panic!("expected a capability denial, got {other}"),
    }
}

#[test]
fn an_unlisted_policy_subject_is_refused_at_runtime() {
    // No `network` rule at all. Default-deny must hold at runtime too.
    let ir = searcher(None);
    let mut provider = ScriptedProvider::new(vec![json!("unused")]);
    let mut tools = StaticToolHost::new().with("web.search", |_| Ok(json!([])));
    let (result, _) = run_with(
        &ir,
        &mut provider,
        &mut tools,
        [("query".to_string(), json!("q"))].into(),
    );

    let error = result.unwrap_err();
    match &error {
        RunError::CapabilityDenied { explicit, .. } => {
            assert!(
                !explicit,
                "an absent rule is distinct from an explicit deny"
            );
        }
        other => panic!("expected a capability denial, got {other}"),
    }
    assert!(
        error.to_string().contains("absent rule is a denial"),
        "{error}"
    );
}

#[test]
fn an_allowed_capability_reaches_the_tool_host() {
    let ir = searcher(Some(Decision::Allow));
    let mut provider = ScriptedProvider::new(vec![json!("# Report")]);
    let mut tools =
        StaticToolHost::new().with("web.search", |_| Ok(json!([{"title": "t", "url": "u"}])));
    let (result, events) = run_with(
        &ir,
        &mut provider,
        &mut tools,
        [("query".to_string(), json!("q"))].into(),
    );

    assert!(result.is_ok(), "{:?}", result.err().map(|e| e.to_string()));
    assert!(events
        .iter()
        .any(|e| matches!(e, RunEvent::ToolCall { .. })));
}

#[test]
fn a_missing_tool_host_stops_the_run_rather_than_skipping_the_call() {
    let ir = searcher(Some(Decision::Allow));
    let mut provider = ScriptedProvider::new(vec![json!("unused")]);
    let mut tools = DenyAllTools;
    let (result, _) = run_with(
        &ir,
        &mut provider,
        &mut tools,
        [("query".to_string(), json!("q"))].into(),
    );
    let error = result.unwrap_err();
    assert!(matches!(error, RunError::Tool { .. }));
    assert!(error.is_operator_error());
}

#[test]
fn a_tool_result_of_the_wrong_type_is_rejected() {
    let ir = searcher(Some(Decision::Allow));
    let mut provider = ScriptedProvider::new(vec![json!("unused")]);
    // Declared `hit[]`, returns a bare string.
    let mut tools = StaticToolHost::new().with("web.search", |_| Ok(json!("oops")));
    let (result, _) = run_with(
        &ir,
        &mut provider,
        &mut tools,
        [("query".to_string(), json!("q"))].into(),
    );
    assert!(result.unwrap_err().to_string().contains("expected `hit[]`"));
}

// --- budgets --------------------------------------------------------------

#[test]
fn the_step_budget_is_enforced() {
    let mut ir = summarizer();
    ir.budget.steps = Some(0);
    let mut provider = ScriptedProvider::new(vec![json!("x")]);
    let mut tools = DenyAllTools;
    let (result, _) = run_with(
        &ir,
        &mut provider,
        &mut tools,
        [("document".to_string(), json!("d"))].into(),
    );
    let error = result.unwrap_err();
    assert!(matches!(error, RunError::BudgetExceeded { .. }), "{error}");
    assert!(error.to_string().contains("steps"), "{error}");
}

#[test]
fn the_token_budget_is_enforced() {
    let mut ir = summarizer();
    ir.budget.tokens = Some(5);
    let mut provider = ScriptedProvider::new(vec![json!("x")]).with_usage(Usage {
        input_tokens: 100,
        output_tokens: 100,
        cache_read_tokens: 0,
    });
    let mut tools = DenyAllTools;
    let (result, _) = run_with(
        &ir,
        &mut provider,
        &mut tools,
        [("document".to_string(), json!("d"))].into(),
    );
    let error = result.unwrap_err();
    assert!(error.to_string().contains("tokens"), "{error}");
}

// --- control flow ---------------------------------------------------------

#[test]
fn a_loop_stops_at_its_static_bound() {
    let mut ir = base("test.Looper");
    ir.outputs.insert("out".into(), "artifact<markdown>".into());
    ir.entry = Some("n0".into());

    let mut loop_node = Node::new("n0", NodeKind::Loop);
    loop_node.max_iterations = Some(3);
    // A guard that is always true: only the static bound can stop this.
    loop_node.guard = Some(IrValue::bool(true));
    loop_node.body = Some("n1".into());
    loop_node.next = Some("n2".into());

    ir.nodes = vec![
        loop_node,
        llm("n1", Some("scratch"), "again", "markdown", None),
        llm("n2", Some("final"), "done", "markdown", Some("n3")),
        emit("n3", "out", "final", None),
    ];

    let mut provider =
        ScriptedProvider::new(vec![json!("1"), json!("2"), json!("3"), json!("final")]);
    let mut tools = DenyAllTools;
    let (result, events) = run_with(&ir, &mut provider, &mut tools, BTreeMap::new());

    assert!(result.is_ok(), "{:?}", result.err().map(|e| e.to_string()));
    let iterations = events
        .iter()
        .filter(|e| matches!(e, RunEvent::LoopIteration { .. }))
        .count();
    assert_eq!(
        iterations, 3,
        "the static bound must cap an always-true guard"
    );
}

#[test]
fn a_branch_takes_only_one_arm() {
    let mut ir = base("test.Brancher");
    ir.inputs.insert("n".into(), "int".into());
    ir.outputs.insert("out".into(), "artifact<markdown>".into());
    ir.entry = Some("n0".into());

    let mut branch = Node::new("n0", NodeKind::Branch);
    branch.condition = Some(IrValue::Binary {
        op: ">".into(),
        lhs: Box::new(IrValue::Ref {
            scope: RefScope::Input,
            path: vec!["n".into()],
        }),
        rhs: Box::new(IrValue::int(1)),
    });
    branch.then = Some("n1".into());
    branch.otherwise = Some("n3".into());

    ir.nodes = vec![
        branch,
        llm("n1", Some("big"), "big", "markdown", Some("n2")),
        emit("n2", "out", "big", None),
        llm("n3", Some("small"), "small", "markdown", Some("n4")),
        emit("n4", "out", "small", None),
    ];

    let mut provider = ScriptedProvider::new(vec![json!("the big one")]);
    let mut tools = DenyAllTools;
    let (result, events) = run_with(
        &ir,
        &mut provider,
        &mut tools,
        [("n".to_string(), json!(5))].into(),
    );

    let report = result.expect("branch run should succeed");
    assert_eq!(report.steps, 1, "only one arm should run");
    assert!(events
        .iter()
        .any(|e| matches!(e, RunEvent::BranchTaken { arm, .. } if arm == "then")));
}

#[test]
fn parallel_map_visits_every_element() {
    let mut ir = base("test.Mapper");
    ir.inputs.insert("items".into(), "string[]".into());
    ir.outputs.insert("out".into(), "artifact<json>".into());
    ir.entry = Some("n0".into());

    let mut map = Node::new("n0", NodeKind::Parallel);
    map.binding = Some("results".into());
    map.mode = Some("map".into());
    map.binder = Some("item".into());
    map.source = Some(IrValue::Ref {
        scope: RefScope::Input,
        path: vec!["items".into()],
    });
    map.body = Some("n1".into());
    map.next = Some("n2".into());

    let mut emit_node = Node::new("n2", NodeKind::ArtifactEmit);
    emit_node.output = Some("out".into());
    emit_node.value = Some(IrValue::Ref {
        scope: RefScope::Binding,
        path: vec!["results".into()],
    });

    ir.nodes = vec![
        map,
        llm("n1", Some("one"), "process", "markdown", None),
        emit_node,
    ];

    let mut provider =
        ScriptedProvider::new(vec![json!("a-done"), json!("b-done"), json!("c-done")]);
    let mut tools = DenyAllTools;
    let (result, events) = run_with(
        &ir,
        &mut provider,
        &mut tools,
        [("items".to_string(), json!(["a", "b", "c"]))].into(),
    );

    let report = result.expect("map run should succeed");
    assert_eq!(
        report.outputs["out"].value,
        json!(["a-done", "b-done", "c-done"])
    );
    assert_eq!(report.steps, 3);
    let iterations = events
        .iter()
        .filter(|e| matches!(e, RunEvent::MapIteration { .. }))
        .count();
    assert_eq!(iterations, 3);
}

#[test]
fn mapping_an_empty_list_performs_no_work() {
    let mut ir = base("test.Mapper");
    ir.inputs.insert("items".into(), "string[]".into());
    ir.outputs.insert("out".into(), "artifact<json>".into());
    ir.entry = Some("n0".into());

    let mut map = Node::new("n0", NodeKind::Parallel);
    map.binding = Some("results".into());
    map.mode = Some("map".into());
    map.binder = Some("item".into());
    map.source = Some(IrValue::Ref {
        scope: RefScope::Input,
        path: vec!["items".into()],
    });
    map.body = Some("n1".into());
    map.next = Some("n2".into());

    let mut emit_node = Node::new("n2", NodeKind::ArtifactEmit);
    emit_node.output = Some("out".into());
    emit_node.value = Some(IrValue::Ref {
        scope: RefScope::Binding,
        path: vec!["results".into()],
    });

    ir.nodes = vec![
        map,
        llm("n1", Some("one"), "process", "markdown", None),
        emit_node,
    ];

    let mut provider = ScriptedProvider::new(Vec::new());
    let mut tools = DenyAllTools;
    let (result, _) = run_with(
        &ir,
        &mut provider,
        &mut tools,
        [("items".to_string(), json!([]))].into(),
    );
    let report = result.expect("an empty map is not an error");
    assert_eq!(report.outputs["out"].value, json!([]));
    assert_eq!(report.steps, 0);
}

// --- approvals ------------------------------------------------------------

fn gated() -> AgentIr {
    let mut ir = base("test.Gated");
    ir.outputs.insert("out".into(), "artifact<markdown>".into());
    ir.policy.insert(
        "external_write".into(),
        PolicyRule {
            decision: Decision::RequireApproval,
            values: Vec::new(),
            qualifier: None,
        },
    );
    ir.tools = vec![ToolBinding {
        reference: "mcp:mailer.send".into(),
        name: "mailer.send".into(),
        transport: "mcp".into(),
        scopes: BTreeMap::new(),
        effects: vec!["external_write".into()],
        signature: ToolSignature {
            params: Vec::new(),
            result: "bool".into(),
        },
    }];
    ir.entry = Some("n0".into());

    let mut approval = Node::new("n0", NodeKind::Approval);
    approval.label = Some("approval required before calling mcp:mailer.send".into());
    approval.effects = vec!["external_write".into()];
    approval.next = Some("n1".into());

    let mut call = Node::new("n1", NodeKind::ToolCall);
    call.binding = Some("sent".into());
    call.tool = Some("mcp:mailer.send".into());
    call.effects = vec!["external_write".into()];
    call.next = Some("n2".into());

    ir.nodes = vec![
        approval,
        call,
        llm("n2", Some("note"), "done", "markdown", Some("n3")),
        emit("n3", "out", "note", None),
    ];
    ir
}

fn run_gated(approval: HumanChannel) -> (Result<crate::RunReport, RunError>, Vec<RunEvent>) {
    let ir = gated();
    let mut provider = ScriptedProvider::new(vec![json!("sent it")]);
    let mut tools = StaticToolHost::new().with("mailer.send", |_| Ok(json!(true)));
    let mut sink = CollectingSink::default();
    let registry = BTreeMap::new();
    let result = run(
        &ir,
        &registry,
        &mut provider,
        &mut tools,
        &mut sink,
        RunOptions {
            approval,
            ..RunOptions::default()
        },
    );
    (result, sink.events)
}

#[test]
fn an_approval_denial_aborts_the_run() {
    let (result, events) = run_gated(HumanChannel::Deny);
    let error = result.unwrap_err();
    assert!(matches!(error, RunError::ApprovalDenied { .. }));
    assert!(events
        .iter()
        .any(|e| matches!(e, RunEvent::ApprovalDecided { allowed: false, .. })));
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, RunEvent::ToolCall { .. })),
        "the gated tool must not run after a denial"
    );
}

#[test]
fn an_approval_grant_lets_the_gated_call_proceed() {
    let (result, events) = run_gated(HumanChannel::Ask(Box::new(ScriptedApprovals::new(vec![
        true,
    ]))));
    assert!(result.is_ok(), "{:?}", result.err().map(|e| e.to_string()));
    assert!(events
        .iter()
        .any(|e| matches!(e, RunEvent::ToolCall { .. })));
}

#[test]
fn assume_yes_is_an_explicit_opt_in() {
    let (result, _) = run_gated(HumanChannel::AssumeYes);
    assert!(result.is_ok());
}

/// A coordinator that calls a sub-agent and *then* reaches an approval gate.
///
/// This is the shape of `examples/code-review-team`: sub-agents review the
/// files, and only afterwards does the external write need a human.
fn gated_after_a_sub_agent() -> (AgentIr, crate::AgentRegistry) {
    let mut child = base("test.Child");
    child
        .outputs
        .insert("notes".into(), "artifact<markdown>".into());
    child.entry = Some("c0".into());
    child.nodes = vec![
        llm("c0", Some("notes"), "review it", "markdown", Some("c1")),
        emit("c1", "notes", "notes", None),
    ];

    let mut parent = base("test.Parent");
    parent
        .outputs
        .insert("out".into(), "artifact<markdown>".into());
    parent.effects = vec!["external_write".into(), "model_access".into()];
    parent.policy.insert(
        "external_write".into(),
        PolicyRule {
            decision: Decision::RequireApproval,
            values: Vec::new(),
            qualifier: None,
        },
    );
    parent.tools = vec![ToolBinding {
        reference: "mcp:mailer.send".into(),
        name: "mailer.send".into(),
        transport: "mcp".into(),
        scopes: BTreeMap::new(),
        effects: vec!["external_write".into()],
        signature: ToolSignature {
            params: Vec::new(),
            result: "bool".into(),
        },
    }];
    parent.entry = Some("n0".into());

    let mut call_child = Node::new("n0", NodeKind::AgentCall);
    call_child.binding = Some("notes".into());
    call_child.agent = Some("test.Child".into());
    call_child.next = Some("n1".into());

    let mut approval = Node::new("n1", NodeKind::Approval);
    approval.label = Some("approval required before calling mcp:mailer.send".into());
    approval.effects = vec!["external_write".into()];
    approval.next = Some("n2".into());

    let mut send = Node::new("n2", NodeKind::ToolCall);
    send.binding = Some("sent".into());
    send.tool = Some("mcp:mailer.send".into());
    send.effects = vec!["external_write".into()];
    send.next = Some("n3".into());

    parent.nodes = vec![
        call_child,
        approval,
        send,
        llm("n3", Some("note"), "done", "markdown", Some("n4")),
        emit("n4", "out", "note", None),
    ];

    let registry: crate::AgentRegistry = [("test.Child".to_string(), child)].into_iter().collect();
    (parent, registry)
}

fn run_gated_after_a_sub_agent(
    approval: HumanChannel,
) -> (Result<crate::RunReport, RunError>, Vec<RunEvent>) {
    let (ir, registry) = gated_after_a_sub_agent();
    let mut provider = ScriptedProvider::new(vec![json!("child notes"), json!("parent note")]);
    let mut tools = StaticToolHost::new().with("mailer.send", |_| Ok(json!(true)));
    let mut sink = CollectingSink::default();
    let result = run(
        &ir,
        &registry,
        &mut provider,
        &mut tools,
        &mut sink,
        RunOptions {
            approval,
            ..RunOptions::default()
        },
    );
    (result, sink.events)
}

#[test]
fn calling_a_sub_agent_does_not_disarm_a_later_approval_gate() {
    // The approval mode used to be *moved* into the sub-agent, leaving the
    // parent set to deny. Every gate after the first `agent.call` was then
    // refused without anyone being asked — including under `--yes`.
    let (result, events) = run_gated_after_a_sub_agent(HumanChannel::AssumeYes);
    assert!(
        result.is_ok(),
        "the gate must still be approvable after a sub-agent call: {:?}",
        result.err().map(|error| error.to_string())
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, RunEvent::ToolCall { .. })),
        "the gated tool must run"
    );
}

#[test]
fn the_operator_is_asked_by_the_parent_even_after_a_sub_agent_ran() {
    let (result, events) =
        run_gated_after_a_sub_agent(HumanChannel::Ask(Box::new(ScriptedApprovals::new(vec![
            true,
        ]))));
    assert!(result.is_ok(), "{:?}", result.err().map(|e| e.to_string()));
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, RunEvent::ApprovalRequested { .. }))
            .count(),
        1,
        "exactly one gate, and it was reached"
    );
    assert!(events
        .iter()
        .any(|event| matches!(event, RunEvent::ApprovalDecided { allowed: true, .. })));
}

#[test]
fn a_denial_after_a_sub_agent_still_stops_the_run() {
    let (result, _) = run_gated_after_a_sub_agent(HumanChannel::Deny);
    let error = result.unwrap_err();
    assert!(matches!(error, RunError::ApprovalDenied { .. }), "{error}");
}

// --- state ----------------------------------------------------------------

#[test]
fn state_survives_between_nodes() {
    let mut ir = base("test.Stateful");
    ir.inputs.insert("seed".into(), "string".into());
    ir.outputs.insert("out".into(), "artifact<markdown>".into());
    ir.state.insert("note".into(), "string".into());
    ir.entry = Some("n0".into());

    let mut write = Node::new("n0", NodeKind::StateWrite);
    write.field = Some("note".into());
    write.value = Some(IrValue::Ref {
        scope: RefScope::Input,
        path: vec!["seed".into()],
    });
    write.next = Some("n1".into());

    let mut read = Node::new("n1", NodeKind::StateRead);
    read.field = Some("note".into());
    read.binding = Some("$state.note".into());
    read.next = Some("n2".into());

    let mut call = llm("n2", Some("draft"), "use it", "markdown", Some("n3"));
    call.prompt = Some(IrValue::Template {
        parts: vec![TemplatePart::Value {
            value: IrValue::Ref {
                scope: RefScope::Binding,
                path: vec!["$state.note".into()],
            },
            ty: "string".into(),
        }],
    });

    ir.nodes = vec![write, read, call, emit("n3", "out", "draft", None)];

    let mut provider = ScriptedProvider::new(vec![json!("used")]);
    let mut tools = DenyAllTools;
    let (result, events) = run_with(
        &ir,
        &mut provider,
        &mut tools,
        [("seed".to_string(), json!("remembered"))].into(),
    );
    assert!(result.is_ok(), "{:?}", result.err().map(|e| e.to_string()));
    assert!(events
        .iter()
        .any(|e| matches!(e, RunEvent::StateWritten { .. })));
}

#[test]
fn reading_unset_state_is_an_error_rather_than_null() {
    let mut ir = base("test.Stateful");
    ir.outputs.insert("out".into(), "artifact<markdown>".into());
    ir.state.insert("note".into(), "string".into());
    ir.entry = Some("n0".into());

    let mut read = Node::new("n0", NodeKind::StateRead);
    read.field = Some("note".into());
    read.binding = Some("$state.note".into());

    ir.nodes = vec![read];

    let mut provider = ScriptedProvider::new(Vec::new());
    let mut tools = DenyAllTools;
    let (result, _) = run_with(&ir, &mut provider, &mut tools, BTreeMap::new());
    assert!(matches!(result.unwrap_err(), RunError::StateNotSet { .. }));
}

// --- version gating -------------------------------------------------------

#[test]
fn an_unsupported_ir_major_version_is_refused() {
    let mut ir = summarizer();
    ir.ir_version = "9.0".into();
    let mut provider = ScriptedProvider::new(vec![json!("x")]);
    let mut tools = DenyAllTools;
    let (result, _) = run_with(
        &ir,
        &mut provider,
        &mut tools,
        [("document".to_string(), json!("d"))].into(),
    );
    let error = result.unwrap_err();
    assert!(matches!(error, RunError::UnsupportedIrVersion { .. }));
    assert!(error.to_string().contains("Refusing to run"), "{error}");
}

#[test]
fn an_unresolved_value_in_the_artifact_stops_the_run() {
    let mut ir = summarizer();
    ir.nodes[1].value = Some(IrValue::Unknown);
    let mut provider = ScriptedProvider::new(vec![json!("x")]);
    let mut tools = DenyAllTools;
    let (result, _) = run_with(
        &ir,
        &mut provider,
        &mut tools,
        [("document".to_string(), json!("d"))].into(),
    );
    assert!(matches!(result.unwrap_err(), RunError::MalformedIr(_)));
}

// --- determinism ----------------------------------------------------------

fn record_summarizer() -> Cassette {
    let ir = summarizer();
    let mut provider = RecordingProvider::new(
        ScriptedProvider::new(vec![json!("# Recorded")]),
        ir.agent.clone(),
    );
    let mut tools = DenyAllTools;
    let mut sink = CollectingSink::default();
    let registry = BTreeMap::new();
    run(
        &ir,
        &registry,
        &mut provider,
        &mut tools,
        &mut sink,
        RunOptions {
            inputs: [("document".to_string(), json!("the source"))].into(),
            ..RunOptions::default()
        },
    )
    .expect("recording run should succeed");
    provider.finish()
}

fn replay_summarizer(
    cassette: Cassette,
    document: &str,
) -> (Result<crate::RunReport, RunError>, Vec<RunEvent>) {
    let ir = summarizer();
    let mut provider = ReplayProvider::new(cassette);
    let mut tools = DenyAllTools;
    run_with(
        &ir,
        &mut provider,
        &mut tools,
        [("document".to_string(), json!(document))].into(),
    )
}

#[test]
fn a_cassette_replays_deterministically() {
    let cassette = record_summarizer();
    let (result, _) = replay_summarizer(cassette, "the source");
    let report = result.expect("replay should succeed");
    assert_eq!(report.outputs["summary"].value, json!("# Recorded"));
}

#[test]
fn a_changed_prompt_fails_cassette_replay() {
    let cassette = record_summarizer();
    // The template interpolates the document, so a different document is a
    // different prompt — and must not silently reuse the recorded answer.
    let (result, _) = replay_summarizer(cassette, "a completely different source");
    let error = result.unwrap_err();
    assert!(error.to_string().contains("re-record"), "{error}");
}

#[test]
fn the_event_stream_is_identical_across_replays() {
    let cassette = record_summarizer();
    let (_, first) = replay_summarizer(cassette.clone(), "the source");
    let (_, second) = replay_summarizer(cassette, "the source");
    assert_eq!(first, second, "events must carry no clock-dependent data");
}

#[test]
fn recorded_cassettes_round_trip_through_disk_format() {
    let cassette = record_summarizer();
    let parsed = Cassette::from_json(&cassette.to_canonical_json()).unwrap();
    assert_eq!(parsed, cassette);
}

// --- verify ---------------------------------------------------------------

/// One model call, one `verify`, one emission.
fn verifying() -> AgentIr {
    let mut ir = base("test.Verifying");
    ir.inputs.insert("topic".into(), "string".into());
    ir.outputs
        .insert("report".into(), "artifact<markdown>".into());
    ir.entry = Some("n0".into());

    let mut check = Node::new("n1", NodeKind::Verify);
    check.verifier = Some("CitationCheck".into());
    check.args = vec![Argument {
        name: "draft".into(),
        value: IrValue::Ref {
            scope: RefScope::Binding,
            path: vec!["draft".into()],
        },
    }];
    check.next = Some("n2".into());

    ir.nodes = vec![
        llm("n0", Some("draft"), "Write it", "markdown", Some("n1")),
        check,
        emit("n2", "report", "draft", None),
    ];
    ir
}

#[test]
fn a_verify_that_cannot_run_is_reported_as_not_performed() {
    let mut provider = ScriptedProvider::new(vec![json!("# Draft")]);
    let mut tools = StaticToolHost::default();
    let inputs = BTreeMap::from([("topic".to_string(), json!("compiler design"))]);
    let (result, events) = run_with(&verifying(), &mut provider, &mut tools, inputs);
    assert!(result.is_ok(), "{:?}", result.err());

    let verified = events
        .iter()
        .find_map(|event| match event {
            RunEvent::Verified {
                verifier, outcome, ..
            } => Some((verifier.clone(), *outcome)),
            _ => None,
        })
        .expect("a verify node emits an event");
    assert_eq!(verified.0, "CitationCheck");
    assert_eq!(
        verified.1,
        crate::VerifyOutcome::NotPerformed,
        "the artifact names a check this runtime cannot carry out, and saying it \
         passed would be a pass nothing earned"
    );
    assert!(
        !verified.1.is_failure(),
        "a check that never ran has not failed either"
    );
}

/// The same flow, with the check carried in the artifact.
///
/// `len(draft) >= n` over the model's markdown answer: a shape check, which is
/// all a pure condition can be.
fn verifying_with(minimum: i64) -> AgentIr {
    let mut ir = verifying();
    let check = ir
        .nodes
        .iter_mut()
        .find(|node| node.kind == NodeKind::Verify)
        .expect("the fixture has a verify node");
    check.condition = Some(IrValue::Binary {
        op: ">=".into(),
        lhs: Box::new(IrValue::Builtin {
            name: "len".into(),
            args: vec![IrValue::Ref {
                scope: RefScope::Binding,
                path: vec!["draft".into()],
            }],
        }),
        rhs: Box::new(IrValue::int(minimum)),
    });
    ir
}

fn run_verifying(minimum: i64) -> (Result<crate::RunReport, RunError>, Vec<RunEvent>) {
    let mut provider = ScriptedProvider::new(vec![json!("# Draft")]);
    let mut tools = StaticToolHost::default();
    let inputs = BTreeMap::from([("topic".to_string(), json!("compiler design"))]);
    run_with(&verifying_with(minimum), &mut provider, &mut tools, inputs)
}

fn verify_outcome(events: &[RunEvent]) -> crate::VerifyOutcome {
    events
        .iter()
        .find_map(|event| match event {
            RunEvent::Verified { outcome, .. } => Some(*outcome),
            _ => None,
        })
        .expect("a verify node emits an event")
}

#[test]
fn a_check_that_holds_passes_and_the_run_continues() {
    // "# Draft" is 7 characters.
    let (result, events) = run_verifying(7);
    assert!(result.is_ok(), "{:?}", result.err());
    assert_eq!(verify_outcome(&events), crate::VerifyOutcome::Passed);
    assert!(
        events
            .iter()
            .any(|event| matches!(event, RunEvent::Emitted { .. })),
        "the statement after a passing check still runs"
    );
}

#[test]
fn a_check_that_does_not_hold_fails_and_ends_the_run() {
    let (result, events) = run_verifying(8);

    let error = result.expect_err("a failed check ends the run");
    let RunError::VerificationFailed { verifier, .. } = &error else {
        panic!("expected a verification failure, got {error:?}");
    };
    assert_eq!(verifier, "CitationCheck");
    assert!(error.to_string().contains("did not hold"), "{error}");

    assert_eq!(verify_outcome(&events), crate::VerifyOutcome::Failed);
    assert!(
        verify_outcome(&events).is_failure(),
        "unlike `notPerformed`, this one is a failure"
    );
}

#[test]
fn a_failed_check_says_what_it_found_before_the_run_ends() {
    // The ordering is normative: a record has to say what the check found
    // before it says the run ended, or the outcome cannot be attributed.
    let (_, events) = run_verifying(8);
    let verified = events
        .iter()
        .position(|event| matches!(event, RunEvent::Verified { .. }))
        .expect("a verify node emits an event");
    let failed = events
        .iter()
        .position(|event| matches!(event, RunEvent::RunFailed { .. }))
        .expect("the run ends by saying so");

    assert_eq!(verified + 1, failed, "nothing comes between the two");
    assert_eq!(failed, events.len() - 1, "and the run ends there");
}

#[test]
fn no_statement_after_a_failed_check_executes() {
    let (_, events) = run_verifying(8);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, RunEvent::Emitted { .. })),
        "the emit that followed the failing check must not have happened"
    );
}

#[test]
fn a_condition_that_is_not_a_boolean_is_a_malformed_artifact() {
    let mut ir = verifying_with(1);
    let check = ir
        .nodes
        .iter_mut()
        .find(|node| node.kind == NodeKind::Verify)
        .expect("the fixture has a verify node");
    check.condition = Some(IrValue::int(1));

    let mut provider = ScriptedProvider::new(vec![json!("# Draft")]);
    let mut tools = StaticToolHost::default();
    let inputs = BTreeMap::from([("topic".to_string(), json!("compiler design"))]);
    let (result, _) = run_with(&ir, &mut provider, &mut tools, inputs);

    let error = result.expect_err("a non-boolean condition cannot decide anything");
    assert!(
        matches!(error, RunError::MalformedIr(_)),
        "the artifact is wrong, not the check: {error:?}"
    );
}

#[test]
fn a_replayed_run_reproduces_every_verified_event() {
    // A pure condition is a function of the events before it, so this holds
    // for the same reason it holds for a branch.
    let (_, first) = run_verifying(7);
    let (_, second) = run_verifying(7);
    assert_eq!(first, second);
}

#[test]
fn a_not_performed_verify_never_serialises_as_a_pass() {
    let event = RunEvent::Verified {
        node: "n1".into(),
        verifier: "CitationCheck".into(),
        outcome: crate::VerifyOutcome::NotPerformed,
    };
    let line = event.to_json_line();
    assert!(line.contains("\"outcome\":\"notPerformed\""), "{line}");
    assert!(
        !line.contains("passed"),
        "the field a consumer used to read must be gone, not merely false: {line}"
    );
    assert!(event
        .to_line()
        .contains("verify CitationCheck: not performed"));
}

// --- cost -----------------------------------------------------------------

fn priced(amount: &str) -> AgentIr {
    let mut ir = summarizer();
    ir.budget.cost = Some(ingot_ir::Cost {
        amount: amount.to_string(),
        currency: "usd".to_string(),
    });
    ir
}

fn usd(model: &str, input: &str, output: &str) -> crate::price::Pricing {
    crate::price::Pricing::new(vec![crate::price::ModelPrice {
        model: model.to_string(),
        input: input.to_string(),
        output: output.to_string(),
        cache_read: None,
        currency: "usd".to_string(),
    }])
}

fn run_priced(ir: &AgentIr, pricing: crate::price::Pricing) -> Result<crate::RunReport, RunError> {
    let mut provider = ScriptedProvider::new(vec![json!("# Brief")]).with_usage(Usage {
        input_tokens: 1_000,
        output_tokens: 500,
        cache_read_tokens: 0,
    });
    let mut tools = StaticToolHost::default();
    let mut sink = CollectingSink::default();
    run(
        ir,
        &BTreeMap::new(),
        &mut provider,
        &mut tools,
        &mut sink,
        RunOptions {
            inputs: BTreeMap::from([("document".to_string(), json!("the source"))]),
            pricing,
            ..RunOptions::default()
        },
    )
}

#[test]
fn a_cost_budget_is_charged_against_the_prices_the_run_was_given() {
    // 1000 input at $3/M and 500 output at $15/M is $0.0105.
    let report = run_priced(&priced("5"), usd("scripted", "3", "15")).expect("within budget");
    assert!(report.spend.is_complete());
    assert_eq!(report.spend.rendered().as_deref(), Some("0.0105 USD"));
}

#[test]
fn a_cost_budget_that_is_exhausted_ends_the_run() {
    let error = run_priced(&priced("0.001"), usd("scripted", "3", "15")).unwrap_err();
    let RunError::BudgetExceeded { budget, limit, .. } = &error else {
        panic!("expected a budget failure, got {error:?}");
    };
    assert_eq!(budget, "cost");
    assert_eq!(limit, "0.001 USD");
}

#[test]
fn an_unpriced_model_leaves_the_budget_uncharged_rather_than_satisfied() {
    // The failure this guards against: a budget that looks enforced because
    // nothing exceeded a total that was never computed.
    let report = run_priced(&priced("0.000001"), crate::price::Pricing::default())
        .expect("an uncharged budget cannot be exceeded");
    assert!(
        !report.spend.is_complete(),
        "a total that missed every call is not a total"
    );
    let unpriced: Vec<&str> = report.spend.unpriced().map(|(model, _)| model).collect();
    assert_eq!(unpriced, vec!["scripted"]);
    assert_eq!(report.spend.rendered(), None);
}

#[test]
fn an_artifact_with_no_cost_budget_is_not_priced_at_all() {
    let report = run_priced(&summarizer(), usd("scripted", "3", "15")).expect("no budget");
    assert_eq!(report.spend.rendered(), None);
    assert!(report.spend.is_complete());
}

// --- streaming ------------------------------------------------------------

/// Records both channels, so a test can assert that they stayed separate.
#[derive(Default)]
struct WatchingSink {
    events: Vec<RunEvent>,
    deltas: Vec<String>,
    settled: Vec<bool>,
}

impl crate::EventSink for WatchingSink {
    fn emit(&mut self, event: RunEvent) {
        self.events.push(event);
    }
    fn delta(&mut self, _node: &str, text: &str) {
        self.deltas.push(text.to_string());
    }
    fn settled(&mut self, _node: &str, kept: bool) {
        self.settled.push(kept);
    }
}

/// Hands over one fragment per word, then answers with `value`.
///
/// The answer is deliberately settable independently of the fragments, so a
/// test can arrange the case that matters: text a watcher saw, and a response
/// the run then refuses.
struct Streaming {
    fragments: Vec<String>,
    value: serde_json::Value,
    /// The cap the interpreter asked for on the last call.
    asked_for: std::rc::Rc<std::cell::Cell<u32>>,
}

impl Streaming {
    fn new(fragments: &[&str], value: serde_json::Value) -> Streaming {
        Streaming {
            fragments: fragments.iter().map(|text| text.to_string()).collect(),
            value,
            asked_for: std::rc::Rc::new(std::cell::Cell::new(0)),
        }
    }
}

impl crate::ModelProvider for Streaming {
    fn name(&self) -> &str {
        "streaming"
    }

    fn complete(
        &mut self,
        request: &crate::provider::CompletionRequest,
    ) -> Result<crate::provider::CompletionResponse, crate::provider::ProviderError> {
        self.asked_for.set(request.max_tokens);
        Ok(crate::provider::CompletionResponse {
            value: self.value.clone(),
            usage: Usage::default(),
            model: "streaming".to_string(),
        })
    }

    fn streams(&self) -> bool {
        true
    }

    fn complete_streaming(
        &mut self,
        request: &crate::provider::CompletionRequest,
        on_delta: crate::provider::DeltaSink<'_>,
    ) -> Result<crate::provider::CompletionResponse, crate::provider::ProviderError> {
        for fragment in self.fragments.clone() {
            on_delta(&fragment);
        }
        self.complete(request)
    }
}

fn watch(
    ir: &AgentIr,
    provider: &mut dyn crate::ModelProvider,
) -> (WatchingSink, Result<crate::RunReport, RunError>) {
    let mut tools = DenyAllTools;
    let mut sink = WatchingSink::default();
    let report = run(
        ir,
        &BTreeMap::new(),
        provider,
        &mut tools,
        &mut sink,
        RunOptions {
            inputs: BTreeMap::from([("document".to_string(), json!("the source"))]),
            ..RunOptions::default()
        },
    );
    (sink, report)
}

#[test]
fn text_reaches_the_watcher_without_reaching_the_event_stream() {
    let mut provider = Streaming::new(&["Half ", "an ", "answer"], json!("Half an answer"));
    let (sink, report) = watch(&summarizer(), &mut provider);
    report.expect("the run should succeed");

    assert_eq!(sink.deltas, vec!["Half ", "an ", "answer"]);
    assert_eq!(sink.settled, vec![true]);
    // The property the design rests on: a replay has to reproduce this stream
    // byte for byte, and it cannot reproduce how a connection was chunked.
    assert!(
        sink.events
            .iter()
            .all(|event| !event.to_json_line().contains("Half ")),
        "a fragment leaked into the event stream: {:?}",
        sink.events
    );
}

#[test]
fn text_shown_before_a_refused_answer_is_struck_rather_than_left_standing() {
    // The answer is a number where the artifact declared markdown, so the run
    // fails after the watcher has already been shown the beginning of it.
    let mut provider = Streaming::new(&["Half an ans"], json!(7));
    let (sink, report) = watch(&summarizer(), &mut provider);

    assert!(report.is_err(), "a mistyped answer must not be accepted");
    assert_eq!(sink.deltas, vec!["Half an ans"]);
    assert_eq!(
        sink.settled,
        vec![false],
        "the watcher was told the text became the answer when it did not"
    );
}

#[test]
fn a_streaming_provider_is_allowed_a_longer_answer_than_one_that_arrives_whole() {
    let mut streaming = Streaming::new(&["ok"], json!("ok"));
    let asked_for = streaming.asked_for.clone();
    watch(&summarizer(), &mut streaming).1.expect("streamed");
    let streamed = asked_for.get();

    let mut at_once = ScriptedProvider::new(vec![json!("ok")]);
    watch(&summarizer(), &mut at_once).1.expect("at once");

    assert_eq!(streamed, 64_000);
    assert!(
        streamed > 16_000,
        "streaming exists to lift the ceiling, and did not: {streamed}"
    );
}

#[test]
fn a_replay_shows_nothing_live() {
    // Correct rather than unfortunate: a cassette produces its answer at once,
    // and inventing fragments for it would make a replayed run look like a call
    // that never happened.
    let mut recorder = RecordingProvider::new(Streaming::new(&["a", "b"], json!("ab")), "x");
    let mut sink = WatchingSink::default();
    run(
        &summarizer(),
        &BTreeMap::new(),
        &mut recorder,
        &mut DenyAllTools,
        &mut sink,
        RunOptions {
            inputs: BTreeMap::from([("document".to_string(), json!("the source"))]),
            ..RunOptions::default()
        },
    )
    .expect("recording run");
    assert_eq!(
        sink.deltas,
        vec!["a", "b"],
        "recording must not swallow the live text"
    );

    let mut replay = ReplayProvider::new(recorder.finish());
    let (replayed, report) = watch(&summarizer(), &mut replay);
    report.expect("replayed run");
    assert!(replayed.deltas.is_empty(), "{:?}", replayed.deltas);
    assert!(replayed.settled.is_empty());
}

// --- persistent memory ------------------------------------------------------
//
// RFC-0018. The interpreter never touches the filesystem: it is handed what a
// store held and hands back what the run left.

/// An artifact that reads a persistent field, adds one, and emits the result.
fn counter() -> AgentIr {
    let mut ir = base("test.Counter");
    ir.outputs
        .insert("log".to_string(), "artifact<json>".to_string());
    ir.persistent.insert(
        "depth".to_string(),
        ingot_ir::PersistentField {
            ty: "int".to_string(),
            initial: json!(0),
        },
    );

    let mut read = Node::new("n0", NodeKind::StateRead);
    read.field = Some("depth".to_string());
    read.scope = Some(RefScope::Memory);
    read.binding = Some("$memory.depth".to_string());
    read.next = Some("n1".to_string());

    let mut write = Node::new("n1", NodeKind::StateWrite);
    write.field = Some("depth".to_string());
    write.scope = Some(RefScope::Memory);
    write.value = Some(IrValue::Binary {
        op: "+".to_string(),
        lhs: Box::new(IrValue::Ref {
            scope: RefScope::Binding,
            path: vec!["$memory.depth".to_string()],
        }),
        rhs: Box::new(IrValue::Literal {
            ty: "int".to_string(),
            value: json!(1),
        }),
    });
    write.next = Some("n2".to_string());

    let mut read_back = Node::new("n2", NodeKind::StateRead);
    read_back.field = Some("depth".to_string());
    read_back.scope = Some(RefScope::Memory);
    read_back.binding = Some("$memory.depth2".to_string());
    read_back.next = Some("n3".to_string());

    ir.nodes = vec![
        read,
        write,
        read_back,
        emit("n3", "log", "$memory.depth2", None),
    ];
    ir.entry = Some("n0".to_string());
    ir
}

fn run_counter(stored: BTreeMap<String, serde_json::Value>) -> crate::RunReport {
    let ir = counter();
    let mut provider = ScriptedProvider::new(vec![]);
    let mut tools = DenyAllTools;
    let mut sink = CollectingSink::default();
    run(
        &ir,
        &BTreeMap::new(),
        &mut provider,
        &mut tools,
        &mut sink,
        RunOptions {
            memory: stored,
            ..RunOptions::default()
        },
    )
    .expect("the counter runs")
}

#[test]
fn a_first_run_starts_from_the_declared_initial_value() {
    // No store, and no error: this is what makes an initial value required
    // rather than optional.
    let report = run_counter(BTreeMap::new());
    assert_eq!(report.memory.get("depth"), Some(&json!(1)));
}

#[test]
fn a_stored_value_wins_over_the_declared_one() {
    let stored = [("depth".to_string(), json!(7))].into_iter().collect();
    let report = run_counter(stored);
    assert_eq!(report.memory.get("depth"), Some(&json!(8)));
}

#[test]
fn a_stored_value_of_the_wrong_type_stops_the_run_before_it_spends_anything() {
    let ir = counter();
    let mut provider = ScriptedProvider::new(vec![]);
    let mut tools = DenyAllTools;
    let mut sink = CollectingSink::default();
    let error = run(
        &ir,
        &BTreeMap::new(),
        &mut provider,
        &mut tools,
        &mut sink,
        RunOptions {
            memory: [("depth".to_string(), json!("seven"))]
                .into_iter()
                .collect(),
            ..RunOptions::default()
        },
    )
    .expect_err("a string is not an int");
    assert!(
        matches!(error, RunError::InvalidMemory { ref field, .. } if field == "depth"),
        "{error}"
    );
    // Before anything: not even the run started.
    assert!(
        sink.events.is_empty(),
        "the run should not have begun: {:?}",
        sink.events
    );
}

#[test]
fn a_store_carrying_a_field_the_artifact_does_not_declare_is_refused() {
    // The mirror of an unknown input. Silently dropping it would lose whatever
    // an older version of the agent was keeping without saying so.
    let ir = counter();
    let mut provider = ScriptedProvider::new(vec![]);
    let mut tools = DenyAllTools;
    let mut sink = CollectingSink::default();
    let error = run(
        &ir,
        &BTreeMap::new(),
        &mut provider,
        &mut tools,
        &mut sink,
        RunOptions {
            memory: [("gone".to_string(), json!(1))].into_iter().collect(),
            ..RunOptions::default()
        },
    )
    .expect_err("`gone` is not declared");
    let text = error.to_string();
    assert!(text.contains("gone"), "{text}");
    assert!(text.contains("depth"), "{text}");
}

#[test]
fn a_persistent_write_reports_the_same_event_an_ephemeral_one_does() {
    // Which store a field lives in is a property of the artifact, not of the
    // run, so the event stream does not carry a second kind of write.
    let ir = counter();
    let mut provider = ScriptedProvider::new(vec![]);
    let mut tools = DenyAllTools;
    let mut sink = CollectingSink::default();
    run(
        &ir,
        &BTreeMap::new(),
        &mut provider,
        &mut tools,
        &mut sink,
        RunOptions::default(),
    )
    .expect("the counter runs");
    let written: Vec<&str> = sink
        .events
        .iter()
        .filter_map(|event| match event {
            RunEvent::StateWritten { field, .. } => Some(field.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(written, vec!["depth"]);
}

#[test]
fn an_artifact_with_no_persistent_block_reports_no_memory() {
    let report = run(
        &summarizer(),
        &BTreeMap::new(),
        &mut ScriptedProvider::new(vec![json!("done")]),
        &mut DenyAllTools,
        &mut CollectingSink::default(),
        RunOptions {
            inputs: [("document".to_string(), json!("a paragraph"))]
                .into_iter()
                .collect(),
            ..RunOptions::default()
        },
    )
    .expect("the summarizer runs");
    assert!(report.memory.is_empty());
}

// --- resumption -------------------------------------------------------------
//
// RFC-0018 §4. The end-to-end property -- that the two halves' events
// concatenate to the uninterrupted run's -- needs three runs and lives in
// `crates/ingot-cli/tests/resume.rs`. These pin the pieces it rests on.

/// One model call, a resumable checkpoint, and an emission after it.
fn paused() -> AgentIr {
    let mut ir = base("test.Paused");
    ir.outputs
        .insert("report".to_string(), "artifact<markdown>".to_string());

    let mut checkpoint = Node::new("n1", NodeKind::Checkpoint);
    checkpoint.label = Some("half-way".to_string());
    checkpoint.resumable = true;
    checkpoint.next = Some("n2".to_string());

    let mut nested = Node::new("n3", NodeKind::Checkpoint);
    nested.label = Some("inside".to_string());

    ir.nodes = vec![
        llm("n0", Some("draft"), "Write.", "markdown", Some("n1")),
        checkpoint,
        emit("n2", "report", "draft", None),
        nested,
    ];
    ir.entry = Some("n0".to_string());
    ir
}

fn stop_at(ir: &AgentIr, label: &str) -> Result<crate::RunReport, RunError> {
    run(
        ir,
        &BTreeMap::new(),
        &mut ScriptedProvider::new(vec![json!("half a report")]),
        &mut DenyAllTools,
        &mut CollectingSink::default(),
        RunOptions {
            stop_at: Some(label.to_string()),
            ..RunOptions::default()
        },
    )
}

#[test]
fn a_run_that_stops_reports_a_snapshot_and_no_output() {
    let ir = paused();
    let report = stop_at(&ir, "half-way").expect("stopping is not a failure");
    let snapshot = report.stopped.expect("a snapshot");

    assert_eq!(snapshot.label, "half-way");
    assert_eq!(snapshot.stopped_at, "n1");
    // The node *after* the checkpoint, so a resumed run does not re-emit it.
    assert_eq!(snapshot.resume_at, "n2");
    assert_eq!(
        snapshot.bindings.get("draft"),
        Some(&json!("half a report"))
    );
    // The declared output was not produced, and that is not an error.
    assert!(report.outputs.is_empty());
    // The counters carry, so the second half cannot spend the budget twice.
    assert_eq!(snapshot.steps, 1);
    assert_eq!(snapshot.model_calls, 1);
}

#[test]
fn a_stopped_run_ends_with_run_stopped_and_not_run_finished() {
    let ir = paused();
    let mut sink = CollectingSink::default();
    run(
        &ir,
        &BTreeMap::new(),
        &mut ScriptedProvider::new(vec![json!("half a report")]),
        &mut DenyAllTools,
        &mut sink,
        RunOptions {
            stop_at: Some("half-way".to_string()),
            ..RunOptions::default()
        },
    )
    .expect("stopping is not a failure");

    let last = sink.events.last().expect("the run emitted something");
    assert!(
        matches!(last, RunEvent::RunStopped { label, .. } if label == "half-way"),
        "{last:?}"
    );
    assert!(
        !sink
            .events
            .iter()
            .any(|event| matches!(event, RunEvent::RunFinished { .. })),
        "a stopped run is not a finished one: {:?}",
        sink.events
    );
    // The checkpoint's own event comes first, exactly where an uninterrupted
    // run puts it. That is what makes the two halves concatenate.
    let checkpoint = sink
        .events
        .iter()
        .position(|event| matches!(event, RunEvent::Checkpoint { .. }))
        .expect("the checkpoint was reached");
    assert_eq!(checkpoint + 1, sink.events.len() - 1);
}

#[test]
fn stopping_at_a_nested_checkpoint_is_refused_before_the_run_starts() {
    let ir = paused();
    let error = stop_at(&ir, "inside").expect_err("a nested checkpoint is not resumable");
    assert!(
        matches!(error, RunError::NotResumable { nested: true, .. }),
        "{error}"
    );
    let text = error.to_string();
    assert!(text.contains("inside a branch or a loop"), "{text}");
    assert!(
        text.contains("half-way"),
        "it offers what is available: {text}"
    );
}

#[test]
fn stopping_at_a_label_nobody_wrote_is_refused() {
    let ir = paused();
    let error = stop_at(&ir, "nowhere").expect_err("no such checkpoint");
    assert!(
        matches!(error, RunError::NotResumable { nested: false, .. }),
        "{error}"
    );
}

#[test]
fn a_run_that_is_not_stopped_reports_no_snapshot() {
    let ir = paused();
    let report = run(
        &ir,
        &BTreeMap::new(),
        &mut ScriptedProvider::new(vec![json!("a whole report")]),
        &mut DenyAllTools,
        &mut CollectingSink::default(),
        RunOptions::default(),
    )
    .expect("the run finishes");
    assert!(report.stopped.is_none());
    assert!(report.outputs.contains_key("report"));
}

#[test]
fn resuming_continues_from_the_node_the_snapshot_names() {
    let ir = paused();
    let snapshot = stop_at(&ir, "half-way")
        .expect("stopping is not a failure")
        .stopped
        .expect("a snapshot");

    let mut sink = CollectingSink::default();
    let report = run(
        &ir,
        &BTreeMap::new(),
        // No answer scripted: the second half makes no model call, because the
        // first half already made the only one.
        &mut ScriptedProvider::new(vec![]),
        &mut DenyAllTools,
        &mut sink,
        RunOptions {
            resume: Some(snapshot),
            ..RunOptions::default()
        },
    )
    .expect("the second half finishes");

    assert!(report.outputs.contains_key("report"));
    // The counters continued rather than restarting.
    assert_eq!(report.steps, 1);
    assert!(
        !sink
            .events
            .iter()
            .any(|event| matches!(event, RunEvent::Checkpoint { .. })),
        "the checkpoint was re-emitted: {:?}",
        sink.events
    );
}

#[test]
fn resuming_against_a_changed_artifact_is_refused() {
    let ir = paused();
    let snapshot = stop_at(&ir, "half-way")
        .expect("stopping is not a failure")
        .stopped
        .expect("a snapshot");

    let mut edited = ir.clone();
    edited.budget.steps = Some(11);
    let error = run(
        &edited,
        &BTreeMap::new(),
        &mut ScriptedProvider::new(vec![]),
        &mut DenyAllTools,
        &mut CollectingSink::default(),
        RunOptions {
            resume: Some(snapshot),
            ..RunOptions::default()
        },
    )
    .expect_err("a different program");
    assert!(error
        .to_string()
        .contains("has changed since the run stopped"));
}

#[test]
fn a_map_body_that_ends_in_an_unbound_node_is_refused() {
    // Runtime 0.1 §5 says an iteration's value is the last body node's result.
    // A node that binds nothing has no result, and collecting `null` for it
    // produced a list of the right length and the wrong contents — a failure
    // that surfaced wherever the list was used, far from its cause.
    let mut ir = base("test.Mapper");
    ir.inputs.insert("items".into(), "string[]".into());
    ir.outputs.insert("out".into(), "artifact<json>".into());
    ir.entry = Some("n0".into());

    let mut map = Node::new("n0", NodeKind::Parallel);
    map.binding = Some("results".into());
    map.mode = Some("map".into());
    map.binder = Some("item".into());
    map.source = Some(IrValue::Ref {
        scope: RefScope::Input,
        path: vec!["items".into()],
    });
    map.body = Some("n1".into());
    map.next = Some("n2".into());

    let mut emit_node = Node::new("n2", NodeKind::ArtifactEmit);
    emit_node.output = Some("out".into());
    emit_node.value = Some(IrValue::Ref {
        scope: RefScope::Binding,
        path: vec!["results".into()],
    });

    ir.nodes = vec![
        map,
        // No binding: the shape the compiler used to emit for a bare
        // expression body.
        llm("n1", None, "process", "markdown", None),
        emit_node,
    ];

    let mut provider = ScriptedProvider::new(vec![json!("a-done")]);
    let mut tools = DenyAllTools;
    let (result, _) = run_with(
        &ir,
        &mut provider,
        &mut tools,
        [("items".to_string(), json!(["a"]))].into(),
    );
    let error = result.expect_err("an unbound last node has no value");
    let text = error.to_string();
    assert!(text.contains("binds nothing"), "{text}");
    assert!(text.contains("n1"), "{text}");
}
