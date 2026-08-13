//! The interpreter.
//!
//! Walks an [`AgentIr`] node graph and executes it. Two properties matter more
//! than speed:
//!
//! * **Every guarantee is re-checked.** Capabilities, budgets, loop bounds and
//!   approvals are enforced here from the artifact's own data, not trusted
//!   because a compiler once looked at the source. Whoever runs an artifact is
//!   often not whoever built it.
//! * **`parallel` runs sequentially.** The compiler guarantees a map body
//!   contains no state write, no emission and no checkpoint, so iterations
//!   cannot observe each other and sequential execution yields the same result.
//!   The IR node names an opportunity for concurrency, not an obligation.

use std::collections::BTreeMap;

use ingot_ir::{AgentIr, Decision, Node, NodeKind, RefScope, TemplatePart, Value as IrValue};
use serde_json::{json, Value};

use crate::events::{Artifact, EventSink, RunEvent, VerifyOutcome};
use crate::price::{parse_micros, Pricing, Spend};
use crate::provider::{CompletionRequest, ModelProvider, ModelSelection, ProviderError, Usage};
use crate::schema;
use crate::snapshot::{self, Resumption};
use crate::tools::{ApprovalMode, ApprovalRequest, ToolError, ToolHost, ToolInvocation};
use crate::{RunError, RunReport};

/// Hard ceiling on a single call whose answer arrives as one whole body.
///
/// Chosen to stay comfortably inside HTTP timeouts: a service that composes the
/// entire response before sending it holds the connection open for as long as
/// the answer takes, and several refuse a larger cap outright unless the
/// request streams.
const NON_STREAMING_CEILING: u32 = 16_000;

/// Hard ceiling on a single streamed call.
///
/// Higher because the objection above does not apply — text arrives as it is
/// produced, so nothing waits on the whole answer. Still a ceiling rather than
/// no limit: `max_tokens` above what a model accepts is a rejected request, and
/// a number the interpreter picked is easier to explain than a provider's 400.
const STREAMING_CEILING: u32 = 64_000;

pub struct RunOptions {
    /// Input name to value.
    pub inputs: BTreeMap<String, Value>,
    pub approval: ApprovalMode,
    /// Stop after this many steps even if the artifact allows more. A backstop
    /// for artifacts with no `steps` budget.
    pub max_steps: u32,
    /// Persistent memory this run starts from, usually loaded from the agent's
    /// store. Fields absent here start from the artifact's declared value, so
    /// an empty map is a correct first run rather than a missing one.
    pub memory: BTreeMap<String, Value>,
    /// Stop at the resumable checkpoint with this label, and report a snapshot.
    ///
    /// A label that names no checkpoint, or one that is not resumable, fails
    /// before the run starts. Running to completion without stopping would
    /// leave a caller unable to tell that from a run with no checkpoint.
    pub stop_at: Option<String>,
    /// Continue an interrupted run instead of starting one.
    ///
    /// The snapshot's inputs, bindings, state, outputs and counters replace
    /// this run's, and execution begins at the node it names.
    pub resume: Option<Resumption>,
    /// What each model costs, so `budget.cost` can be charged.
    ///
    /// Empty by default. A run with no prices charges nothing and reports every
    /// model it could not price, because
    /// [Runtime 0.1 §8](../../../specs/runtime/v0.1.md) says a backend that
    /// cannot price a request must not pretend to.
    pub pricing: Pricing,
}

impl Default for RunOptions {
    fn default() -> Self {
        RunOptions {
            inputs: BTreeMap::new(),
            approval: ApprovalMode::Deny,
            max_steps: 1_000,
            memory: BTreeMap::new(),
            stop_at: None,
            resume: None,
            pricing: Pricing::default(),
        }
    }
}

/// Other agents this run may call.
pub type AgentRegistry = BTreeMap<String, AgentIr>;

struct Interp<'a> {
    ir: &'a AgentIr,
    registry: &'a AgentRegistry,
    provider: &'a mut dyn ModelProvider,
    tools: &'a mut dyn ToolHost,
    sink: &'a mut dyn EventSink,
    approval: &'a mut ApprovalMode,

    bindings: BTreeMap<String, Value>,
    state: BTreeMap<String, Value>,
    /// Persistent memory, seeded before the first node and handed back in the
    /// report so the caller can write it to the agent's store.
    memory: BTreeMap<String, Value>,
    outputs: BTreeMap<String, Artifact>,

    steps: u32,
    max_steps: u32,
    usage: Usage,
    pricing: Pricing,
    spend: Spend,

    /// The checkpoint label this run stops at, if any.
    stop_at: Option<String>,
    /// The checkpoint it stopped at, as `(node, label)`.
    ///
    /// Once set, every enclosing region unwinds without running another node.
    /// It is only ever set at the top level -- a nested checkpoint is not
    /// resumable -- so the unwinding never abandons a partly finished loop.
    stopped: Option<(String, String)>,
    /// The inputs this run was given, kept so a snapshot can carry them.
    run_inputs: BTreeMap<String, Value>,
    /// Model and tool calls made, so a snapshot can say where a cassette got to.
    model_calls: u32,
    tool_calls: u32,
}

/// Execute an agent.
pub fn run(
    ir: &AgentIr,
    registry: &AgentRegistry,
    provider: &mut dyn ModelProvider,
    tools: &mut dyn ToolHost,
    sink: &mut dyn EventSink,
    options: RunOptions,
) -> Result<RunReport, RunError> {
    let RunOptions {
        inputs,
        mut approval,
        max_steps,
        memory,
        stop_at,
        resume,
        pricing,
    } = options;
    run_nested(
        ir,
        registry,
        provider,
        tools,
        sink,
        inputs,
        &mut approval,
        max_steps,
        memory,
        Interruption { stop_at, resume },
        &pricing,
    )
}

/// The body of a run, with the approval mode **borrowed** rather than owned.
///
/// A sub-agent has to be able to ask the same operator the parent would, and
/// the parent has to still be able to ask afterwards. Handing the mode down by
/// value did the first and broke the second: the handler was consumed by the
/// first sub-agent call and every later gate in the parent was denied without
/// anyone being asked.
#[allow(clippy::too_many_arguments)]
fn run_nested(
    ir: &AgentIr,
    registry: &AgentRegistry,
    provider: &mut dyn ModelProvider,
    tools: &mut dyn ToolHost,
    sink: &mut dyn EventSink,
    inputs: BTreeMap<String, Value>,
    approval: &mut ApprovalMode,
    step_ceiling: u32,
    // Whatever the caller loaded from the agent's store. A sub-agent gets none
    // of its caller's: persistent memory belongs to an agent, and a sub-agent
    // is a fresh run against its own artifact.
    stored: BTreeMap<String, Value>,
    // Where this run stops, and where it starts. A sub-agent gets neither: a
    // stop is a property of the run an operator asked for.
    interruption: Interruption,
    // Prices are a property of the deployment, not of an agent, so a sub-agent
    // is charged with the same ones its caller was.
    pricing: &Pricing,
) -> Result<RunReport, RunError> {
    check_ir_version(ir)?;

    // A resumption carries the inputs the first half ran with. Supplying them
    // again would let the two halves disagree about what the run was given, so
    // the snapshot's win outright and a caller that passed different ones is
    // told rather than quietly overridden.
    let inputs = match &interruption.resume {
        Some(snapshot) => {
            if !inputs.is_empty() && inputs != snapshot.inputs {
                return Err(RunError::InputsAfterResume);
            }
            snapshot.inputs.clone()
        }
        None => inputs,
    };

    let mut bindings = BTreeMap::new();
    for (name, declared_type) in &ir.inputs {
        let Some(value) = inputs.get(name) else {
            return Err(RunError::MissingInput {
                name: name.clone(),
                ty: declared_type.clone(),
            });
        };
        schema::validate(value, declared_type, &ir.types).map_err(|reason| {
            RunError::InvalidInput {
                name: name.clone(),
                reason,
            }
        })?;
        bindings.insert(name.clone(), value.clone());
    }
    for name in inputs.keys() {
        if !ir.inputs.contains_key(name) {
            return Err(RunError::UnknownInput {
                name: name.clone(),
                expected: ir.inputs.keys().cloned().collect(),
            });
        }
    }

    // Seed persistent memory before anything runs, so every declared field has
    // a value and a read can never find one missing. A stored value wins over
    // the declared one; a field the store does not carry starts from the
    // artifact. Validating here rather than at the first read means a store
    // holding the wrong shape stops the run before it spends anything.
    let mut memory = BTreeMap::new();
    for (name, field) in &ir.persistent {
        let value = stored
            .get(name)
            .cloned()
            .unwrap_or_else(|| field.initial.clone());
        schema::validate(&value, &field.ty, &ir.types).map_err(|reason| {
            RunError::InvalidMemory {
                field: name.clone(),
                reason,
            }
        })?;
        memory.insert(name.clone(), value);
    }
    for name in stored.keys() {
        if !ir.persistent.contains_key(name) {
            return Err(RunError::UnknownMemoryField {
                field: name.clone(),
                expected: ir.persistent.keys().cloned().collect(),
            });
        }
    }

    // Both halves of an interruption are settled before the run starts. A
    // `--stop-at` that silently never fires, or a resumption checked at the
    // node it lands on, would both spend tokens before saying no.
    let Interruption { stop_at, resume } = interruption;
    if let Some(label) = &stop_at {
        check_stop_label(ir, label)?;
    }
    if let Some(snapshot) = &resume {
        snapshot.check(ir).map_err(RunError::Snapshot)?;
    }

    // A resumption replaces what the first half established. Its inputs are the
    // ones that half ran with, so `inputs` above only validated a caller's
    // duplicate of them.
    let entry = match &resume {
        Some(snapshot) => {
            bindings = snapshot.bindings.clone();
            Some(snapshot.resume_at.clone())
        }
        None => ir.entry.clone(),
    };

    sink.emit(RunEvent::RunStarted {
        agent: ir.agent.clone(),
        provider: provider.name().to_string(),
    });

    let max_steps = match ir.budget.steps {
        Some(limit) if limit >= 0 => (limit as u32).min(step_ceiling),
        _ => step_ceiling,
    };

    let mut interp = Interp {
        ir,
        registry,
        provider,
        tools,
        sink,
        approval,
        bindings,
        // The counters carry across a stop: a budget bounds a run, and a run
        // that stopped and continued is one run. Resetting them here would make
        // `--stop-at` a way to spend twice what the artifact permits.
        state: resume
            .as_ref()
            .map(|snapshot| snapshot.state.clone())
            .unwrap_or_default(),
        memory,
        outputs: resume
            .as_ref()
            .map(|snapshot| snapshot.outputs.clone())
            .unwrap_or_default(),
        steps: resume.as_ref().map(|snapshot| snapshot.steps).unwrap_or(0),
        max_steps,
        usage: resume
            .as_ref()
            .map(|snapshot| snapshot.usage)
            .unwrap_or_default(),
        pricing: pricing.clone(),
        spend: resume
            .as_ref()
            .map(|snapshot| snapshot.spend.clone())
            .unwrap_or_default(),
        stop_at,
        stopped: None,
        run_inputs: match &resume {
            Some(snapshot) => snapshot.inputs.clone(),
            None => inputs,
        },
        model_calls: resume
            .as_ref()
            .map(|snapshot| snapshot.model_calls)
            .unwrap_or(0),
        tool_calls: resume
            .as_ref()
            .map(|snapshot| snapshot.tool_calls)
            .unwrap_or(0),
    };

    let result = interp.run_region(entry.as_deref());

    match result {
        Ok(()) => {
            let stopped = interp.take_snapshot();
            let report = RunReport {
                agent: ir.agent.clone(),
                outputs: interp.outputs,
                memory: interp.memory,
                stopped,
                usage: interp.usage,
                steps: interp.steps,
                spend: interp.spend.clone(),
            };
            if let Some(snapshot) = &report.stopped {
                sink.emit(RunEvent::RunStopped {
                    node: snapshot.stopped_at.clone(),
                    label: snapshot.label.clone(),
                });
                // No output check. A stopped run has not reached the artifact's
                // outputs and is not expected to have; `runStopped` is in the
                // record so a reader can see the check was suppressed rather
                // than having to infer it.
                return Ok(report);
            }
            sink.emit(RunEvent::RunFinished {
                steps: report.steps,
                usage: report.usage,
            });
            for name in ir.outputs.keys() {
                if !report.outputs.contains_key(name) {
                    return Err(RunError::OutputNotProduced { name: name.clone() });
                }
            }
            Ok(report)
        }
        Err(error) => {
            sink.emit(RunEvent::RunFailed {
                reason: error.to_string(),
            });
            Err(error)
        }
    }
}

/// Where a run stops and where it starts, kept together so a sub-agent can be
/// handed neither in one word.
#[derive(Default)]
struct Interruption {
    stop_at: Option<String>,
    resume: Option<Resumption>,
}

/// Refuse a `--stop-at` that would never fire.
///
/// Two different mistakes with two different answers: a label nobody wrote, and
/// a label on a checkpoint inside a branch arm or a loop body.
fn check_stop_label(ir: &AgentIr, label: &str) -> Result<(), RunError> {
    if snapshot::resumable_labels(ir).iter().any(|it| it == label) {
        return Ok(());
    }
    let nested = snapshot::all_checkpoint_labels(ir)
        .iter()
        .any(|it| it == label);
    Err(RunError::NotResumable {
        label: label.to_string(),
        nested,
        available: snapshot::resumable_labels(ir),
    })
}

fn check_ir_version(ir: &AgentIr) -> Result<(), RunError> {
    let major = ir.ir_version.split('.').next().unwrap_or_default();
    if major != "0" {
        return Err(RunError::UnsupportedIrVersion {
            found: ir.ir_version.clone(),
            supported: ingot_ir::IR_VERSION.to_string(),
        });
    }
    Ok(())
}

impl Interp<'_> {
    fn node(&self, id: &str) -> Result<&Node, RunError> {
        self.ir
            .node(id)
            .ok_or_else(|| RunError::MalformedIr(format!("node `{id}` does not exist")))
    }

    /// The snapshot this run stopped at, if it stopped.
    fn take_snapshot(&self) -> Option<Resumption> {
        let (node, label) = self.stopped.clone()?;
        // The node *after* the checkpoint, so a resumed run does not re-emit
        // the checkpoint's event. A checkpoint with no successor stopped at the
        // end of the flow, and there is nothing to continue into.
        let resume_at = self.ir.node(&node).and_then(|node| node.next.clone())?;
        Some(Resumption {
            ingot_snapshot: snapshot::SNAPSHOT_VERSION.to_string(),
            kind: snapshot::KIND.to_string(),
            agent: self.ir.agent.clone(),
            artifact: snapshot::artifact_digest(self.ir),
            label,
            stopped_at: node,
            resume_at,
            inputs: self.run_inputs.clone(),
            bindings: self.bindings.clone(),
            state: self.state.clone(),
            outputs: self.outputs.clone(),
            steps: self.steps,
            usage: self.usage,
            spend: self.spend.clone(),
            model_calls: self.model_calls,
            tool_calls: self.tool_calls,
        })
    }

    /// Walk a region from `entry` until a node has no successor.
    fn run_region(&mut self, entry: Option<&str>) -> Result<(), RunError> {
        let mut current = entry.map(str::to_string);
        while let Some(id) = current {
            if self.stopped.is_some() {
                return Ok(());
            }
            let node = self.node(&id)?.clone();
            self.sink.emit(RunEvent::NodeStarted {
                node: node.id.clone(),
                kind: node.kind.as_str().to_string(),
            });
            self.run_node(&node)?;
            current = node.next.clone();
        }
        Ok(())
    }

    fn run_node(&mut self, node: &Node) -> Result<(), RunError> {
        match node.kind {
            NodeKind::LlmCall => self.run_llm_call(node),
            NodeKind::ToolCall => self.run_tool_call(node),
            NodeKind::AgentCall => self.run_agent_call(node),
            NodeKind::Branch => self.run_branch(node),
            NodeKind::Parallel => self.run_parallel(node),
            NodeKind::Loop => self.run_loop(node),
            NodeKind::Approval => self.run_approval(node),
            NodeKind::Verify => self.run_verify(node),
            NodeKind::StateRead => self.run_state_read(node),
            NodeKind::StateWrite => self.run_state_write(node),
            NodeKind::ArtifactEmit => self.run_emit(node),
            NodeKind::Checkpoint => {
                let label = node.label.clone().unwrap_or_default();
                self.sink.emit(RunEvent::Checkpoint {
                    node: node.id.clone(),
                    label: label.clone(),
                });
                // After the event, so the checkpoint is in the first half's
                // record exactly where an uninterrupted run puts it. That is
                // what makes the two halves concatenate.
                if node.resumable && self.stop_at.as_deref() == Some(label.as_str()) {
                    self.stopped = Some((node.id.clone(), label));
                }
                Ok(())
            }
        }
    }

    // --- budgets ----------------------------------------------------------

    fn charge_step(&mut self, node: &Node) -> Result<(), RunError> {
        self.steps += 1;
        if self.steps > self.max_steps {
            return Err(RunError::BudgetExceeded {
                budget: "steps".to_string(),
                limit: self.max_steps.to_string(),
                node: node.id.clone(),
            });
        }
        Ok(())
    }

    /// Charge what a call cost, and stop when the artifact's ceiling is passed.
    ///
    /// A call that could not be priced is remembered rather than skipped: the
    /// total is only a total if nothing was missed, and enforcing a budget
    /// against a partial total would be the pretending
    /// [Runtime 0.1 §8](../../../specs/runtime/v0.1.md) forbids.
    fn charge_cost(&mut self, node: &Node, model: &str, usage: Usage) -> Result<(), RunError> {
        let Some(budget) = self.ir.budget.cost.clone() else {
            // No ceiling stated. Nothing to charge against, and pricing a run
            // nobody bounded would be arithmetic for its own sake.
            return Ok(());
        };
        self.spend
            .add(model, self.pricing.charge(model, usage, &budget.currency));

        let Some(limit) = parse_micros(&budget.amount) else {
            return Ok(());
        };
        if self.spend.is_complete() && self.spend.micros() > limit {
            return Err(RunError::BudgetExceeded {
                budget: "cost".to_string(),
                limit: format!("{} {}", budget.amount, budget.currency.to_ascii_uppercase()),
                node: node.id.clone(),
            });
        }
        Ok(())
    }

    fn charge_tokens(&mut self, node: &Node, usage: Usage) -> Result<(), RunError> {
        self.usage.add(usage);
        if let Some(limit) = self.ir.budget.tokens {
            if limit >= 0 && self.usage.total() > limit as u64 {
                return Err(RunError::BudgetExceeded {
                    budget: "tokens".to_string(),
                    limit: limit.to_string(),
                    node: node.id.clone(),
                });
            }
        }
        Ok(())
    }

    // --- policy -----------------------------------------------------------

    /// Re-check the artifact's policy before performing an effect.
    ///
    /// Duplicates the compile-time check on purpose: this protects whoever runs
    /// the artifact, who may never have seen its source.
    fn check_effects(&mut self, node: &Node, effects: &[String]) -> Result<bool, RunError> {
        let mut needs_approval = Vec::new();
        for effect in effects {
            if effect == "model_access" {
                continue;
            }
            let subject = subject_for_effect(effect);
            match self.ir.policy.get(subject) {
                Some(rule) => match rule.decision {
                    Decision::Allow => {}
                    Decision::RequireApproval => needs_approval.push(effect.clone()),
                    Decision::Deny => {
                        return Err(RunError::CapabilityDenied {
                            node: node.id.clone(),
                            effect: effect.clone(),
                            explicit: true,
                        })
                    }
                },
                None => {
                    return Err(RunError::CapabilityDenied {
                        node: node.id.clone(),
                        effect: effect.clone(),
                        explicit: false,
                    })
                }
            }
        }
        Ok(!needs_approval.is_empty())
    }

    // --- nodes ------------------------------------------------------------

    fn run_llm_call(&mut self, node: &Node) -> Result<(), RunError> {
        self.charge_step(node)?;
        // Counted before the call rather than after it. A cassette advances its
        // position on the attempt, so a run that stopped after a failed call
        // still has to resume past that interaction.
        self.model_calls += 1;

        let response_type = node
            .response_type
            .clone()
            .ok_or_else(|| RunError::MalformedIr(format!("`{}` has no responseType", node.id)))?;
        let shape = schema::response_shape(&response_type, &self.ir.types).map_err(|error| {
            RunError::UnsupportedResponseType {
                node: node.id.clone(),
                ty: error.ty,
                reason: error.reason,
            }
        })?;

        let prompt_value = node
            .prompt
            .as_ref()
            .ok_or_else(|| RunError::MalformedIr(format!("`{}` has no prompt", node.id)))?;
        let prompt = self.render_prompt(prompt_value)?;

        let mut system = None;
        let mut context = Vec::new();
        for argument in &node.args {
            let value = self.eval(&argument.value)?;
            match argument.name.as_str() {
                "system" => system = value.as_str().map(str::to_string),
                // `temperature` and `max_tokens` are model-tuning hints. They
                // are deliberately not forwarded: provider support varies, and
                // an artifact that silently behaves differently per provider is
                // exactly what portability is supposed to prevent.
                "temperature" | "max_tokens" => {}
                name => context.push((name.to_string(), value)),
            }
        }

        let request = CompletionRequest {
            node: node.id.clone(),
            model: self.model_selection(),
            system,
            prompt,
            context,
            response_type: response_type.clone(),
            shape,
            max_tokens: self.max_output_tokens(),
        };

        // The two channels are borrowed side by side here, and that is the
        // whole arrangement: text goes out live as it arrives, while the
        // decision about what the run does with it waits for the finished
        // response below.
        let node_id = node.id.clone();
        let mut shown = false;
        let attempt = {
            let sink = &mut *self.sink;
            self.provider.complete_streaming(&request, &mut |text| {
                shown = true;
                sink.delta(&node_id, text);
            })
        };

        // A partial answer is not an answer. Whatever a watcher saw is struck
        // rather than parsed, repaired or bound — including on a truncation,
        // where the text on screen is the beginning of a real answer and
        // therefore the most tempting thing in the system to keep.
        let response = match attempt {
            Ok(response) => response,
            Err(error) => {
                if shown {
                    self.sink.settled(&node_id, false);
                }
                return Err(RunError::Provider {
                    node: node.id.clone(),
                    source: error,
                });
            }
        };

        if let Err(reason) = schema::validate(&response.value, &response_type, &self.ir.types) {
            if shown {
                self.sink.settled(&node_id, false);
            }
            return Err(RunError::Provider {
                node: node.id.clone(),
                source: ProviderError::InvalidResponse(reason),
            });
        }
        if shown {
            self.sink.settled(&node_id, true);
        }

        self.sink.emit(RunEvent::ModelCall {
            node: node.id.clone(),
            model: response.model.clone(),
            response_type,
            usage: response.usage,
        });
        self.charge_tokens(node, response.usage)?;
        self.charge_cost(node, &response.model, response.usage)?;
        self.bind(node, response.value);
        Ok(())
    }

    fn run_tool_call(&mut self, node: &Node) -> Result<(), RunError> {
        self.tool_calls += 1;
        let reference = node
            .tool
            .clone()
            .ok_or_else(|| RunError::MalformedIr(format!("`{}` names no tool", node.id)))?;
        let binding = self
            .ir
            .tools
            .iter()
            .find(|tool| tool.reference == reference)
            .ok_or_else(|| {
                RunError::MalformedIr(format!(
                    "`{}` calls `{reference}`, which the artifact does not grant",
                    node.id
                ))
            })?
            .clone();

        self.check_effects(node, &node.effects)?;
        self.charge_step(node)?;

        if !self.tools.provides(&binding.name) {
            return Err(RunError::Tool {
                node: node.id.clone(),
                source: ToolError::NotAvailable(binding.name.clone()),
            });
        }

        let mut arguments = BTreeMap::new();
        for argument in &node.args {
            arguments.insert(argument.name.clone(), self.eval(&argument.value)?);
        }

        let invocation = ToolInvocation {
            node: node.id.clone(),
            agent: self.ir.agent.clone(),
            reference: binding.reference.clone(),
            name: binding.name.clone(),
            transport: binding.transport.clone(),
            arguments,
            effects: node.effects.clone(),
            result_type: binding.signature.result.clone(),
        };

        let result = self
            .tools
            .call(&invocation)
            .map_err(|source| RunError::Tool {
                node: node.id.clone(),
                source,
            })?;

        schema::validate(&result, &binding.signature.result, &self.ir.types).map_err(|reason| {
            RunError::Tool {
                node: node.id.clone(),
                source: ToolError::InvalidResult(reason),
            }
        })?;

        self.sink.emit(RunEvent::ToolCall {
            node: node.id.clone(),
            tool: binding.reference,
            effects: node.effects.clone(),
        });
        self.bind(node, result);
        Ok(())
    }

    fn run_agent_call(&mut self, node: &Node) -> Result<(), RunError> {
        let name = node
            .agent
            .clone()
            .ok_or_else(|| RunError::MalformedIr(format!("`{}` names no agent", node.id)))?;
        let sub = self
            .registry
            .get(&name)
            .ok_or_else(|| RunError::AgentNotAvailable {
                node: node.id.clone(),
                agent: name.clone(),
            })?;

        self.check_effects(node, &node.effects)?;
        self.charge_step(node)?;
        self.sink.emit(RunEvent::AgentCall {
            node: node.id.clone(),
            agent: name.clone(),
        });

        let mut inputs = BTreeMap::new();
        for argument in &node.args {
            inputs.insert(argument.name.clone(), self.eval(&argument.value)?);
        }

        // The sub-agent gets its own budget and its own policy. The parent
        // cannot widen either: this is a fresh run against the callee's own
        // artifact, not an inlined continuation of the caller's.
        //
        // The approval mode is the one thing that is shared rather than
        // duplicated. It is the operator, not the artifact, and there is only
        // one of them.
        let sub = sub.clone();
        let report = run_nested(
            &sub,
            self.registry,
            self.provider,
            self.tools,
            self.sink,
            inputs,
            &mut *self.approval,
            self.max_steps.saturating_sub(self.steps).max(1),
            // No store. A sub-agent's persistent memory is its own, and the
            // interpreter cannot open one anyway — a caller that wants a
            // sub-agent to keep memory runs it as an agent.
            BTreeMap::new(),
            // A sub-agent is never stopped at. A stop is a property of the run
            // an operator asked for, and half a sub-agent is not something the
            // caller could hold or continue.
            Interruption::default(),
            &self.pricing,
        )
        .map_err(|error| RunError::SubAgent {
            node: node.id.clone(),
            agent: name.clone(),
            source: Box::new(error),
        })?;

        self.steps += report.steps;
        self.charge_tokens(node, report.usage)?;

        let value = report
            .outputs
            .values()
            .next()
            .map(|artifact| artifact.value.clone())
            .unwrap_or(Value::Null);
        self.bind(node, value);
        Ok(())
    }

    fn run_branch(&mut self, node: &Node) -> Result<(), RunError> {
        let condition = node
            .condition
            .as_ref()
            .ok_or_else(|| RunError::MalformedIr(format!("`{}` has no condition", node.id)))?;
        let taken = self.eval(condition)?.as_bool().ok_or_else(|| {
            RunError::MalformedIr(format!(
                "`{}` condition did not evaluate to a boolean",
                node.id
            ))
        })?;

        self.sink.emit(RunEvent::BranchTaken {
            node: node.id.clone(),
            arm: if taken { "then".into() } else { "else".into() },
        });
        let arm = if taken {
            node.then.as_deref()
        } else {
            node.otherwise.as_deref()
        };
        self.run_region(arm)
    }

    /// Run a `parallel` body once per element, sequentially.
    ///
    /// See the module documentation: the compiler guarantees iterations are
    /// independent, so this produces the same result as concurrent execution.
    fn run_parallel(&mut self, node: &Node) -> Result<(), RunError> {
        let source = node
            .source
            .as_ref()
            .ok_or_else(|| RunError::MalformedIr(format!("`{}` has no source", node.id)))?;
        let binder = node
            .binder
            .clone()
            .ok_or_else(|| RunError::MalformedIr(format!("`{}` has no binder", node.id)))?;
        let items = self.eval(source)?;
        let items = items.as_array().cloned().ok_or_else(|| {
            RunError::MalformedIr(format!("`{}` source did not evaluate to a list", node.id))
        })?;

        let last_body_node = node
            .body
            .as_deref()
            .map(|entry| self.last_of_region(entry))
            .transpose()?
            .flatten();

        let total = items.len();
        let mut collected = Vec::with_capacity(total);
        let shadowed = self.bindings.remove(&binder);

        for (index, item) in items.into_iter().enumerate() {
            self.sink.emit(RunEvent::MapIteration {
                node: node.id.clone(),
                index,
                total,
            });
            self.bindings.insert(binder.clone(), item);
            self.run_region(node.body.as_deref())?;
            if self.stopped.is_some() {
                break;
            }

            // The value of an iteration is the result of the last node in the
            // body — the rule the IR specification states.
            //
            // A last node with no binding has no result, so an artifact that
            // ends a map body with one is malformed. It used to collect `null`
            // per element instead, which is a list of the right length and the
            // wrong contents: the failure surfaced wherever the list was
            // eventually used, a long way from the node that caused it.
            let value = match &last_body_node {
                Some(id) => {
                    let last = self.node(id)?.clone();
                    match &last.binding {
                        Some(name) => self.bindings.get(name).cloned().unwrap_or(Value::Null),
                        None => {
                            return Err(RunError::MalformedIr(format!(
                                "`{}` ends its body at `{id}`, which binds nothing, so an                                  iteration has no value to collect",
                                node.id
                            )))
                        }
                    }
                }
                None => Value::Null,
            };
            collected.push(value);
        }

        self.bindings.remove(&binder);
        if let Some(shadowed) = shadowed {
            self.bindings.insert(binder, shadowed);
        }
        self.bind(node, Value::Array(collected));
        Ok(())
    }

    fn run_loop(&mut self, node: &Node) -> Result<(), RunError> {
        // The static bound is enforced here too, so a guard that never
        // falsifies cannot produce an unbounded run.
        let max = node.max_iterations.unwrap_or(0).max(0) as u32;
        for iteration in 0..max {
            if let Some(guard) = &node.guard {
                let keep_going = self.eval(guard)?.as_bool().ok_or_else(|| {
                    RunError::MalformedIr(format!(
                        "`{}` guard did not evaluate to a boolean",
                        node.id
                    ))
                })?;
                if !keep_going {
                    break;
                }
            }
            self.sink.emit(RunEvent::LoopIteration {
                node: node.id.clone(),
                iteration: iteration + 1,
            });
            self.run_region(node.body.as_deref())?;
            // Unreachable today -- a checkpoint inside a loop is not resumable,
            // so nothing in a body can set this -- and here so that the day one
            // can, the loop does not run its remaining iterations first.
            if self.stopped.is_some() {
                break;
            }
        }
        Ok(())
    }

    fn run_approval(&mut self, node: &Node) -> Result<(), RunError> {
        let reason = node
            .label
            .clone()
            .unwrap_or_else(|| "approval required".to_string());
        self.sink.emit(RunEvent::ApprovalRequested {
            node: node.id.clone(),
            effects: node.effects.clone(),
            reason: reason.clone(),
        });

        let allowed = match self.approval {
            ApprovalMode::AssumeYes => true,
            ApprovalMode::Deny => false,
            ApprovalMode::Ask(handler) => handler.approve(&ApprovalRequest {
                node: node.id.clone(),
                effects: node.effects.clone(),
                reason: reason.clone(),
            }),
        };

        self.sink.emit(RunEvent::ApprovalDecided {
            node: node.id.clone(),
            allowed,
        });
        if allowed {
            Ok(())
        } else {
            Err(RunError::ApprovalDenied {
                node: node.id.clone(),
                reason,
            })
        }
    }

    fn run_verify(&mut self, node: &Node) -> Result<(), RunError> {
        let verifier = node
            .verifier
            .clone()
            .ok_or_else(|| RunError::MalformedIr(format!("`{}` names no verifier", node.id)))?;
        for argument in &node.args {
            self.eval(&argument.value)?;
        }

        // No `condition` means the source declared a verifier without a body:
        // the artifact names a check and carries no way to perform it. Saying
        // so is the whole point — `passed: true` here would be a pass nothing
        // earned.
        let Some(condition) = &node.condition else {
            self.sink.emit(RunEvent::Verified {
                node: node.id.clone(),
                verifier: verifier.clone(),
                outcome: VerifyOutcome::NotPerformed,
            });
            return Ok(());
        };

        let held = self.eval(condition)?.as_bool().ok_or_else(|| {
            RunError::MalformedIr(format!(
                "`{}` condition did not evaluate to a boolean",
                node.id
            ))
        })?;

        self.sink.emit(RunEvent::Verified {
            node: node.id.clone(),
            verifier: verifier.clone(),
            outcome: if held {
                VerifyOutcome::Passed
            } else {
                VerifyOutcome::Failed
            },
        });

        // The event is emitted first, so the record says what the check found
        // before it says the run ended. A failure ends the run rather than
        // letting it finish under a property that does not hold.
        if held {
            Ok(())
        } else {
            Err(RunError::VerificationFailed {
                node: node.id.clone(),
                verifier: verifier.clone(),
            })
        }
    }

    fn run_state_read(&mut self, node: &Node) -> Result<(), RunError> {
        let field = node
            .field
            .clone()
            .ok_or_else(|| RunError::MalformedIr(format!("`{}` names no state field", node.id)))?;
        let value = match node.scope {
            Some(RefScope::Memory) => self.memory.get(&field).cloned().ok_or_else(|| {
                RunError::MalformedIr(format!(
                    "`{}` reads `memory.{field}`, which is not declared",
                    node.id
                ))
            })?,
            _ => self
                .state
                .get(&field)
                .cloned()
                .ok_or_else(|| RunError::StateNotSet {
                    node: node.id.clone(),
                    field: field.clone(),
                })?,
        };
        self.bind(node, value);
        Ok(())
    }

    fn run_state_write(&mut self, node: &Node) -> Result<(), RunError> {
        let field = node
            .field
            .clone()
            .ok_or_else(|| RunError::MalformedIr(format!("`{}` names no state field", node.id)))?;
        let value = node
            .value
            .as_ref()
            .ok_or_else(|| RunError::MalformedIr(format!("`{}` has no value", node.id)))?;
        let value = self.eval(value)?;
        let persistent = matches!(node.scope, Some(RefScope::Memory));
        let declared = if persistent {
            self.ir.persistent.get(&field).map(|field| field.ty.clone())
        } else {
            self.ir.state.get(&field).cloned()
        };
        if let Some(declared) = declared {
            let root = if persistent { "memory" } else { "state" };
            schema::validate(&value, &declared, &self.ir.types).map_err(|reason| {
                RunError::TypeMismatch {
                    node: node.id.clone(),
                    what: format!("{root}.{field}"),
                    reason,
                }
            })?;
        }
        if persistent {
            self.memory.insert(field.clone(), value);
        } else {
            self.state.insert(field.clone(), value);
        }
        self.sink.emit(RunEvent::StateWritten {
            node: node.id.clone(),
            field,
        });
        Ok(())
    }

    fn run_emit(&mut self, node: &Node) -> Result<(), RunError> {
        let output = node
            .output
            .clone()
            .ok_or_else(|| RunError::MalformedIr(format!("`{}` names no output", node.id)))?;
        let value = node
            .value
            .as_ref()
            .ok_or_else(|| RunError::MalformedIr(format!("`{}` has no value", node.id)))?;
        let value = self.eval(value)?;

        let declared = self.ir.outputs.get(&output).cloned().ok_or_else(|| {
            RunError::MalformedIr(format!(
                "`{}` emits `{output}`, which is not declared",
                node.id
            ))
        })?;
        let content_type = declared
            .strip_prefix("artifact<")
            .and_then(|rest| rest.strip_suffix('>'))
            .unwrap_or(&declared)
            .to_string();
        schema::validate(&value, &content_type, &self.ir.types).map_err(|reason| {
            RunError::TypeMismatch {
                node: node.id.clone(),
                what: format!("output `{output}`"),
                reason,
            }
        })?;

        self.outputs.insert(
            output.clone(),
            Artifact {
                name: output.clone(),
                content_type,
                value,
            },
        );
        self.sink.emit(RunEvent::Emitted {
            node: node.id.clone(),
            output,
        });
        Ok(())
    }

    // --- helpers ----------------------------------------------------------

    fn bind(&mut self, node: &Node, value: Value) {
        if let Some(name) = &node.binding {
            self.bindings.insert(name.clone(), value);
        }
    }

    /// Id of the last node in a region, following `next` to the end.
    fn last_of_region(&self, entry: &str) -> Result<Option<String>, RunError> {
        let mut current = Some(entry.to_string());
        let mut last = None;
        let mut visited = 0usize;
        while let Some(id) = current {
            let node = self.node(&id)?;
            last = Some(node.id.clone());
            current = node.next.clone();
            visited += 1;
            if visited > self.ir.nodes.len() {
                return Err(RunError::MalformedIr(
                    "the node graph contains a cycle".to_string(),
                ));
            }
        }
        Ok(last)
    }

    fn model_selection(&self) -> ModelSelection {
        match &self.ir.requirements.model {
            ingot_ir::ModelRequirement::Exact { reference } => {
                ModelSelection::Exact(reference.clone())
            }
            ingot_ir::ModelRequirement::Capabilities {
                capabilities,
                context_tokens,
            } => ModelSelection::Capabilities {
                capabilities: capabilities.clone(),
                min_context_tokens: context_tokens.as_ref().map(|tokens| tokens.min),
            },
            ingot_ir::ModelRequirement::Unspecified => ModelSelection::Default,
        }
    }

    /// The cap on one call: what is left of the token budget, or the ceiling.
    ///
    /// Bounded from below by 1, because asking a provider for zero tokens is a
    /// request that cannot succeed, and from above by whichever ceiling the
    /// transport earns. An artifact does not choose this and cannot: the same
    /// artifact run against a streaming provider and a non-streaming one is the
    /// same program, and only the second has to keep the smaller number.
    fn max_output_tokens(&self) -> u32 {
        let ceiling = if self.provider.streams() {
            STREAMING_CEILING
        } else {
            NON_STREAMING_CEILING
        };
        let remaining = match self.ir.budget.tokens {
            Some(limit) if limit >= 0 => (limit as u64)
                .saturating_sub(self.usage.total())
                .min(u32::MAX as u64) as u32,
            _ => ceiling,
        };
        remaining.clamp(1, ceiling)
    }

    fn render_prompt(&mut self, value: &IrValue) -> Result<String, RunError> {
        match value {
            IrValue::Literal {
                value: Value::String(text),
                ..
            } => Ok(text.clone()),
            IrValue::Template { parts } => {
                let mut out = String::new();
                for part in parts {
                    match part {
                        TemplatePart::Text { value } => out.push_str(value),
                        TemplatePart::Value { value, ty } => {
                            let resolved = self.eval(value)?;
                            out.push_str(&render_value(&resolved, ty));
                        }
                    }
                }
                Ok(out)
            }
            other => {
                let resolved = self.eval(other)?;
                Ok(render_value(&resolved, "string"))
            }
        }
    }

    /// Evaluate a pure IR value.
    fn eval(&mut self, value: &IrValue) -> Result<Value, RunError> {
        match value {
            IrValue::Literal { value, .. } => Ok(value.clone()),
            IrValue::Ref { scope, path } => self.eval_ref(*scope, path),
            IrValue::List { items } => {
                let mut out = Vec::with_capacity(items.len());
                for item in items {
                    out.push(self.eval(item)?);
                }
                Ok(Value::Array(out))
            }
            IrValue::Template { .. } => Ok(Value::String(self.render_prompt(value)?)),
            IrValue::Unary { op, operand } => {
                let operand = self.eval(operand)?;
                match op.as_str() {
                    "!" => Ok(json!(!truthy(&operand))),
                    "-" => match operand.as_i64() {
                        Some(int) => Ok(json!(-int)),
                        None => Ok(json!(-operand.as_f64().unwrap_or_default())),
                    },
                    other => Err(RunError::MalformedIr(format!(
                        "unknown unary operator `{other}`"
                    ))),
                }
            }
            IrValue::Binary { op, lhs, rhs } => {
                let lhs = self.eval(lhs)?;
                let rhs = self.eval(rhs)?;
                eval_binary(op, &lhs, &rhs)
            }
            IrValue::Builtin { name, args } => {
                let mut evaluated = Vec::with_capacity(args.len());
                for arg in args {
                    evaluated.push(self.eval(arg)?);
                }
                eval_builtin(name, &evaluated)
            }
            IrValue::Unknown => Err(RunError::MalformedIr(
                "the artifact contains an unresolved value; it was not built from a clean compile"
                    .to_string(),
            )),
        }
    }

    fn eval_ref(&mut self, scope: RefScope, path: &[String]) -> Result<Value, RunError> {
        let Some((root, fields)) = path.split_first() else {
            return Err(RunError::MalformedIr(
                "a reference has an empty path".to_string(),
            ));
        };
        let mut current = match scope {
            RefScope::Input | RefScope::Binding => {
                self.bindings.get(root).cloned().ok_or_else(|| {
                    RunError::MalformedIr(format!("`{root}` is not bound at this point"))
                })?
            }
            RefScope::State => {
                self.state
                    .get(root)
                    .cloned()
                    .ok_or_else(|| RunError::StateNotSet {
                        node: String::new(),
                        field: root.clone(),
                    })?
            }
            // Never absent: every persistent field is seeded from its declared
            // initial value before the first node runs, which is the whole
            // reason that value is required.
            RefScope::Memory => self.memory.get(root).cloned().ok_or_else(|| {
                RunError::MalformedIr(format!("`memory.{root}` is not a declared field"))
            })?,
        };
        for field in fields {
            current = current
                .get(field)
                .cloned()
                .ok_or_else(|| RunError::MalformedIr(format!("no field `{field}` on the value")))?;
        }
        Ok(current)
    }
}

fn render_value(value: &Value, ty: &str) -> String {
    match value {
        // Substituting a string into a prompt should insert the text, not a
        // JSON-quoted copy of it.
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => match ty {
            "json" => serde_json::to_string_pretty(other).unwrap_or_else(|_| other.to_string()),
            _ => other.to_string(),
        },
    }
}

fn truthy(value: &Value) -> bool {
    value.as_bool().unwrap_or(false)
}

fn eval_binary(op: &str, lhs: &Value, rhs: &Value) -> Result<Value, RunError> {
    let result = match op {
        "==" => json!(lhs == rhs),
        "!=" => json!(lhs != rhs),
        "&&" => json!(truthy(lhs) && truthy(rhs)),
        "||" => json!(truthy(lhs) || truthy(rhs)),
        "<" | "<=" | ">" | ">=" => {
            let (a, b) = numeric_pair(lhs, rhs, op)?;
            json!(match op {
                "<" => a < b,
                "<=" => a <= b,
                ">" => a > b,
                _ => a >= b,
            })
        }
        "+" | "-" => {
            if let (Some(a), Some(b)) = (lhs.as_i64(), rhs.as_i64()) {
                json!(if op == "+" { a + b } else { a - b })
            } else {
                let (a, b) = numeric_pair(lhs, rhs, op)?;
                json!(if op == "+" { a + b } else { a - b })
            }
        }
        other => {
            return Err(RunError::MalformedIr(format!(
                "unknown binary operator `{other}`"
            )));
        }
    };
    Ok(result)
}

fn numeric_pair(lhs: &Value, rhs: &Value, op: &str) -> Result<(f64, f64), RunError> {
    match (lhs.as_f64(), rhs.as_f64()) {
        (Some(a), Some(b)) => Ok((a, b)),
        _ => Err(RunError::MalformedIr(format!(
            "`{op}` needs two numbers but was given {lhs} and {rhs}"
        ))),
    }
}

fn eval_builtin(name: &str, args: &[Value]) -> Result<Value, RunError> {
    match name {
        "len" => {
            let Some(value) = args.first() else {
                return Err(RunError::MalformedIr(
                    "`len` takes one argument".to_string(),
                ));
            };
            let length = match value {
                Value::Array(items) => items.len(),
                Value::String(text) => text.chars().count(),
                Value::Object(map) => map.len(),
                other => {
                    return Err(RunError::MalformedIr(format!(
                        "`len` cannot measure {other}"
                    )))
                }
            };
            Ok(json!(length))
        }
        other => Err(RunError::MalformedIr(format!("unknown builtin `{other}`"))),
    }
}

/// The policy subject that governs an effect.
///
/// Mirrors `PolicySubject::for_effect` in `ingot-types`, but reads from the
/// artifact's own vocabulary so the runtime stays independent of the compiler.
fn subject_for_effect(effect: &str) -> &str {
    match effect {
        "secret_access" => "secrets",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_access_maps_to_the_secrets_subject() {
        assert_eq!(subject_for_effect("secret_access"), "secrets");
        assert_eq!(subject_for_effect("network"), "network");
    }

    #[test]
    fn len_measures_lists_strings_and_objects() {
        assert_eq!(eval_builtin("len", &[json!([1, 2, 3])]).unwrap(), json!(3));
        assert_eq!(eval_builtin("len", &[json!("abc")]).unwrap(), json!(3));
        assert_eq!(eval_builtin("len", &[json!({"a": 1})]).unwrap(), json!(1));
    }

    #[test]
    fn comparison_operators_work_on_numbers() {
        assert_eq!(eval_binary(">", &json!(3), &json!(1)).unwrap(), json!(true));
        assert_eq!(
            eval_binary("<=", &json!(3), &json!(3)).unwrap(),
            json!(true)
        );
    }

    #[test]
    fn equality_works_on_any_value() {
        assert_eq!(
            eval_binary("==", &json!("a"), &json!("a")).unwrap(),
            json!(true)
        );
        assert_eq!(
            eval_binary("!=", &json!([1]), &json!([2])).unwrap(),
            json!(true)
        );
    }

    #[test]
    fn integer_arithmetic_stays_integral() {
        assert_eq!(eval_binary("+", &json!(2), &json!(3)).unwrap(), json!(5));
    }

    #[test]
    fn strings_render_without_json_quoting() {
        assert_eq!(render_value(&json!("hello"), "string"), "hello");
        assert_eq!(render_value(&json!(3), "int"), "3");
    }
}
