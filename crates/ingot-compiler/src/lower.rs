//! Lowering: validated syntax to Agent IR.
//!
//! Lowering makes implicit things explicit and never re-decides anything the
//! checker already decided. Concretely it:
//!
//! * flattens the flow into one node array with `next` pointers,
//! * hoists nested calls into their own nodes bound to synthetic names, so a
//!   node's arguments are always pure values,
//! * turns every read of working memory into a `state.read` node, so state
//!   access is auditable in the artifact,
//! * inlines pure bindings, which keeps the IR to nodes that actually do work,
//! * inserts an `approval` node ahead of any call whose effects the policy
//!   marks `require approval`.
//!
//! Node ids are assigned in creation order, and creation order follows source
//! order, so the same source always lowers to the same ids.

use std::collections::{BTreeMap, HashMap};

use ingot_ir::{
    node::Argument, AgentIr, Budget, ContextTokens, Cost, Decision, FieldType, ModelRequirement,
    Node, NodeKind, PersistentField, PolicyRule, RecordType, RefScope, Requirements, SourceSpan,
    TemplatePart, ToolBinding, ToolSignature, Value, IR_VERSION,
};
use ingot_semantic::{AgentInfo, Analysis, CallTarget, ModelInfo};
use ingot_source::{FileId, SourceMap, Span};
use ingot_syntax::*;
use ingot_types::{PolicyDecision, Ty};

/// Lower one checked agent to IR.
pub fn lower_agent(
    program: &Program,
    analysis: &Analysis,
    agent: &AgentInfo,
    sources: &SourceMap,
    root_file: FileId,
) -> AgentIr {
    let decl = &program.agents[agent.decl_index];
    let mut lowerer = Lowerer {
        program,
        analysis,
        agent,
        sources,
        root_file,
        nodes: Vec::new(),
        counter: 0,
        temp_counter: 0,
        aliases: HashMap::new(),
        region_depth: 0,
        state_reads: HashMap::new(),
    };

    let level = match &decl.flow {
        Some(flow) => lowerer.lower_statements(&flow.statements),
        None => Vec::new(),
    };
    lowerer.link(&level);
    let entry = level.first().map(|index| lowerer.nodes[*index].id.clone());

    let language = program
        .language
        .map(|version| version.text())
        .unwrap_or_else(|| "0.1".to_string());

    AgentIr {
        ir_version: IR_VERSION.to_string(),
        language,
        agent: agent.qualified_name.clone(),
        doc: decl.doc.clone(),
        inputs: agent
            .inputs
            .iter()
            .map(|param| (param.name.clone(), param.ty.to_string()))
            .collect(),
        outputs: agent
            .output
            .iter()
            .map(|output| (output.name.clone(), format!("artifact<{}>", output.content)))
            .collect(),
        types: lower_types(analysis),
        requirements: Requirements {
            model: lower_model(&agent.model),
        },
        tools: lower_tools(analysis, agent),
        state: agent
            .state
            .iter()
            .map(|param| (param.name.clone(), param.ty.to_string()))
            .collect(),
        persistent: lower_persistent(agent, decl),
        budget: Budget {
            steps: agent.budget.steps,
            tokens: agent.budget.tokens,
            cost: agent
                .budget
                .cost
                .as_ref()
                .map(|cost| Cost::new(cost.amount, cost.currency.clone())),
        },
        policy: agent
            .policy
            .iter()
            .map(|(subject, rule)| {
                (
                    subject.as_str().to_string(),
                    PolicyRule {
                        decision: match rule.decision {
                            PolicyDecision::Allow => Decision::Allow,
                            PolicyDecision::RequireApproval => Decision::RequireApproval,
                            // Unspecified never reaches the IR: the checker
                            // rejects any call that would depend on it.
                            PolicyDecision::Deny | PolicyDecision::Unspecified => Decision::Deny,
                        },
                        values: rule.values.clone(),
                        qualifier: rule.qualifier.clone(),
                    },
                )
            })
            .collect(),
        effects: agent.effects.names(),
        entry,
        nodes: lowerer.nodes,
    }
}

/// Persistent fields, paired with the literal each starts from.
///
/// The checker validated both the type and the literal-ness; this reads the
/// value back out of the syntax tree, which is where a literal lives. A field
/// the checker rejected is absent from `agent.persistent` and so never reaches
/// here.
fn lower_persistent(agent: &AgentInfo, decl: &AgentDecl) -> BTreeMap<String, PersistentField> {
    let declared = decl
        .memory
        .as_ref()
        .and_then(|memory| memory.persistent.as_ref());
    let Some(block) = declared else {
        return BTreeMap::new();
    };

    agent
        .persistent
        .iter()
        .filter_map(|param| {
            let field = block
                .fields
                .iter()
                .find(|field| field.name.text == param.name)?;
            let initial = literal_json(field.initial.as_ref()?)?;
            Some((
                param.name.clone(),
                PersistentField {
                    ty: param.ty.to_string(),
                    initial,
                },
            ))
        })
        .collect()
}

/// A literal expression as canonical JSON.
///
/// Mirrors the checker's `literal_ty`: what one accepts the other encodes, and
/// anything else was already refused with a diagnostic.
fn literal_json(expr: &Expr) -> Option<serde_json::Value> {
    match expr {
        Expr::Str(literal) if literal.is_plain() => {
            Some(serde_json::Value::String(literal.plain_text()))
        }
        Expr::Int { value, .. } => Some(serde_json::Value::from(*value)),
        Expr::Float { value, .. } => serde_json::Number::from_f64(*value).map(Into::into),
        Expr::Bool { value, .. } => Some(serde_json::Value::Bool(*value)),
        Expr::List { items, .. } => items
            .iter()
            .map(literal_json)
            .collect::<Option<Vec<_>>>()
            .map(serde_json::Value::Array),
        _ => None,
    }
}

fn lower_types(analysis: &Analysis) -> BTreeMap<String, RecordType> {
    analysis
        .records
        .iter()
        .map(|(name, record)| {
            (
                name.clone(),
                RecordType {
                    fields: record
                        .fields
                        .iter()
                        .map(|field| FieldType {
                            name: field.name.clone(),
                            ty: field.ty.to_string(),
                        })
                        .collect(),
                },
            )
        })
        .collect()
}

fn lower_model(model: &ModelInfo) -> ModelRequirement {
    match model {
        ModelInfo::Requires {
            capabilities,
            context_tokens,
        } => ModelRequirement::Capabilities {
            capabilities: capabilities.clone(),
            context_tokens: context_tokens.map(|min| ContextTokens { min }),
        },
        ModelInfo::Exact { reference } => ModelRequirement::Exact {
            reference: reference.clone(),
        },
        ModelInfo::Unspecified => ModelRequirement::Unspecified,
    }
}

fn lower_tools(analysis: &Analysis, agent: &AgentInfo) -> Vec<ToolBinding> {
    agent
        .grants
        .iter()
        .filter_map(|grant| {
            let tool = analysis.tools.get(&grant.tool)?;
            Some(ToolBinding {
                reference: format!("{}:{}", grant.transport, grant.tool),
                name: grant.tool.clone(),
                transport: grant.transport.clone(),
                effects: tool.effects.names(),
                scopes: {
                    let mut scopes: BTreeMap<String, Vec<String>> = BTreeMap::new();
                    for declared in &tool.reach {
                        scopes
                            .entry(declared.effect.as_str().to_string())
                            .or_default()
                            .push(declared.value.clone());
                    }
                    scopes
                },
                signature: ToolSignature {
                    params: tool
                        .params
                        .iter()
                        .map(|param| FieldType {
                            name: param.name.clone(),
                            ty: param.ty.to_string(),
                        })
                        .collect(),
                    result: tool.result.to_string(),
                },
            })
        })
        .collect()
}

struct Lowerer<'a> {
    program: &'a Program,
    analysis: &'a Analysis,
    agent: &'a AgentInfo,
    sources: &'a SourceMap,
    root_file: FileId,
    nodes: Vec<Node>,
    counter: usize,
    temp_counter: usize,
    /// Pure bindings, inlined at their use sites instead of becoming nodes.
    aliases: HashMap<String, Value>,
    /// How many nested regions -- branch arms, loop and map bodies -- enclose
    /// the statement being lowered. Zero is the top level of the flow, which is
    /// the only place a `checkpoint` is resumable.
    region_depth: usize,
    /// Store fields already read in the statement being lowered, keyed by the
    /// qualified name (`state.notes`, `memory.seen`) so the two stores cannot
    /// share a read.
    state_reads: HashMap<String, String>,
}

impl<'a> Lowerer<'a> {
    fn next_id(&mut self) -> String {
        let id = format!("n{}", self.counter);
        self.counter += 1;
        id
    }

    fn next_temp(&mut self) -> String {
        let name = format!("$tmp{}", self.temp_counter);
        self.temp_counter += 1;
        name
    }

    fn source_span(&self, span: Span) -> SourceSpan {
        SourceSpan {
            source: self.sources.portable_name(span.file, self.root_file),
            start: span.start,
            end: span.end,
        }
    }

    /// Append a node to the current region, inserting an approval gate first if
    /// the policy requires one for the node's effects.
    fn push(&mut self, level: &mut Vec<usize>, mut node: Node) -> usize {
        let gated: Vec<String> = node
            .effects
            .iter()
            .filter(|effect| {
                ingot_types::Effect::from_name(effect)
                    .map(|effect| {
                        self.agent.decision_for(effect) == PolicyDecision::RequireApproval
                    })
                    .unwrap_or(false)
            })
            .cloned()
            .collect();

        if !gated.is_empty() {
            let mut approval = Node::new(self.next_id(), NodeKind::Approval);
            approval.source_span = node.source_span.clone();
            approval.label = Some(match (&node.tool, &node.agent) {
                (Some(tool), _) => format!("approval required before calling {tool}"),
                (_, Some(agent)) => format!("approval required before calling {agent}"),
                _ => "approval required".to_string(),
            });
            approval.effects = gated;
            let index = self.nodes.len();
            self.nodes.push(approval);
            level.push(index);
        }

        node.id = self.next_id();
        let index = self.nodes.len();
        self.nodes.push(node);
        level.push(index);
        index
    }

    /// Chain a region's nodes with `next`, terminating the last one.
    fn link(&mut self, level: &[usize]) {
        for window in level.windows(2) {
            let next_id = self.nodes[window[1]].id.clone();
            self.nodes[window[0]].next = Some(next_id);
        }
        if let Some(last) = level.last() {
            self.nodes[*last].next = None;
        }
    }

    /// Lower a nested region, so a `checkpoint` inside it is not resumable.
    fn lower_region(&mut self, statements: &[Stmt]) -> Vec<usize> {
        self.region_depth += 1;
        let level = self.lower_statements(statements);
        self.region_depth -= 1;
        level
    }

    fn lower_statements(&mut self, statements: &[Stmt]) -> Vec<usize> {
        let mut level = Vec::new();
        for statement in statements {
            self.state_reads.clear();
            self.lower_statement(&mut level, statement);
        }
        level
    }

    fn lower_statement(&mut self, level: &mut Vec<usize>, statement: &Stmt) {
        match statement {
            Stmt::Bind { name, value, .. } => {
                if produces_node(value) {
                    let node = self.lower_call_like(level, value, Some(name.text.clone()));
                    if let Some(node) = node {
                        self.push(level, node);
                    }
                } else {
                    // Pure: record it and inline at use sites.
                    let lowered = self.lower_value(level, value);
                    self.aliases.insert(name.text.clone(), lowered);
                }
            }
            Stmt::StateWrite {
                scope,
                field,
                value,
                span,
            } => {
                let lowered = self.lower_value(level, value);
                let mut node = Node::new(String::new(), NodeKind::StateWrite);
                node.source_span = Some(self.source_span(*span));
                node.field = Some(field.text.clone());
                node.scope = ref_scope(*scope);
                node.value = Some(lowered);
                self.push(level, node);
            }
            Stmt::Expr { value, .. } => {
                if let Some(node) = self.lower_call_like(level, value, None) {
                    self.push(level, node);
                }
            }
            Stmt::Verify {
                validator,
                args,
                span,
            } => {
                let Some(info) = self.analysis.verifies.get(span).cloned() else {
                    return;
                };
                let Some(verifier) = self.analysis.verifiers.get(&info.verifier).cloned() else {
                    return;
                };
                let arguments =
                    self.lower_arguments(level, args, &info.arg_order, &verifier.params);
                let condition = self.lower_verifier_body(level, &verifier, &arguments);
                let mut node = Node::new(String::new(), NodeKind::Verify);
                node.source_span = Some(self.source_span(*span));
                node.verifier = Some(validator.text.clone());
                node.args = arguments;
                node.condition = condition;
                self.push(level, node);
            }
            Stmt::Emit {
                output,
                value,
                span,
            } => {
                let lowered = self.lower_value(level, value);
                let mut node = Node::new(String::new(), NodeKind::ArtifactEmit);
                node.source_span = Some(self.source_span(*span));
                node.output = Some(output.text.clone());
                node.value = Some(lowered);
                self.push(level, node);
            }
            Stmt::If {
                condition,
                then_branch,
                else_branch,
                span,
            } => {
                let lowered = self.lower_value(level, condition);
                let mut node = Node::new(String::new(), NodeKind::Branch);
                node.source_span = Some(self.source_span(*span));
                node.condition = Some(lowered);
                let index = self.push(level, node);

                let then_level = self.lower_region(then_branch);
                self.link(&then_level);
                let then_entry = then_level.first().map(|i| self.nodes[*i].id.clone());

                let else_entry = match else_branch {
                    Some(else_branch) => {
                        let else_level = self.lower_region(else_branch);
                        self.link(&else_level);
                        else_level.first().map(|i| self.nodes[*i].id.clone())
                    }
                    None => None,
                };

                self.nodes[index].then = then_entry;
                self.nodes[index].otherwise = else_entry;
            }
            Stmt::Loop {
                max,
                guard,
                body,
                span,
            } => {
                let lowered_guard = guard.as_ref().map(|guard| self.lower_value(level, guard));
                let mut node = Node::new(String::new(), NodeKind::Loop);
                node.source_span = Some(self.source_span(*span));
                node.max_iterations = max.map(|bound| bound.value);
                node.guard = lowered_guard;
                let index = self.push(level, node);

                let mut body_level = self.lower_region(body);
                // A guard is evaluated before **every** iteration, and the read
                // above happens once, before the loop. Re-read each field the
                // guard names at the end of the body, binding the same name, so
                // the next evaluation sees what the body wrote. Without this a
                // guard over working memory never changes and only `max` ever
                // stops the loop.
                self.refresh_guard_reads(&mut body_level, guard.as_ref(), *span);
                self.link(&body_level);
                self.nodes[index].body = body_level.first().map(|i| self.nodes[*i].id.clone());
            }
            Stmt::Checkpoint { label, span } => {
                let mut node = Node::new(String::new(), NodeKind::Checkpoint);
                node.source_span = Some(self.source_span(*span));
                node.label = Some(label.plain_text());
                // Only at the top level. A checkpoint inside a branch arm or a
                // loop body is reached with a partially unwound interpreter,
                // and resuming into one would mean serialising a continuation.
                node.resumable = self.region_depth == 0;
                self.push(level, node);
            }
            Stmt::Error { .. } => {}
        }
    }

    /// Build the node for an `ask`, `consult`, `call` or `parallel map` expression.
    fn lower_call_like(
        &mut self,
        level: &mut Vec<usize>,
        expr: &Expr,
        binding: Option<String>,
    ) -> Option<Node> {
        match expr {
            Expr::Ask { result, args, span } => {
                let mut node = Node::new(String::new(), NodeKind::LlmCall);
                node.source_span = Some(self.source_span(*span));
                node.binding = binding;
                node.response_type = Some(result.text());
                node.effects = vec!["model_access".to_string()];

                let mut named: Vec<Argument> = Vec::new();
                for arg in args {
                    let lowered = self.lower_value(level, &arg.value);
                    match &arg.name {
                        None => node.prompt = Some(lowered),
                        Some(name) => named.push(Argument {
                            name: name.text.clone(),
                            value: lowered,
                        }),
                    }
                }
                named.sort_by(|a, b| a.name.cmp(&b.name));
                node.args = named;
                Some(node)
            }
            Expr::Consult { args, span } => {
                let mut node = Node::new(String::new(), NodeKind::Consult);
                node.source_span = Some(self.source_span(*span));
                node.binding = binding;
                // A person types text or picks from a list, so the answer is
                // always a string. Recorded on the node all the same, because a
                // runner reads the type from the artifact rather than knowing it.
                node.response_type = Some("string".to_string());
                node.effects = vec!["human".to_string()];

                let mut named: Vec<Argument> = Vec::new();
                for arg in args {
                    let lowered = self.lower_value(level, &arg.value);
                    match &arg.name {
                        None => node.prompt = Some(lowered),
                        Some(name) => named.push(Argument {
                            name: name.text.clone(),
                            value: lowered,
                        }),
                    }
                }
                named.sort_by(|a, b| a.name.cmp(&b.name));
                node.args = named;
                Some(node)
            }
            Expr::Call { callee, args, span } => {
                let info = self.analysis.call(*span)?.clone();
                let (params, mut node) = match &info.target {
                    CallTarget::Tool(name) => {
                        let tool = self.analysis.tools.get(name)?.clone();
                        let mut node = Node::new(String::new(), NodeKind::ToolCall);
                        let transport = self
                            .agent
                            .grants
                            .iter()
                            .find(|grant| &grant.tool == name)
                            .map(|grant| grant.transport.clone())
                            .unwrap_or_else(|| "mcp".to_string());
                        node.tool = Some(format!("{transport}:{name}"));
                        (tool.params, node)
                    }
                    CallTarget::Agent(name) => {
                        let target = self.analysis.agent(name)?;
                        let mut node = Node::new(String::new(), NodeKind::AgentCall);
                        node.agent = Some(target.qualified_name.clone());
                        (target.inputs.clone(), node)
                    }
                };
                node.source_span = Some(self.source_span(*span));
                node.binding = binding;
                node.effects = info.effects.names();
                node.args = self.lower_arguments(level, args, &info.arg_order, &params);
                let _ = callee;
                Some(node)
            }
            Expr::ParallelMap {
                source,
                binder,
                body,
                span,
            } => {
                let lowered_source = self.lower_value(level, source);
                let mut node = Node::new(String::new(), NodeKind::Parallel);
                node.source_span = Some(self.source_span(*span));
                node.binding = binding;
                node.mode = Some("map".to_string());
                node.binder = Some(binder.text.clone());
                node.source = Some(lowered_source);

                // The body is lowered into the shared node array; the container
                // points at its first node and the region self-terminates.
                let body_level = self.lower_region(body);
                self.link(&body_level);
                node.body = body_level.first().map(|i| self.nodes[*i].id.clone());

                // An iteration's value is the result of the last node in the
                // body (Runtime 0.1 §5), and a backend reads that result from
                // the node's binding. A body written as a bare expression --
                // `call web.search(query)`, which is the idiom -- produced a
                // node with no binding, so every iteration collected null.
                // Naming it here is what makes the value reachable.
                if let Some(last) = body_level.last() {
                    if self.nodes[*last].binding.is_none() {
                        let name = self.next_temp();
                        self.nodes[*last].binding = Some(name);
                    }
                }
                Some(node)
            }
            _ => None,
        }
    }

    fn lower_arguments(
        &mut self,
        level: &mut Vec<usize>,
        args: &[Arg],
        order: &[Option<usize>],
        params: &[ingot_semantic::Param],
    ) -> Vec<Argument> {
        let mut arguments = Vec::new();
        for (position, slot) in order.iter().enumerate() {
            let Some(index) = slot else { continue };
            let Some(arg) = args.get(*index) else {
                continue;
            };
            let Some(param) = params.get(position) else {
                continue;
            };
            let value = self.lower_value(level, &arg.value);
            arguments.push(Argument {
                name: param.name.clone(),
                value,
            });
        }
        arguments
    }

    /// Lower an expression to a pure value, hoisting any calls it contains.
    fn lower_value(&mut self, level: &mut Vec<usize>, expr: &Expr) -> Value {
        if produces_node(expr) {
            let temp = self.next_temp();
            if let Some(node) = self.lower_call_like(level, expr, Some(temp.clone())) {
                self.push(level, node);
                return Value::Ref {
                    scope: RefScope::Binding,
                    path: vec![temp],
                };
            }
            return Value::Unknown;
        }

        match expr {
            Expr::Str(literal) => self.lower_string(level, literal),
            Expr::Int { value, .. } => Value::int(*value),
            Expr::Float { value, .. } => Value::Literal {
                ty: "float".to_string(),
                value: serde_json::Number::from_f64(*value)
                    .map(serde_json::Value::Number)
                    .unwrap_or(serde_json::Value::Null),
            },
            Expr::Bool { value, .. } => Value::bool(*value),
            Expr::List { items, .. } => Value::List {
                items: items
                    .iter()
                    .map(|item| self.lower_value(level, item))
                    .collect(),
            },
            Expr::Path(path) => self.lower_path(level, path),
            Expr::Builtin { name, args, .. } => Value::Builtin {
                name: name.text.clone(),
                args: args
                    .iter()
                    .map(|arg| self.lower_value(level, arg))
                    .collect(),
            },
            Expr::FunctionCall { args, span, .. } => self.lower_function_call(level, args, *span),
            Expr::Unary { op, operand, .. } => Value::Unary {
                op: op.symbol().to_string(),
                operand: Box::new(self.lower_value(level, operand)),
            },
            Expr::Binary { op, lhs, rhs, .. } => Value::Binary {
                op: op.symbol().to_string(),
                lhs: Box::new(self.lower_value(level, lhs)),
                rhs: Box::new(self.lower_value(level, rhs)),
            },
            Expr::Error { .. } => Value::Unknown,
            // Handled above by `produces_node`.
            Expr::Ask { .. }
            | Expr::Consult { .. }
            | Expr::Call { .. }
            | Expr::ParallelMap { .. } => Value::Unknown,
        }
    }

    fn lower_string(&mut self, level: &mut Vec<usize>, literal: &StringLit) -> Value {
        if literal.is_plain() {
            return Value::string(literal.plain_text());
        }
        let mut parts = Vec::new();
        for part in &literal.parts {
            match part {
                StringPart::Literal(text) => {
                    if !text.is_empty() {
                        parts.push(TemplatePart::Text {
                            value: text.clone(),
                        });
                    }
                }
                StringPart::Interpolation(path) => {
                    let ty = self
                        .analysis
                        .interpolations
                        .get(&path.span)
                        .cloned()
                        .unwrap_or(Ty::Unknown);
                    let value = self.lower_reference(level, &path.root, &path.segments, path.span);
                    parts.push(TemplatePart::Value {
                        value,
                        ty: ty.to_string(),
                    });
                }
            }
        }
        Value::Template { parts }
    }

    fn lower_function_call(&mut self, level: &mut Vec<usize>, args: &[Arg], span: Span) -> Value {
        let Some(call) = self.analysis.function_call(span).cloned() else {
            return Value::Unknown;
        };
        let Some(function) = self.analysis.functions.get(&call.function) else {
            return Value::Unknown;
        };
        let Some(decl) = self.program.functions.get(function.decl_index) else {
            return Value::Unknown;
        };

        let mut saved = Vec::new();
        for (position, slot) in call.arg_order.iter().enumerate() {
            let Some(index) = slot else { continue };
            let Some(arg) = args.get(*index) else {
                continue;
            };
            let Some(param) = function.params.get(position) else {
                continue;
            };
            let value = self.lower_value(level, &arg.value);
            saved.push((
                param.name.clone(),
                self.aliases.insert(param.name.clone(), value),
            ));
        }

        let lowered = self.lower_value(level, &decl.body);

        for (name, previous) in saved.into_iter().rev() {
            match previous {
                Some(value) => {
                    self.aliases.insert(name, value);
                }
                None => {
                    self.aliases.remove(&name);
                }
            }
        }

        lowered
    }

    /// Inline a verifier's body at a `verify` site, as the node's `condition`.
    ///
    /// The arguments are already lowered, and each carries the name of the
    /// parameter it fills, so they are exactly the substitution map the body
    /// needs. This is `lower_function_call`'s substitution with the lowering
    /// of the arguments already done.
    ///
    /// A verifier with no body yields `None`, which is what a backend reads as
    /// `notPerformed`.
    fn lower_verifier_body(
        &mut self,
        level: &mut Vec<usize>,
        verifier: &ingot_semantic::VerifierInfo,
        arguments: &[Argument],
    ) -> Option<Value> {
        let body = self
            .program
            .verifiers
            .get(verifier.decl_index)?
            .body
            .clone()?;

        let mut saved = Vec::new();
        for argument in arguments {
            let previous = self
                .aliases
                .insert(argument.name.clone(), argument.value.clone());
            saved.push((argument.name.clone(), previous));
        }

        let lowered = self.lower_value(level, &body);

        for (name, previous) in saved.into_iter().rev() {
            match previous {
                Some(value) => {
                    self.aliases.insert(name, value);
                }
                None => {
                    self.aliases.remove(&name);
                }
            }
        }

        Some(lowered)
    }

    fn lower_path(&mut self, level: &mut Vec<usize>, path: &PathExpr) -> Value {
        self.lower_reference(level, &path.root, &path.segments, path.span)
    }

    /// Resolve a name into a pure value.
    ///
    /// Inlines pure bindings, and turns a read of working memory into an
    /// explicit `state.read` node whose synthetic binding the value refers to.
    fn lower_reference(
        &mut self,
        level: &mut Vec<usize>,
        root: &PathRoot,
        segments: &[Ident],
        span: Span,
    ) -> Value {
        let field_names = || segments.iter().map(|segment| segment.text.clone());

        match root {
            PathRoot::State { .. } | PathRoot::Memory { .. } => {
                let Some(field) = segments.first() else {
                    return Value::Unknown;
                };
                let scope = root.scope().expect("both arms address a store");
                let binding = self.read_state(level, scope, &field.text, span);
                let mut path = vec![binding];
                path.extend(segments[1..].iter().map(|segment| segment.text.clone()));
                Value::Ref {
                    scope: RefScope::Binding,
                    path,
                }
            }
            PathRoot::Binding(ident) => {
                if let Some(alias) = self.aliases.get(&ident.text).cloned() {
                    return match alias {
                        Value::Ref {
                            scope,
                            path: mut base,
                        } => {
                            base.extend(field_names());
                            Value::Ref { scope, path: base }
                        }
                        other if segments.is_empty() => other,
                        // Only record-typed values have fields, and in v0.1 a
                        // pure record value is always a reference, so this is
                        // unreachable for a program that passed the checker.
                        _ => Value::Unknown,
                    };
                }
                let scope = if self
                    .agent
                    .inputs
                    .iter()
                    .any(|param| param.name == ident.text)
                {
                    RefScope::Input
                } else {
                    RefScope::Binding
                };
                let mut path = vec![ident.text.clone()];
                path.extend(field_names());
                Value::Ref { scope, path }
            }
        }
    }

    /// Re-read every store field a loop guard names, at the end of its body.
    ///
    /// The node binds the same `$state.<field>` name the guard's own read did,
    /// so the guard's reference resolves to the refreshed value on the next
    /// evaluation. A guard that names no store field appends nothing, which is
    /// why a loop over an ordinary binding lowers exactly as it always has.
    fn refresh_guard_reads(&mut self, level: &mut Vec<usize>, guard: Option<&Expr>, span: Span) {
        let Some(guard) = guard else { return };
        let mut fields = Vec::new();
        collect_store_reads(guard, &mut fields);
        for (scope, field) in fields {
            let mut node = Node::new(String::new(), NodeKind::StateRead);
            node.source_span = Some(self.source_span(span));
            node.field = Some(field.clone());
            node.scope = ref_scope(scope);
            node.binding = Some(format!("${}.{field}", scope.root()));
            self.push(level, node);
        }
    }

    /// Emit one `state.read` per field per statement and reuse it afterwards.
    fn read_state(
        &mut self,
        level: &mut Vec<usize>,
        scope: StateScope,
        field: &str,
        span: Span,
    ) -> String {
        let qualified = format!("{}.{field}", scope.root());
        if let Some(binding) = self.state_reads.get(&qualified) {
            return binding.clone();
        }
        let binding = format!("${qualified}");
        let mut node = Node::new(String::new(), NodeKind::StateRead);
        node.source_span = Some(self.source_span(span));
        node.field = Some(field.to_string());
        node.scope = ref_scope(scope);
        node.binding = Some(binding.clone());
        self.push(level, node);
        self.state_reads.insert(qualified, binding.clone());
        binding
    }
}

/// Every store field an expression reads, in source order and deduplicated.
///
/// Only the first path segment is a field; the rest are record accesses on the
/// value it holds, and re-reading the root refreshes those too.
fn collect_store_reads(expr: &Expr, out: &mut Vec<(StateScope, String)>) {
    match expr {
        Expr::Path(path) => {
            if let (Some(scope), Some(first)) = (path.root.scope(), path.segments.first()) {
                let entry = (scope, first.text.clone());
                if !out.contains(&entry) {
                    out.push(entry);
                }
            }
        }
        Expr::Unary { operand, .. } => collect_store_reads(operand, out),
        Expr::Binary { lhs, rhs, .. } => {
            collect_store_reads(lhs, out);
            collect_store_reads(rhs, out);
        }
        Expr::Builtin { args, .. } => {
            for argument in args {
                collect_store_reads(argument, out);
            }
        }
        Expr::FunctionCall { args, .. } => {
            for argument in args {
                collect_store_reads(&argument.value, out);
            }
        }
        Expr::List { items, .. } => {
            for item in items {
                collect_store_reads(item, out);
            }
        }
        // A guard is a pure expression: the checker refuses an `ask`, a `call`
        // or a `parallel map` in one, so there is nothing else that can read a
        // store. A string literal's interpolations cannot appear in a boolean.
        _ => {}
    }
}

/// The IR scope for a store, or `None` for working memory.
///
/// Working memory is the absent case so that an artifact that uses no
/// persistent memory -- which is every artifact compiled before IR 0.2 grew it
/// -- encodes byte for byte as it always did.
fn ref_scope(scope: StateScope) -> Option<RefScope> {
    match scope {
        StateScope::Ephemeral => None,
        StateScope::Persistent => Some(RefScope::Memory),
    }
}

/// Whether an expression becomes a node of its own rather than a pure value.
fn produces_node(expr: &Expr) -> bool {
    matches!(
        expr,
        Expr::Ask { .. } | Expr::Consult { .. } | Expr::Call { .. } | Expr::ParallelMap { .. }
    )
}
