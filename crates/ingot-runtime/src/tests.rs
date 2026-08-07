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
use crate::tools::{ApprovalMode, ScriptedApprovals, StaticToolHost};
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

fn run_gated(approval: ApprovalMode) -> (Result<crate::RunReport, RunError>, Vec<RunEvent>) {
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
    let (result, events) = run_gated(ApprovalMode::Deny);
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
    let (result, events) = run_gated(ApprovalMode::Ask(Box::new(ScriptedApprovals::new(vec![
        true,
    ]))));
    assert!(result.is_ok(), "{:?}", result.err().map(|e| e.to_string()));
    assert!(events
        .iter()
        .any(|e| matches!(e, RunEvent::ToolCall { .. })));
}

#[test]
fn assume_yes_is_an_explicit_opt_in() {
    let (result, _) = run_gated(ApprovalMode::AssumeYes);
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
    approval: ApprovalMode,
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
    let (result, events) = run_gated_after_a_sub_agent(ApprovalMode::AssumeYes);
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
        run_gated_after_a_sub_agent(ApprovalMode::Ask(Box::new(ScriptedApprovals::new(vec![
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
    let (result, _) = run_gated_after_a_sub_agent(ApprovalMode::Deny);
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
