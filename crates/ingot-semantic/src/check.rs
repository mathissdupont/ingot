//! The checker itself.
//!
//! Runs in four passes so that declaration order in the source never matters:
//!
//! 1. record, tool and verifier declarations
//! 2. agent signatures and their effect upper bound
//! 3. the agent call graph, to reject recursion before it can be walked
//! 4. each agent's sections and flow

use std::collections::{BTreeMap, BTreeSet, HashMap};

use ingot_diagnostics::{codes, Diagnostic, DiagnosticBag};
use ingot_source::Span;
use ingot_syntax::*;
use ingot_types::{
    is_known_model_capability, BudgetKey, Effect, EffectSet, PolicyDecision, PolicySubject, Ty,
    MODEL_CAPABILITIES, SUPPORTED_CURRENCIES,
};

use crate::*;

/// Named arguments `ask` accepts, with the type each one requires.
const ASK_NAMED_ARGS: &[(&str, &str)] = &[
    ("context", "any renderable value"),
    ("system", "string"),
    ("temperature", "float"),
    ("max_tokens", "int"),
];

/// Transports the `tools` block supports in language 0.1.
const SUPPORTED_TRANSPORTS: &[&str] = &["mcp"];

/// Working-memory lifetimes supported in language 0.1.
const SUPPORTED_MEMORY_LIFETIMES: &[&str] = &["ephemeral"];

#[derive(Debug, Clone)]
struct AgentSignature {
    name: String,
    inputs: Vec<Param>,
    output: Option<OutputInfo>,
    /// Upper bound on the effects the agent can trigger, derived from the tools
    /// it grants. Available before its flow is checked, which is what lets an
    /// agent call another agent declared later in the file.
    effects: EffectSet,
    span: Span,
}

#[derive(Debug)]
struct Binding {
    ty: Ty,
    span: Span,
    used: bool,
}

#[derive(Debug, Default)]
struct Scope {
    bindings: HashMap<String, Binding>,
    /// Report unused bindings when this scope closes.
    report_unused: bool,
}

/// Mutable state for checking one agent's flow.
struct FlowCtx {
    agent: String,
    scopes: Vec<Scope>,
    state: Vec<Param>,
    /// Persistent memory fields, addressed by `memory.`.
    persistent: Vec<Param>,
    granted: BTreeSet<String>,
    used_tools: BTreeSet<String>,
    effects: EffectSet,
    output: Option<OutputInfo>,
    emitted_anywhere: bool,
    /// Root binding names whose value has already left the flow via `emit`.
    ///
    /// Roots rather than whole paths: a record cannot be an artifact, so an
    /// `emit` almost always takes a *field* of the value a `verify` names, and
    /// comparing whole paths would never match.
    emitted_roots: BTreeSet<String>,
    /// Depth of enclosing `parallel map` bodies.
    parallel_depth: usize,
}

impl FlowCtx {
    fn push_scope(&mut self, report_unused: bool) {
        self.scopes.push(Scope {
            bindings: HashMap::new(),
            report_unused,
        });
    }

    fn lookup(&mut self, name: &str) -> Option<&Binding> {
        for scope in self.scopes.iter_mut().rev() {
            if let Some(binding) = scope.bindings.get_mut(name) {
                binding.used = true;
                return Some(binding);
            }
        }
        None
    }

    /// Where `name` was introduced, searching every enclosing scope.
    ///
    /// Shadowing is rejected outright: with no shadowing, a name means the same
    /// thing everywhere in an agent, which is what lets lowering resolve a
    /// reference to an input or a binding without carrying scope information
    /// into the IR.
    fn already_bound(&self, name: &str) -> Option<Span> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.bindings.get(name))
            .map(|binding| binding.span)
    }

    fn insert(&mut self, name: String, ty: Ty, span: Span) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.bindings.insert(
                name,
                Binding {
                    ty,
                    span,
                    used: false,
                },
            );
        }
    }

    fn state_field(&self, name: &str) -> Option<&Param> {
        self.state.iter().find(|field| field.name == name)
    }

    /// The declared fields of one of the two stores.
    fn store(&self, scope: StateScope) -> &[Param] {
        match scope {
            StateScope::Ephemeral => &self.state,
            StateScope::Persistent => &self.persistent,
        }
    }

    fn store_field(&self, scope: StateScope, name: &str) -> Option<&Param> {
        self.store(scope).iter().find(|field| field.name == name)
    }

    fn store_names(&self, scope: StateScope) -> Vec<String> {
        self.store(scope)
            .iter()
            .map(|param| param.name.clone())
            .collect()
    }

    fn visible_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .scopes
            .iter()
            .flat_map(|scope| scope.bindings.keys().cloned())
            .collect();
        names.sort();
        names.dedup();
        names
    }
}

pub(crate) struct Checker<'a> {
    program: &'a Program,
    diagnostics: DiagnosticBag,
    records: BTreeMap<String, RecordInfo>,
    tools: BTreeMap<String, ToolInfo>,
    verifiers: BTreeMap<String, VerifierInfo>,
    functions: BTreeMap<String, FunctionInfo>,
    signatures: BTreeMap<String, AgentSignature>,
    recursive_agents: BTreeSet<String>,
    agents: Vec<AgentInfo>,
    exprs: HashMap<Span, ExprInfo>,
    calls: HashMap<Span, CallInfo>,
    verifies: HashMap<Span, VerifyInfo>,
    function_calls: HashMap<Span, FunctionCallInfo>,
    interpolations: HashMap<Span, Ty>,
}

impl<'a> Checker<'a> {
    pub(crate) fn new(program: &'a Program) -> Self {
        Checker {
            program,
            diagnostics: DiagnosticBag::new(),
            records: BTreeMap::new(),
            tools: BTreeMap::new(),
            verifiers: BTreeMap::new(),
            functions: BTreeMap::new(),
            signatures: BTreeMap::new(),
            recursive_agents: BTreeSet::new(),
            agents: Vec::new(),
            exprs: HashMap::new(),
            calls: HashMap::new(),
            verifies: HashMap::new(),
            function_calls: HashMap::new(),
            interpolations: HashMap::new(),
        }
    }

    pub(crate) fn run(mut self) -> Analysis {
        self.collect_records();
        self.collect_tools();
        self.collect_verifiers();
        self.collect_functions();
        self.collect_agent_signatures();
        self.check_function_bodies();
        self.check_verifier_bodies();
        self.detect_recursion();

        if self.program.agents.is_empty() {
            self.diagnostics.push(
                Diagnostic::error(codes::NO_AGENT_DECLARED, "this file declares no agent")
                    .with_primary(
                        self.program.span,
                        "expected at least one `agent` declaration",
                    )
                    .with_help("a compilation unit must define something to run"),
            );
        }

        for index in 0..self.program.agents.len() {
            self.check_agent(index);
        }

        self.diagnostics.sort_by_position();

        Analysis {
            language: self
                .program
                .language
                .map(|version| (version.major, version.minor)),
            package: self.program.package.as_ref().map(|name| name.text()),
            records: self.records,
            tools: self.tools,
            verifiers: self.verifiers,
            functions: self.functions,
            agents: self.agents,
            exprs: self.exprs,
            calls: self.calls,
            verifies: self.verifies,
            function_calls: self.function_calls,
            interpolations: self.interpolations,
            diagnostics: self.diagnostics,
        }
    }

    fn error(&mut self, diagnostic: Diagnostic) {
        self.diagnostics.push(diagnostic);
    }

    // --- pass 1: records, tools, verifiers --------------------------------

    fn collect_records(&mut self) {
        let mut spans: HashMap<String, Span> = HashMap::new();
        for decl in &self.program.types {
            if let Some(previous) = spans.get(&decl.name.text).copied() {
                self.duplicate(&decl.name.text, "type", decl.name.span, previous);
                continue;
            }
            spans.insert(decl.name.text.clone(), decl.name.span);
            self.records.insert(
                decl.name.text.clone(),
                RecordInfo {
                    name: decl.name.text.clone(),
                    fields: Vec::new(),
                    doc: decl.doc.clone(),
                },
            );
        }

        // Field types resolve in a second sweep so records may reference each other.
        for decl in &self.program.types {
            let mut fields = Vec::new();
            let mut seen: HashMap<String, Span> = HashMap::new();
            for field in &decl.fields {
                if let Some(previous) = seen.get(&field.name.text).copied() {
                    self.duplicate(&field.name.text, "field", field.name.span, previous);
                    continue;
                }
                seen.insert(field.name.text.clone(), field.name.span);
                let ty = self.resolve_type(&field.ty);
                fields.push(Param {
                    name: field.name.text.clone(),
                    ty,
                });
            }
            if let Some(record) = self.records.get_mut(&decl.name.text) {
                record.fields = fields;
            }
        }
    }

    fn collect_tools(&mut self) {
        for decl in &self.program.tools {
            let name = decl.name.text();
            if let Some(previous) = self.tools.get(&name).map(|tool| tool.span) {
                self.duplicate(&name, "tool", decl.name.span, previous);
                continue;
            }
            let params = self.resolve_params(&decl.params);
            let result = self.resolve_type(&decl.ret);
            let mut effects = EffectSet::new();
            let mut reach = Vec::new();
            for effect in &decl.effects {
                match Effect::from_name(&effect.name.text) {
                    Some(Effect::ModelAccess) => {
                        self.error(
                            Diagnostic::error(
                                codes::UNKNOWN_EFFECT,
                                "`model_access` cannot be declared on a tool",
                            )
                            .with_primary(effect.name.span, "implicit on `ask`, never on a tool")
                            .with_help("remove this effect"),
                        );
                    }
                    Some(resolved) => {
                        effects.insert(resolved);
                        self.collect_reach(resolved, effect, &mut reach);
                    }
                    None => {
                        let known: Vec<String> = Effect::all()
                            .iter()
                            .filter(|effect| !effect.is_implicitly_granted())
                            .map(|effect| effect.as_str().to_string())
                            .collect();
                        let mut diagnostic = Diagnostic::error(
                            codes::UNKNOWN_EFFECT,
                            format!("unknown effect `{}`", effect.name.text),
                        )
                        .with_primary(effect.name.span, "not a declarable effect")
                        .with_note(format!("declarable effects: {}", known.join(", ")));
                        if let Some(suggestion) = closest_match(&effect.name.text, &known) {
                            diagnostic =
                                diagnostic.with_help(format!("did you mean `{suggestion}`?"));
                        }
                        self.error(diagnostic);
                    }
                }
            }
            // Sorted and deduplicated here rather than at every use, so that
            // the canonical IR encoding is a property of the document instead
            // of a property of whoever wrote the declaration.
            reach.sort_by(|left, right| {
                (left.effect.as_str(), &left.value).cmp(&(right.effect.as_str(), &right.value))
            });
            reach.dedup_by(|left, right| left.effect == right.effect && left.value == right.value);

            self.tools.insert(
                name.clone(),
                ToolInfo {
                    name,
                    params,
                    result,
                    effects,
                    reach,
                    doc: decl.doc.clone(),
                    span: decl.name.span,
                },
            );
        }
    }

    /// Validate one effect's declared reach and collect its values.
    ///
    /// Everything refused here is refused because the alternative is a value
    /// that reads like a constraint and is not one.
    fn collect_reach(
        &mut self,
        effect: Effect,
        decl: &ingot_syntax::EffectDecl,
        reach: &mut Vec<DeclaredReach>,
    ) {
        if !decl.parenthesised {
            return;
        }
        if !effect.takes_reach() {
            self.error(
                Diagnostic::error(
                    codes::INVALID_EFFECT_REACH,
                    format!("`{effect}` names no resource, so it takes no reach"),
                )
                .with_primary(decl.span, "remove the parentheses")
                .with_note(
                    "only `network`, `filesystem_read` and `filesystem_write` reach somewhere \
                     a policy can name",
                ),
            );
            return;
        }
        if decl.values.is_empty() {
            self.error(
                Diagnostic::error(
                    codes::INVALID_EFFECT_REACH,
                    format!("`{effect}()` declares an empty reach"),
                )
                .with_primary(decl.span, "reaches nothing")
                .with_note("a tool that reaches nothing does not need the effect")
                .with_help(format!(
                    "list what it reaches, or drop `!{effect}` from the declaration"
                )),
            );
            return;
        }

        for literal in &decl.values {
            let value = literal.template();
            if let Some(reason) = invalid_reach_value(effect, &value) {
                self.error(
                    Diagnostic::error(
                        codes::INVALID_EFFECT_REACH,
                        format!("`{value}` is not a usable {}", effect.reach_noun()),
                    )
                    .with_primary(literal.span, reason)
                    .with_note(
                        "a reach uses the same values a policy of that subject does; see \
                         Language 0.1 §7.1",
                    ),
                );
                continue;
            }
            reach.push(DeclaredReach {
                effect,
                value,
                span: literal.span,
            });
        }
    }

    fn collect_verifiers(&mut self) {
        for (index, decl) in self.program.verifiers.iter().enumerate() {
            if let Some(previous) = self.verifiers.get(&decl.name.text).map(|info| info.span) {
                self.duplicate(&decl.name.text, "verifier", decl.name.span, previous);
                continue;
            }
            let params = self.resolve_params(&decl.params);
            self.verifiers.insert(
                decl.name.text.clone(),
                VerifierInfo {
                    name: decl.name.text.clone(),
                    params,
                    doc: decl.doc.clone(),
                    span: decl.name.span,
                    decl_index: index,
                    performable: decl.body.is_some(),
                },
            );
        }
    }

    /// A verifier body is a pure `bool` expression over the parameters.
    ///
    /// This is `check_function_bodies` with two differences: the expected type
    /// is fixed rather than declared, and calling a `fn` helper is allowed.
    /// Language 0.2 §7 forbids helper-to-helper calls, so inlining a helper
    /// into a verifier stays one level deep and still terminates.
    fn check_verifier_bodies(&mut self) {
        for index in 0..self.program.verifiers.len() {
            let decl = &self.program.verifiers[index];
            let Some(body) = decl.body.clone() else {
                continue;
            };
            let Some(signature) = self.verifiers.get(&decl.name.text).cloned() else {
                continue;
            };
            let name = decl.name.text.clone();
            let name_span = decl.name.span;

            if let Some(span) = first_impure_expr(&body, true) {
                self.error(
                    Diagnostic::error(
                        codes::FUNCTION_NOT_PURE,
                        format!("verifier `{name}` is not a pure check"),
                    )
                    .with_primary(span, "this expression is not allowed in a verifier body")
                    .with_help(
                        "a verifier body is one boolean expression over its parameters; \
                         a check that needs to `ask` or `call` is not a verifier",
                    ),
                );
            }

            let mut ctx = FlowCtx {
                agent: format!("verifier {name}"),
                scopes: Vec::new(),
                state: Vec::new(),
                persistent: Vec::new(),
                granted: BTreeSet::new(),
                used_tools: BTreeSet::new(),
                effects: EffectSet::new(),
                output: None,
                emitted_anywhere: false,
                emitted_roots: BTreeSet::new(),
                parallel_depth: 0,
            };
            ctx.push_scope(false);
            for param in &signature.params {
                let span = self.program.verifiers[index]
                    .params
                    .iter()
                    .find(|syntax| syntax.name.text == param.name)
                    .map(|syntax| syntax.name.span)
                    .unwrap_or(name_span);
                ctx.insert(param.name.clone(), param.ty.clone(), span);
            }

            let empty_policy = BTreeMap::new();
            let actual = self.check_expr(&mut ctx, &body, &empty_policy);
            if !ctx.effects.is_empty() {
                self.error(
                    Diagnostic::error(
                        codes::FUNCTION_NOT_PURE,
                        format!("verifier `{name}` is not a pure check"),
                    )
                    .with_primary(body.span(), "this expression performs agent work")
                    .with_help("a verifier cannot `ask`, `call` tools or run agents"),
                );
            }
            if !matches!(actual, Ty::Bool) && !actual.is_unknown() {
                self.error(
                    Diagnostic::error(
                        codes::VERIFIER_BODY_NOT_BOOL,
                        format!("verifier `{name}` does not decide anything"),
                    )
                    .with_primary(body.span(), format!("this produces `{actual}`, not `bool`"))
                    .with_help(
                        "a verifier body answers whether the property holds, \
                         e.g. `len(d.sources) >= min`",
                    ),
                );
            }
        }
    }

    fn collect_functions(&mut self) {
        for (index, decl) in self.program.functions.iter().enumerate() {
            if let Some(previous) = self.functions.get(&decl.name.text).map(|info| info.span) {
                self.duplicate(&decl.name.text, "function", decl.name.span, previous);
                continue;
            }
            let params = self.resolve_params(&decl.params);
            let result = self.resolve_type(&decl.ret);
            self.functions.insert(
                decl.name.text.clone(),
                FunctionInfo {
                    name: decl.name.text.clone(),
                    params,
                    result,
                    doc: decl.doc.clone(),
                    span: decl.name.span,
                    decl_index: index,
                },
            );
        }
    }

    fn check_function_bodies(&mut self) {
        for index in 0..self.program.functions.len() {
            let decl = &self.program.functions[index];
            let Some(signature) = self.functions.get(&decl.name.text).cloned() else {
                continue;
            };

            if let Some(span) = first_impure_expr(&decl.body, false) {
                self.error(
                    Diagnostic::error(
                        codes::FUNCTION_NOT_PURE,
                        format!("function `{}` is not pure", decl.name.text),
                    )
                    .with_primary(span, "this expression is not allowed in a pure helper")
                    .with_help(
                        "keep helper bodies as one value expression over parameters and builtins",
                    ),
                );
            }

            let mut ctx = FlowCtx {
                agent: format!("fn {}", decl.name.text),
                scopes: Vec::new(),
                state: Vec::new(),
                persistent: Vec::new(),
                granted: BTreeSet::new(),
                used_tools: BTreeSet::new(),
                effects: EffectSet::new(),
                output: None,
                emitted_anywhere: false,
                emitted_roots: BTreeSet::new(),
                parallel_depth: 0,
            };
            ctx.push_scope(false);
            for param in &signature.params {
                let span = decl
                    .params
                    .iter()
                    .find(|syntax| syntax.name.text == param.name)
                    .map(|syntax| syntax.name.span)
                    .unwrap_or(decl.name.span);
                ctx.insert(param.name.clone(), param.ty.clone(), span);
            }

            let empty_policy = BTreeMap::new();
            let actual = self.check_expr(&mut ctx, &decl.body, &empty_policy);
            if !ctx.effects.is_empty() {
                self.error(
                    Diagnostic::error(
                        codes::FUNCTION_NOT_PURE,
                        format!("function `{}` is not pure", decl.name.text),
                    )
                    .with_primary(decl.body.span(), "this expression performs agent work")
                    .with_help("pure helper functions cannot `ask`, `call` tools or run agents"),
                );
            }
            self.expect_assignable(
                &actual,
                &signature.result,
                decl.body.span(),
                &format!("function `{}` return type", decl.name.text),
            );
        }
    }

    // --- pass 2: agent signatures -----------------------------------------

    fn collect_agent_signatures(&mut self) {
        for decl in &self.program.agents {
            if let Some(previous) = self.signatures.get(&decl.name.text).map(|sig| sig.span) {
                self.duplicate(&decl.name.text, "agent", decl.name.span, previous);
                continue;
            }
            let inputs = self.resolve_params(&decl.params);
            let output = decl.output.as_ref().map(|output| {
                let content = self.resolve_named_type(&output.content);
                OutputInfo {
                    name: output.name.text.clone(),
                    content,
                }
            });

            // Upper bound: everything the granted tools can do.
            let mut effects = EffectSet::new();
            if let Some(tools) = &decl.tools {
                for grant in &tools.grants {
                    if let Some(tool) = self.tools.get(&grant.name.text()) {
                        effects.union_with(&tool.effects);
                    }
                }
            }
            effects.insert(Effect::ModelAccess);

            self.signatures.insert(
                decl.name.text.clone(),
                AgentSignature {
                    name: decl.name.text.clone(),
                    inputs,
                    output,
                    effects,
                    span: decl.name.span,
                },
            );
        }
    }

    // --- pass 3: recursion -------------------------------------------------

    fn detect_recursion(&mut self) {
        let mut edges: BTreeMap<String, Vec<(String, Span)>> = BTreeMap::new();
        for decl in &self.program.agents {
            let mut targets = Vec::new();
            if let Some(flow) = &decl.flow {
                collect_agent_calls(&flow.statements, &self.signatures, &mut targets);
            }
            edges.insert(decl.name.text.clone(), targets);
        }

        let mut in_cycle: BTreeSet<String> = BTreeSet::new();
        for start in edges.keys() {
            // A node is in a cycle exactly when it can reach itself.
            let mut stack = vec![start.clone()];
            let mut seen: BTreeSet<String> = BTreeSet::new();
            while let Some(current) = stack.pop() {
                for (target, _) in edges.get(&current).into_iter().flatten() {
                    if target == start {
                        in_cycle.insert(start.clone());
                    }
                    if seen.insert(target.clone()) {
                        stack.push(target.clone());
                    }
                }
            }
        }

        for name in &in_cycle {
            let Some(call_span) = edges
                .get(name)
                .and_then(|targets| targets.first())
                .map(|(_, span)| *span)
            else {
                continue;
            };
            self.error(
                Diagnostic::error(
                    codes::RECURSIVE_AGENT,
                    format!("agent `{name}` takes part in a call cycle"),
                )
                .with_primary(call_span, "this call can reach the agent again")
                .with_help("model repetition with `loop max N` so the step budget stays checkable"),
            );
        }
        self.recursive_agents = in_cycle;
    }

    // --- pass 4: one agent -------------------------------------------------

    fn check_agent(&mut self, index: usize) {
        let decl = &self.program.agents[index];
        let name = decl.name.text.clone();
        let Some(signature) = self.signatures.get(&name).cloned() else {
            return; // duplicate declaration; already reported
        };

        let model = self.check_model(decl);
        let (grants, granted) = self.check_tool_grants(decl);
        let state = self.check_memory(decl);
        let persistent = self.check_persistent_memory(decl);
        let budget = self.check_budget(decl);
        let policy = self.check_policy(decl);

        if decl.output.is_none() {
            self.error(
                Diagnostic::error(
                    codes::MISSING_OUTPUT_DECLARATION,
                    format!("agent `{name}` declares no output"),
                )
                .with_primary(decl.name.span, "expected `-> name<content_type>`")
                .with_help("an agent must state what it produces, e.g. `-> report<markdown>`"),
            );
        }
        if let (Some(output), Some(decl_output)) = (&signature.output, &decl.output) {
            if !output.content.is_artifact_content() && !output.content.is_unknown() {
                self.error(
                    Diagnostic::error(
                        codes::INVALID_ARTIFACT_TYPE,
                        format!("`{}` cannot be an artifact content type", output.content),
                    )
                    .with_primary(decl_output.content.span, "not a content type")
                    .with_note("content types are: text, markdown, json, file"),
                );
            }
        }

        let mut ctx = FlowCtx {
            agent: name.clone(),
            scopes: Vec::new(),
            state,
            persistent,
            granted,
            used_tools: BTreeSet::new(),
            effects: EffectSet::new(),
            output: signature.output.clone(),
            emitted_anywhere: false,
            emitted_roots: BTreeSet::new(),
            parallel_depth: 0,
        };
        ctx.push_scope(false);
        for input in &signature.inputs {
            let span = decl
                .params
                .iter()
                .find(|param| param.name.text == input.name)
                .map(|param| param.name.span)
                .unwrap_or(decl.name.span);
            ctx.insert(input.name.clone(), input.ty.clone(), span);
        }

        match &decl.flow {
            Some(flow) => {
                ctx.push_scope(true);
                let policy_for_flow = policy.clone();
                self.check_block(&mut ctx, &flow.statements, &policy_for_flow);
                self.close_scope(&mut ctx);
                self.check_output_emission(decl, flow, &ctx);
                self.check_static_steps(decl, flow, &budget);
            }
            None => {
                self.error(
                    Diagnostic::error(
                        codes::MISSING_FLOW_BLOCK,
                        format!("agent `{name}` has no `flow` block"),
                    )
                    .with_primary(decl.name.span, "nothing to execute")
                    .with_help("add a `flow { ... }` section describing what the agent does"),
                );
            }
        }

        for grant in &grants {
            if !ctx.used_tools.contains(&grant.tool) {
                self.diagnostics.push(
                    Diagnostic::warning(
                        codes::UNUSED_TOOL_GRANT,
                        format!("tool `{}` is granted but never called", grant.tool),
                    )
                    .with_primary(grant.span, "unused capability")
                    .with_help("remove the grant to keep the agent's reach minimal"),
                );
            }
        }

        let qualified_name = match self.program.package.as_ref().map(|package| package.text()) {
            Some(package) => format!("{package}.{name}"),
            None => name.clone(),
        };

        let mut effects = ctx.effects.clone();
        effects.insert(Effect::ModelAccess);

        self.agents.push(AgentInfo {
            name,
            qualified_name,
            inputs: signature.inputs,
            output: signature.output,
            model,
            grants,
            state: ctx.state.clone(),
            persistent: ctx.persistent.clone(),
            budget,
            policy,
            effects,
            decl_index: index,
        });
    }

    fn check_model(&mut self, decl: &AgentDecl) -> ModelInfo {
        match &decl.model {
            None => ModelInfo::Unspecified,
            Some(ModelBlock::Exact {
                reference,
                reference_span,
                ..
            }) => {
                if reference.trim().is_empty() {
                    self.error(
                        Diagnostic::error(
                            codes::TYPE_MISMATCH,
                            "model reference must not be empty",
                        )
                        .with_primary(*reference_span, "expected `provider/model`"),
                    );
                }
                ModelInfo::Exact {
                    reference: reference.clone(),
                }
            }
            Some(ModelBlock::Requires { requirements, .. }) => {
                let mut capabilities: Vec<String> = Vec::new();
                let mut context_tokens = None;
                let mut seen: HashMap<String, Span> = HashMap::new();
                for requirement in requirements {
                    match requirement {
                        ModelRequirement::Capability(ident) => {
                            if let Some(previous) = seen.get(&ident.text).copied() {
                                self.duplicate(&ident.text, "capability", ident.span, previous);
                                continue;
                            }
                            seen.insert(ident.text.clone(), ident.span);
                            if !is_known_model_capability(&ident.text) {
                                let known: Vec<String> =
                                    MODEL_CAPABILITIES.iter().map(|c| c.to_string()).collect();
                                let mut diagnostic = Diagnostic::warning(
                                    codes::UNKNOWN_MODEL_CAPABILITY,
                                    format!("unknown model capability `{}`", ident.text),
                                )
                                .with_primary(ident.span, "not a capability this compiler knows")
                                .with_note(format!("known capabilities: {}", known.join(", ")))
                                .with_help(
                                    "unknown capabilities are passed through to the backend, \
                                     which may reject them",
                                );
                                if let Some(suggestion) = closest_match(&ident.text, &known) {
                                    diagnostic = diagnostic
                                        .with_help(format!("did you mean `{suggestion}`?"));
                                }
                                self.error(diagnostic);
                            }
                            capabilities.push(ident.text.clone());
                        }
                        ModelRequirement::ContextAtLeast { tokens, span } => {
                            if *tokens <= 0 {
                                self.error(
                                    Diagnostic::error(
                                        codes::INVALID_BUDGET_VALUE,
                                        "context requirement must be positive",
                                    )
                                    .with_primary(*span, "expected a positive token count"),
                                );
                            } else {
                                context_tokens = Some(*tokens);
                            }
                        }
                    }
                }
                capabilities.sort();
                ModelInfo::Requires {
                    capabilities,
                    context_tokens,
                }
            }
        }
    }

    fn check_tool_grants(&mut self, decl: &AgentDecl) -> (Vec<GrantInfo>, BTreeSet<String>) {
        let mut grants = Vec::new();
        let mut granted = BTreeSet::new();
        let Some(block) = &decl.tools else {
            return (grants, granted);
        };

        for grant in &block.grants {
            let tool_name = grant.name.text();

            if !SUPPORTED_TRANSPORTS.contains(&grant.transport.text.as_str()) {
                self.error(
                    Diagnostic::error(
                        codes::UNSUPPORTED_TRANSPORT,
                        format!("unsupported tool transport `{}`", grant.transport.text),
                    )
                    .with_primary(grant.transport.span, "not supported in language 0.1")
                    .with_note(format!(
                        "supported transports: {}",
                        SUPPORTED_TRANSPORTS.join(", ")
                    )),
                );
            }

            if !self.tools.contains_key(&tool_name) {
                let known: Vec<String> = self.tools.keys().cloned().collect();
                let mut diagnostic = Diagnostic::error(
                    codes::UNKNOWN_TOOL,
                    format!("no tool named `{tool_name}` is declared"),
                )
                .with_primary(grant.name.span, "undeclared tool")
                .with_help(
                    "declare it first, e.g. `tool web.search(query: string) -> json !network`",
                );
                if let Some(suggestion) = closest_match(&tool_name, &known) {
                    diagnostic =
                        diagnostic.with_note(format!("a tool named `{suggestion}` exists"));
                }
                self.error(diagnostic);
                continue;
            }

            if !granted.insert(tool_name.clone()) {
                self.error(
                    Diagnostic::error(
                        codes::DUPLICATE_DECLARATION,
                        format!("tool `{tool_name}` is granted twice"),
                    )
                    .with_primary(grant.span, "duplicate grant"),
                );
                continue;
            }

            grants.push(GrantInfo {
                transport: grant.transport.text.clone(),
                tool: tool_name,
                span: grant.span,
            });
        }
        (grants, granted)
    }

    fn check_memory(&mut self, decl: &AgentDecl) -> Vec<Param> {
        let mut fields = Vec::new();
        let Some(block) = &decl.memory else {
            return fields;
        };
        let Some(working) = &block.working else {
            return fields;
        };

        if !SUPPORTED_MEMORY_LIFETIMES.contains(&working.lifetime.text.as_str()) {
            self.error(
                Diagnostic::error(
                    codes::UNSUPPORTED_MEMORY_LIFETIME,
                    format!("unsupported memory lifetime `{}`", working.lifetime.text),
                )
                .with_primary(working.lifetime.span, "not supported")
                .with_note(format!(
                    "supported lifetimes: {}",
                    SUPPORTED_MEMORY_LIFETIMES.join(", ")
                ))
                .with_help(
                    "state that outlives the run goes in a sibling `persistent { … }` block",
                ),
            );
        }

        let mut seen: HashMap<String, Span> = HashMap::new();
        for field in &working.fields {
            if let Some(previous) = seen.get(&field.name.text).copied() {
                self.duplicate(&field.name.text, "state field", field.name.span, previous);
                continue;
            }
            seen.insert(field.name.text.clone(), field.name.span);
            let ty = self.resolve_type(&field.ty);
            fields.push(Param {
                name: field.name.text.clone(),
                ty,
            });
        }
        fields
    }

    /// The `persistent { … }` block, and the literal each field starts from.
    ///
    /// Returns the fields paired with their initial value, because the value is
    /// part of the declaration rather than an afterthought: it is what makes a
    /// persistent read always answerable. See
    /// [RFC-0018](../../../rfcs/0018-state-that-outlives-a-run.md) §2.2.
    fn check_persistent_memory(&mut self, decl: &AgentDecl) -> Vec<Param> {
        let mut fields = Vec::new();
        let Some(block) = decl
            .memory
            .as_ref()
            .and_then(|memory| memory.persistent.as_ref())
        else {
            return fields;
        };

        let mut seen: HashMap<String, Span> = HashMap::new();
        for field in &block.fields {
            if let Some(previous) = seen.get(&field.name.text).copied() {
                self.duplicate(
                    &field.name.text,
                    StateScope::Persistent.noun(),
                    field.name.span,
                    previous,
                );
                continue;
            }
            seen.insert(field.name.text.clone(), field.name.span);
            let ty = self.resolve_type(&field.ty);

            let Some(expr) = field.initial.as_ref() else {
                self.error(
                    Diagnostic::error(
                        codes::MISSING_INITIAL_VALUE,
                        format!(
                            "persistent memory field `{}` has no initial value",
                            field.name.text
                        ),
                    )
                    .with_primary(field.span, "expected `= <literal>`")
                    .with_note(
                        "persistent memory has no \"not yet written\" state: the first run \
                         starts from the declared value and every later run from the store",
                    )
                    .with_help(format!(
                        "write `{}: {} = …`",
                        field.name.text,
                        field.ty.text()
                    )),
                );
                continue;
            };

            let Some(initial_ty) = literal_ty(expr) else {
                self.error(
                    Diagnostic::error(
                        codes::INITIAL_VALUE_NOT_LITERAL,
                        "a persistent memory field must start from a literal",
                    )
                    .with_primary(expr.span(), "not a literal")
                    .with_note(
                        "the initial value is resolved before any node runs, so there is \
                         nothing here for an expression to read",
                    ),
                );
                continue;
            };

            if !initial_ty.is_unknown() && !initial_ty.is_assignable_to(&ty) {
                self.error(
                    Diagnostic::error(
                        codes::TYPE_MISMATCH,
                        format!("initial value is `{initial_ty}`, but the field is `{ty}`"),
                    )
                    .with_primary(expr.span(), format!("expected `{ty}`")),
                );
                continue;
            }

            fields.push(Param {
                name: field.name.text.clone(),
                ty,
            });
        }
        fields
    }

    fn check_budget(&mut self, decl: &AgentDecl) -> BudgetInfo {
        let mut budget = BudgetInfo::default();
        let Some(block) = &decl.budget else {
            return budget;
        };

        let mut seen: HashMap<String, Span> = HashMap::new();
        for limit in &block.limits {
            let Some(key) = BudgetKey::from_name(&limit.key.text) else {
                let known: Vec<String> = ["steps", "tokens", "cost"]
                    .iter()
                    .map(|k| k.to_string())
                    .collect();
                let mut diagnostic = Diagnostic::error(
                    codes::UNKNOWN_BUDGET_KEY,
                    format!("unknown budget key `{}`", limit.key.text),
                )
                .with_primary(limit.key.span, "not a budget key")
                .with_note(format!("budget keys: {}", known.join(", ")));
                if let Some(suggestion) = closest_match(&limit.key.text, &known) {
                    diagnostic = diagnostic.with_help(format!("did you mean `{suggestion}`?"));
                }
                self.error(diagnostic);
                continue;
            };

            if let Some(previous) = seen.get(&limit.key.text).copied() {
                self.error(
                    Diagnostic::error(
                        codes::DUPLICATE_BUDGET_KEY,
                        format!("budget key `{}` is set twice", limit.key.text),
                    )
                    .with_primary(limit.key.span, "duplicate limit")
                    .with_secondary(previous, "first set here"),
                );
                continue;
            }
            seen.insert(limit.key.text.clone(), limit.key.span);

            if !limit.value.is_positive() {
                self.error(
                    Diagnostic::error(
                        codes::INVALID_BUDGET_VALUE,
                        format!("budget `{}` must be greater than zero", limit.key.text),
                    )
                    .with_primary(limit.span, "a non-positive budget can never be satisfied"),
                );
                continue;
            }

            match key {
                BudgetKey::Steps | BudgetKey::Tokens => {
                    if let Some(unit) = &limit.unit {
                        self.error(
                            Diagnostic::error(
                                codes::INVALID_BUDGET_VALUE,
                                format!("budget `{}` does not take a unit", limit.key.text),
                            )
                            .with_primary(unit.span, "unexpected unit"),
                        );
                    }
                    let NumberLit::Int(value) = limit.value else {
                        self.error(
                            Diagnostic::error(
                                codes::INVALID_BUDGET_VALUE,
                                format!("budget `{}` must be a whole number", limit.key.text),
                            )
                            .with_primary(limit.span, "expected an integer"),
                        );
                        continue;
                    };
                    if key == BudgetKey::Steps {
                        budget.steps = Some(value);
                    } else {
                        budget.tokens = Some(value);
                    }
                }
                BudgetKey::Cost => {
                    let Some(unit) = &limit.unit else {
                        self.error(
                            Diagnostic::error(
                                codes::MISSING_COST_CURRENCY,
                                "`cost` budget needs a currency",
                            )
                            .with_primary(limit.span, "expected e.g. `cost <= 5 usd`")
                            .with_note(format!(
                                "supported currencies: {}",
                                SUPPORTED_CURRENCIES.join(", ")
                            )),
                        );
                        continue;
                    };
                    if !SUPPORTED_CURRENCIES.contains(&unit.text.as_str()) {
                        self.error(
                            Diagnostic::error(
                                codes::MISSING_COST_CURRENCY,
                                format!("unsupported currency `{}`", unit.text),
                            )
                            .with_primary(unit.span, "not a supported currency")
                            .with_note(format!(
                                "supported currencies: {}",
                                SUPPORTED_CURRENCIES.join(", ")
                            )),
                        );
                        continue;
                    }
                    budget.cost = Some(CostBudget {
                        amount: limit.value.as_f64(),
                        currency: unit.text.clone(),
                    });
                }
            }
        }
        budget
    }

    fn check_policy(&mut self, decl: &AgentDecl) -> BTreeMap<PolicySubject, PolicyRuleInfo> {
        let mut policy = BTreeMap::new();
        let Some(block) = &decl.policy else {
            return policy;
        };

        for rule in &block.rules {
            let Some(subject) = PolicySubject::from_name(&rule.subject.text) else {
                let known: Vec<String> = PolicySubject::all()
                    .iter()
                    .map(|s| s.as_str().to_string())
                    .collect();
                let mut diagnostic = Diagnostic::error(
                    codes::UNKNOWN_POLICY_SUBJECT,
                    format!("unknown policy subject `{}`", rule.subject.text),
                )
                .with_primary(rule.subject.span, "not a policy subject")
                .with_note(format!("policy subjects: {}", known.join(", ")));
                if let Some(suggestion) = closest_match(&rule.subject.text, &known) {
                    diagnostic = diagnostic.with_help(format!("did you mean `{suggestion}`?"));
                }
                self.error(diagnostic);
                continue;
            };

            if let Some(existing) = policy.get(&subject).map(|rule: &PolicyRuleInfo| rule.span) {
                self.error(
                    Diagnostic::error(
                        codes::DUPLICATE_POLICY_RULE,
                        format!("policy subject `{subject}` is decided twice"),
                    )
                    .with_primary(rule.span, "duplicate rule")
                    .with_secondary(existing, "first decided here")
                    .with_help(
                        "a subject must have exactly one decision so the artifact is unambiguous",
                    ),
                );
                continue;
            }

            let (decision, values, qualifier) = match &rule.action {
                PolicyAction::Allow {
                    qualifier,
                    values,
                    span,
                } => {
                    let resolved = self.policy_values(subject, values, *span);
                    self.check_qualifier(subject, qualifier.as_ref());
                    (
                        PolicyDecision::Allow,
                        resolved,
                        qualifier.as_ref().map(|q| q.text.clone()),
                    )
                }
                PolicyAction::Deny { qualifier, .. } => {
                    self.check_qualifier(subject, qualifier.as_ref());
                    (
                        PolicyDecision::Deny,
                        Vec::new(),
                        qualifier.as_ref().map(|q| q.text.clone()),
                    )
                }
                PolicyAction::RequireApproval { .. } => {
                    (PolicyDecision::RequireApproval, Vec::new(), None)
                }
            };

            policy.insert(
                subject,
                PolicyRuleInfo {
                    decision,
                    values,
                    qualifier,
                    span: rule.span,
                },
            );
        }
        policy
    }

    fn policy_values(
        &mut self,
        subject: PolicySubject,
        values: &[StringLit],
        span: Span,
    ) -> Vec<String> {
        if !values.is_empty() && !subject.accepts_values() {
            self.error(
                Diagnostic::error(
                    codes::INVALID_POLICY_ACTION,
                    format!("policy subject `{subject}` does not take a value list"),
                )
                .with_primary(span, "unexpected list")
                .with_help(
                    "value lists apply to `network`, `filesystem_read` and `filesystem_write`",
                ),
            );
            return Vec::new();
        }
        let mut resolved = Vec::new();
        for value in values {
            if !value.is_plain() {
                self.error(
                    Diagnostic::error(
                        codes::INVALID_POLICY_ACTION,
                        "policy values must be literal strings",
                    )
                    .with_primary(value.span, "interpolation is not allowed here")
                    .with_help(
                        "a policy that depends on runtime values could not be verified statically",
                    ),
                );
                continue;
            }
            resolved.push(value.plain_text());
        }
        resolved.sort();
        resolved.dedup();
        resolved
    }

    fn check_qualifier(&mut self, subject: PolicySubject, qualifier: Option<&Ident>) {
        let Some(qualifier) = qualifier else { return };
        let accepted = subject.accepted_qualifiers();
        if !accepted.contains(&qualifier.text.as_str()) {
            let mut diagnostic = Diagnostic::error(
                codes::INVALID_POLICY_ACTION,
                format!(
                    "`{subject}` does not accept the qualifier `{}`",
                    qualifier.text
                ),
            )
            .with_primary(qualifier.span, "unexpected qualifier");
            diagnostic = if accepted.is_empty() {
                diagnostic.with_note(format!("`{subject}` takes no qualifier"))
            } else {
                diagnostic.with_note(format!("accepted qualifiers: {}", accepted.join(", ")))
            };
            self.error(diagnostic);
        }
    }

    // --- flow --------------------------------------------------------------

    fn check_block(
        &mut self,
        ctx: &mut FlowCtx,
        statements: &[Stmt],
        policy: &BTreeMap<PolicySubject, PolicyRuleInfo>,
    ) {
        for statement in statements {
            self.check_statement(ctx, statement, policy);
        }
    }

    fn close_scope(&mut self, ctx: &mut FlowCtx) {
        let Some(scope) = ctx.scopes.pop() else {
            return;
        };
        if !scope.report_unused {
            return;
        }
        let mut unused: Vec<(String, Span)> = scope
            .bindings
            .into_iter()
            .filter(|(name, binding)| !binding.used && !name.starts_with('_'))
            .map(|(name, binding)| (name, binding.span))
            .collect();
        unused.sort_by_key(|(_, span)| span.start);
        for (name, span) in unused {
            self.diagnostics.push(
                Diagnostic::warning(codes::UNUSED_BINDING, format!("`{name}` is never used"))
                    .with_primary(span, "bound here")
                    .with_help(format!("remove it, or rename it to `_{name}` to keep it")),
            );
        }
    }

    fn check_statement(
        &mut self,
        ctx: &mut FlowCtx,
        statement: &Stmt,
        policy: &BTreeMap<PolicySubject, PolicyRuleInfo>,
    ) {
        match statement {
            Stmt::Bind { name, value, .. } => {
                let ty = self.check_expr(ctx, value, policy);
                if let Some(previous) = ctx.already_bound(&name.text) {
                    self.error(
                        Diagnostic::error(
                            codes::DUPLICATE_DECLARATION,
                            format!("`{}` is already bound in this agent", name.text),
                        )
                        .with_primary(name.span, "rebinding and shadowing are not allowed")
                        .with_secondary(previous, "first bound here")
                        .with_help(
                            "each name is written once so the flow is a dataflow graph; \
                             use a new name, or `state` for values that change",
                        ),
                    );
                    return;
                }
                ctx.insert(name.text.clone(), ty, name.span);
            }
            Stmt::StateWrite {
                scope,
                field,
                value,
                span,
            } => {
                if ctx.parallel_depth > 0 {
                    self.error(
                        Diagnostic::error(
                            codes::INVALID_IN_PARALLEL,
                            format!(
                                "writing to `{}` inside `parallel map` is not allowed",
                                scope.root()
                            ),
                        )
                        .with_primary(*span, "iterations run concurrently")
                        .with_help("collect the values the map returns and write them afterwards"),
                    );
                }
                let value_ty = self.check_expr(ctx, value, policy);
                let Some(expected) = ctx
                    .store_field(*scope, &field.text)
                    .map(|param| param.ty.clone())
                else {
                    self.error(self.no_such_field(ctx, *scope, &field.text, field.span));
                    return;
                };
                self.expect_assignable(&value_ty, &expected, value.span(), scope.noun());
            }
            Stmt::Expr { value, .. } => {
                let ty = self.check_expr(ctx, value, policy);
                let is_call = matches!(value, Expr::Call { .. });
                if !is_call && !ty.is_unknown() {
                    self.diagnostics.push(
                        Diagnostic::warning(
                            codes::UNUSED_BINDING,
                            "this expression's value is discarded",
                        )
                        .with_primary(value.span(), format!("evaluates to `{ty}`"))
                        .with_help("bind it with `name = ...` or remove the statement"),
                    );
                }
            }
            Stmt::Verify {
                validator,
                args,
                span,
            } => {
                self.check_verify(ctx, validator, args, *span, policy);
            }
            Stmt::Emit {
                output,
                value,
                span,
            } => {
                if ctx.parallel_depth > 0 {
                    self.error(
                        Diagnostic::error(
                            codes::INVALID_IN_PARALLEL,
                            "`emit` inside `parallel map` is not allowed",
                        )
                        .with_primary(*span, "iterations run concurrently")
                        .with_help("emit once, after the map completes"),
                    );
                }
                let value_ty = self.check_expr(ctx, value, policy);
                let Some(declared) = ctx.output.clone() else {
                    return;
                };
                if declared.name != output.text {
                    self.error(
                        Diagnostic::error(
                            codes::UNKNOWN_OUTPUT,
                            format!("`{}` is not an output of this agent", output.text),
                        )
                        .with_primary(output.span, "unknown output")
                        .with_note(format!("this agent declares `{}`", declared.name)),
                    );
                    return;
                }
                ctx.emitted_anywhere = true;
                if let Some(root) = expr_root_binding(value) {
                    ctx.emitted_roots.insert(root);
                }
                if !value_ty.is_assignable_to(&declared.content) {
                    self.error(
                        Diagnostic::error(
                            codes::EMIT_TYPE_MISMATCH,
                            format!(
                                "output `{}` is declared as `{}` but this value is `{value_ty}`",
                                declared.name, declared.content
                            ),
                        )
                        .with_primary(value.span(), format!("expected `{}`", declared.content))
                        .with_help(format!(
                            "produce the value with `ask<{}>(...)` or change the declared output type",
                            declared.content
                        )),
                    );
                }
            }
            Stmt::If {
                condition,
                then_branch,
                else_branch,
                ..
            } => {
                let condition_ty = self.check_expr(ctx, condition, policy);
                if !condition_ty.is_assignable_to(&Ty::Bool) {
                    self.error(
                        Diagnostic::error(
                            codes::CONDITION_NOT_BOOL,
                            format!("condition must be `bool`, found `{condition_ty}`"),
                        )
                        .with_primary(condition.span(), "expected a boolean expression"),
                    );
                }
                ctx.push_scope(true);
                self.check_block(ctx, then_branch, policy);
                self.close_scope(ctx);
                if let Some(else_branch) = else_branch {
                    ctx.push_scope(true);
                    self.check_block(ctx, else_branch, policy);
                    self.close_scope(ctx);
                }
            }
            Stmt::Loop {
                max,
                guard,
                body,
                span,
            } => {
                match max {
                    None => {
                        self.error(
                            Diagnostic::error(codes::UNBOUNDED_LOOP, "`loop` needs a `max` bound")
                                .with_primary(*span, "no static upper bound")
                                .with_help(
                                    "write `loop max 10 { ... }` so step and cost budgets stay checkable",
                                ),
                        );
                    }
                    Some(bound) if bound.value <= 0 => {
                        self.error(
                            Diagnostic::error(
                                codes::INVALID_BUDGET_VALUE,
                                "loop bound must be greater than zero",
                            )
                            .with_primary(bound.span, "this loop can never run"),
                        );
                    }
                    Some(_) => {}
                }
                if let Some(guard) = guard {
                    let guard_ty = self.check_expr(ctx, guard, policy);
                    if !guard_ty.is_assignable_to(&Ty::Bool) {
                        self.error(
                            Diagnostic::error(
                                codes::CONDITION_NOT_BOOL,
                                format!("loop guard must be `bool`, found `{guard_ty}`"),
                            )
                            .with_primary(guard.span(), "expected a boolean expression"),
                        );
                    }
                }
                ctx.push_scope(true);
                self.check_block(ctx, body, policy);
                self.close_scope(ctx);
            }
            Stmt::Checkpoint { label, span } => {
                if ctx.parallel_depth > 0 {
                    self.error(
                        Diagnostic::error(
                            codes::INVALID_IN_PARALLEL,
                            "`checkpoint` inside `parallel map` is not allowed",
                        )
                        .with_primary(*span, "iterations run concurrently"),
                    );
                }
                if !label.is_plain() {
                    self.error(
                        Diagnostic::error(
                            codes::UNRESOLVED_INTERPOLATION,
                            "checkpoint labels must be literal strings",
                        )
                        .with_primary(label.span, "interpolation is not allowed here")
                        .with_help("checkpoint names identify a resume point and must be stable"),
                    );
                }
            }
            Stmt::Error { .. } => {}
        }
    }

    fn check_verify(
        &mut self,
        ctx: &mut FlowCtx,
        validator: &Ident,
        args: &[Arg],
        span: Span,
        policy: &BTreeMap<PolicySubject, PolicyRuleInfo>,
    ) {
        let arg_types: Vec<Ty> = args
            .iter()
            .map(|arg| self.check_expr(ctx, &arg.value, policy))
            .collect();

        let Some(verifier) = self.verifiers.get(&validator.text).cloned() else {
            let known: Vec<String> = self.verifiers.keys().cloned().collect();
            let mut diagnostic = Diagnostic::error(
                codes::UNKNOWN_VERIFIER,
                format!("no verifier named `{}` is declared", validator.text),
            )
            .with_primary(validator.span, "undeclared verifier")
            .with_help(format!(
                "declare it, e.g. `verifier {}(value: markdown, min_sources: int)`",
                validator.text
            ));
            if let Some(suggestion) = closest_match(&validator.text, &known) {
                diagnostic =
                    diagnostic.with_note(format!("a verifier named `{suggestion}` exists"));
            }
            self.error(diagnostic);
            return;
        };

        let arg_order = self.match_arguments(
            &verifier.params,
            args,
            &arg_types,
            span,
            &verifier.name,
            "verifier",
        );
        if !verifier.performable {
            // The declaration is correct, and there is no check to carry out:
            // the artifact will name a property and carry nothing that tests
            // it. This is the last moment before the run where that can be
            // pointed at, which is the whole reason the compiler exists. A
            // warning rather than an error, because a bodyless verifier is
            // still valid source and still says something true.
            self.diagnostics.push(
                Diagnostic::warning(
                    codes::VERIFIER_NOT_PERFORMED,
                    format!("`{}` names a verifier with no body", verifier.name),
                )
                .with_primary(span, "there is no check to carry out")
                .with_note("the run reports it as `notPerformed` rather than claiming it passed")
                .with_help(format!(
                    "give it one, e.g. `verifier {}(...) = <boolean expression>`",
                    verifier.name
                )),
            );
        } else if let Some(emitted) = self.emitted_before_verify(ctx, args) {
            // A check that runs after publication cannot prevent it. Failing
            // ends the run, but the artifact has already been written.
            self.diagnostics.push(
                Diagnostic::warning(
                    codes::VERIFY_AFTER_EMIT,
                    format!(
                        "`{}` checks a value that was already emitted",
                        verifier.name
                    ),
                )
                .with_primary(span, "this check cannot prevent the emit above")
                .with_note(format!("`{emitted}` was emitted earlier in this flow"))
                .with_help("move the `verify` above the `emit`, so a failing check stops it"),
            );
        }

        self.verifies.insert(
            span,
            VerifyInfo {
                verifier: verifier.name,
                arg_order,
            },
        );
    }

    /// The already-emitted binding a `verify` argument names, if any.
    ///
    /// The check runs against roots, so `emit report = found.body` followed by
    /// `verify MinSources(found, ...)` matches: the field that left the flow
    /// came out of the value being checked.
    fn emitted_before_verify(&self, ctx: &FlowCtx, args: &[Arg]) -> Option<String> {
        args.iter()
            .filter_map(|arg| expr_root_binding(&arg.value))
            .find(|root| ctx.emitted_roots.contains(root))
    }

    fn check_output_emission(&mut self, decl: &AgentDecl, flow: &FlowBlock, ctx: &FlowCtx) {
        let Some(output) = &ctx.output else { return };
        if !ctx.emitted_anywhere {
            self.error(
                Diagnostic::error(
                    codes::OUTPUT_NEVER_EMITTED,
                    format!("output `{}` is never emitted", output.name),
                )
                .with_primary(flow.span, "no `emit` in this flow")
                .with_help(format!(
                    "end the flow with `emit {} = <value>`",
                    output.name
                )),
            );
            return;
        }
        if !definitely_emits(&flow.statements, &output.name) {
            self.diagnostics.push(
                Diagnostic::warning(
                    codes::OUTPUT_NOT_ON_ALL_PATHS,
                    format!("output `{}` is not emitted on every path", output.name),
                )
                .with_primary(
                    decl.name.span,
                    "some paths finish without producing the output",
                )
                .with_help(
                    "add an `else` branch that emits, or move the `emit` after the branch; \
                     a bounded loop may run zero times and never counts",
                ),
            );
        }
    }

    fn check_static_steps(&mut self, decl: &AgentDecl, flow: &FlowBlock, budget: &BudgetInfo) {
        let Some(limit) = budget.steps else { return };
        let minimum = min_steps(&flow.statements);
        if minimum > limit {
            self.error(
                Diagnostic::error(
                    codes::STATIC_STEPS_EXCEED_BUDGET,
                    format!(
                        "the flow needs at least {minimum} steps but the budget allows {limit}"
                    ),
                )
                .with_primary(flow.span, format!("shortest path performs {minimum} calls"))
                .with_secondary(decl.name.span, "in this agent")
                .with_help("raise `steps` in the budget block, or shorten the flow"),
            );
        }
    }

    // --- expressions --------------------------------------------------------

    fn check_expr(
        &mut self,
        ctx: &mut FlowCtx,
        expr: &Expr,
        policy: &BTreeMap<PolicySubject, PolicyRuleInfo>,
    ) -> Ty {
        let (ty, effects) = self.infer(ctx, expr, policy);
        ctx.effects.union_with(&effects);
        self.exprs.insert(
            expr.span(),
            ExprInfo {
                ty: ty.clone(),
                effects,
            },
        );
        ty
    }

    fn infer(
        &mut self,
        ctx: &mut FlowCtx,
        expr: &Expr,
        policy: &BTreeMap<PolicySubject, PolicyRuleInfo>,
    ) -> (Ty, EffectSet) {
        match expr {
            Expr::Str(literal) => {
                self.check_interpolations(ctx, literal);
                (Ty::String, EffectSet::new())
            }
            Expr::Int { .. } => (Ty::Int, EffectSet::new()),
            Expr::Float { .. } => (Ty::Float, EffectSet::new()),
            Expr::Bool { .. } => (Ty::Bool, EffectSet::new()),
            Expr::List { items, span } => {
                let mut effects = EffectSet::new();
                let mut element = Ty::Unknown;
                for item in items {
                    let item_ty = self.check_expr(ctx, item, policy);
                    if let Some(info) = self.exprs.get(&item.span()) {
                        effects.union_with(&info.effects);
                    }
                    if element.is_unknown() {
                        element = item_ty;
                    } else if !item_ty.is_assignable_to(&element) {
                        if element.is_assignable_to(&item_ty) {
                            element = item_ty;
                        } else {
                            self.error(
                                Diagnostic::error(
                                    codes::TYPE_MISMATCH,
                                    format!(
                                        "list elements must share a type: found `{element}` and `{item_ty}`"
                                    ),
                                )
                                .with_primary(item.span(), format!("this element is `{item_ty}`"))
                                .with_secondary(*span, "in this list"),
                            );
                            element = Ty::Unknown;
                        }
                    }
                }
                (Ty::list_of(element), effects)
            }
            Expr::Path(path) => (self.infer_path(ctx, path), EffectSet::new()),
            Expr::Ask { result, args, span } => {
                let ty = self.resolve_type(result);
                let effects: EffectSet = [Effect::ModelAccess].into_iter().collect();
                self.check_ask_args(ctx, args, *span, policy);
                (ty, effects)
            }
            Expr::Call { callee, args, span } => self.infer_call(ctx, callee, args, *span, policy),
            Expr::ParallelMap {
                source,
                binder,
                body,
                span,
            } => {
                let source_ty = self.check_expr(ctx, source, policy);
                let element = match &source_ty {
                    Ty::List(element) => (**element).clone(),
                    Ty::Unknown => Ty::Unknown,
                    other => {
                        self.error(
                            Diagnostic::error(
                                codes::NOT_A_LIST,
                                format!("`parallel map` needs a list, found `{other}`"),
                            )
                            .with_primary(source.span(), format!("this is `{other}`"))
                            .with_help("map over a list, e.g. the result of `ask<string[]>(...)`"),
                        );
                        Ty::Unknown
                    }
                };

                if let Some(previous) = ctx.already_bound(&binder.text) {
                    self.error(
                        Diagnostic::error(
                            codes::DUPLICATE_DECLARATION,
                            format!("`{}` is already bound in this agent", binder.text),
                        )
                        .with_primary(
                            binder.span,
                            "a loop variable may not shadow an existing name",
                        )
                        .with_secondary(previous, "first bound here"),
                    );
                }

                ctx.parallel_depth += 1;
                ctx.push_scope(true);
                ctx.insert(binder.text.clone(), element, binder.span);
                self.check_block(ctx, body, policy);

                let result = match body.last() {
                    Some(Stmt::Expr { value, .. }) => self
                        .exprs
                        .get(&value.span())
                        .map(|info| info.ty.clone())
                        .unwrap_or(Ty::Unknown),
                    _ => {
                        self.error(
                            Diagnostic::error(
                                codes::PARALLEL_BODY_MUST_YIELD_VALUE,
                                "the body of `parallel map` must end in an expression",
                            )
                            .with_primary(*span, "no value to collect")
                            .with_help(
                                "finish the body with the value each iteration contributes, \
                                 e.g. `call web.search(query)`",
                            ),
                        );
                        Ty::Unknown
                    }
                };

                self.close_scope(ctx);
                ctx.parallel_depth -= 1;

                // Effects were folded into ctx.effects while checking the body.
                (Ty::list_of(result), EffectSet::new())
            }
            Expr::Builtin { name, args, span } => {
                self.infer_builtin(ctx, name, args, *span, policy)
            }
            Expr::FunctionCall { callee, args, span } => {
                self.infer_function_call(ctx, callee, args, *span, policy)
            }
            Expr::Unary { op, operand, span } => {
                let operand_ty = self.check_expr(ctx, operand, policy);
                let result = match op {
                    UnaryOp::Not => {
                        if !operand_ty.is_assignable_to(&Ty::Bool) {
                            self.error(
                                Diagnostic::error(
                                    codes::INVALID_OPERAND_TYPE,
                                    format!("`!` needs a `bool`, found `{operand_ty}`"),
                                )
                                .with_primary(*span, "invalid operand"),
                            );
                        }
                        Ty::Bool
                    }
                    UnaryOp::Neg => {
                        if !operand_ty.is_numeric() && !operand_ty.is_unknown() {
                            self.error(
                                Diagnostic::error(
                                    codes::INVALID_OPERAND_TYPE,
                                    format!("`-` needs a number, found `{operand_ty}`"),
                                )
                                .with_primary(*span, "invalid operand"),
                            );
                            Ty::Unknown
                        } else {
                            operand_ty
                        }
                    }
                };
                (result, EffectSet::new())
            }
            Expr::Binary { op, lhs, rhs, span } => {
                let lhs_ty = self.check_expr(ctx, lhs, policy);
                let rhs_ty = self.check_expr(ctx, rhs, policy);
                let result = self.check_binary(*op, &lhs_ty, &rhs_ty, *span);
                (result, EffectSet::new())
            }
            Expr::Error { .. } => (Ty::Unknown, EffectSet::new()),
        }
    }

    fn check_binary(&mut self, op: BinaryOp, lhs: &Ty, rhs: &Ty, span: Span) -> Ty {
        if lhs.is_unknown() || rhs.is_unknown() {
            return if op.is_arithmetic() {
                Ty::Unknown
            } else {
                Ty::Bool
            };
        }
        if op.is_logical() {
            if !lhs.is_assignable_to(&Ty::Bool) || !rhs.is_assignable_to(&Ty::Bool) {
                self.error(
                    Diagnostic::error(
                        codes::INVALID_OPERAND_TYPE,
                        format!(
                            "`{}` needs two booleans, found `{lhs}` and `{rhs}`",
                            op.symbol()
                        ),
                    )
                    .with_primary(span, "invalid operands"),
                );
            }
            return Ty::Bool;
        }
        if op.is_comparison() {
            let compatible = lhs.is_assignable_to(rhs) || rhs.is_assignable_to(lhs);
            if !compatible || !lhs.is_comparable() {
                self.error(
                    Diagnostic::error(
                        codes::INVALID_OPERAND_TYPE,
                        format!("cannot compare `{lhs}` with `{rhs}`"),
                    )
                    .with_primary(span, "invalid comparison")
                    .with_note("comparable types are: string, int, float, bool, text, markdown"),
                );
            }
            return Ty::Bool;
        }
        if !lhs.is_numeric() || !rhs.is_numeric() {
            self.error(
                Diagnostic::error(
                    codes::INVALID_OPERAND_TYPE,
                    format!("`{}` needs numbers, found `{lhs}` and `{rhs}`", op.symbol()),
                )
                .with_primary(span, "invalid operands"),
            );
            return Ty::Unknown;
        }
        if *lhs == Ty::Float || *rhs == Ty::Float {
            Ty::Float
        } else {
            Ty::Int
        }
    }

    fn infer_builtin(
        &mut self,
        ctx: &mut FlowCtx,
        name: &Ident,
        args: &[Expr],
        span: Span,
        policy: &BTreeMap<PolicySubject, PolicyRuleInfo>,
    ) -> (Ty, EffectSet) {
        let arg_types: Vec<Ty> = args
            .iter()
            .map(|arg| self.check_expr(ctx, arg, policy))
            .collect();
        match name.text.as_str() {
            "len" => {
                if arg_types.len() != 1 {
                    self.error(
                        Diagnostic::error(
                            codes::ARGUMENT_COUNT_MISMATCH,
                            format!("`len` takes 1 argument, found {}", arg_types.len()),
                        )
                        .with_primary(span, "wrong number of arguments"),
                    );
                    return (Ty::Int, EffectSet::new());
                }
                let arg_ty = &arg_types[0];
                let ok = matches!(arg_ty, Ty::List(_) | Ty::String | Ty::Text | Ty::Markdown)
                    || arg_ty.is_unknown();
                if !ok {
                    self.error(
                        Diagnostic::error(
                            codes::INVALID_OPERAND_TYPE,
                            format!("`len` needs a list or text value, found `{arg_ty}`"),
                        )
                        .with_primary(args[0].span(), "invalid argument"),
                    );
                }
                (Ty::Int, EffectSet::new())
            }
            other => {
                self.error(
                    Diagnostic::error(
                        codes::UNRESOLVED_NAME,
                        format!("`{other}` is not a builtin function"),
                    )
                    .with_primary(name.span, "unknown builtin")
                    .with_note(format!("builtins: {}", BUILTINS.join(", "))),
                );
                (Ty::Unknown, EffectSet::new())
            }
        }
    }

    fn infer_function_call(
        &mut self,
        ctx: &mut FlowCtx,
        callee: &Ident,
        args: &[Arg],
        span: Span,
        policy: &BTreeMap<PolicySubject, PolicyRuleInfo>,
    ) -> (Ty, EffectSet) {
        let arg_types: Vec<Ty> = args
            .iter()
            .map(|arg| self.check_expr(ctx, &arg.value, policy))
            .collect();
        let name = callee.text.clone();
        let Some(function) = self.functions.get(&name).cloned() else {
            let known: Vec<String> = self.functions.keys().cloned().collect();
            let mut diagnostic = Diagnostic::error(
                codes::UNRESOLVED_NAME,
                format!("no function named `{name}`"),
            )
            .with_primary(callee.span, "not a declared helper")
            .with_help("declare a helper with `fn name(args) -> type = expression`");
            if let Some(suggestion) = closest_match(&name, &known) {
                diagnostic =
                    diagnostic.with_note(format!("a function named `{suggestion}` exists"));
            }
            self.error(diagnostic);
            return (Ty::Unknown, EffectSet::new());
        };

        let arg_order = self.match_arguments(
            &function.params,
            args,
            &arg_types,
            span,
            &function.name,
            "function",
        );
        self.function_calls.insert(
            span,
            FunctionCallInfo {
                function: function.name,
                result: function.result.clone(),
                arg_order,
            },
        );
        (function.result, EffectSet::new())
    }

    /// "no such field" for either store, with the neighbouring store checked.
    ///
    /// Naming the wrong root is the mistake this is most likely to be reporting
    /// — the two look alike and hold the same kind of thing — so a field that
    /// exists on the other side is worth more than a spelling suggestion.
    fn no_such_field(
        &self,
        ctx: &FlowCtx,
        scope: StateScope,
        name: &str,
        span: Span,
    ) -> Diagnostic {
        let other = match scope {
            StateScope::Ephemeral => StateScope::Persistent,
            StateScope::Persistent => StateScope::Ephemeral,
        };
        let mut diagnostic = Diagnostic::error(
            codes::UNKNOWN_STATE_FIELD,
            format!("no {} named `{name}`", scope.noun()),
        )
        .with_primary(span, "not declared in `memory`");

        if ctx.store_field(other, name).is_some() {
            return diagnostic.with_help(format!(
                "`{name}` is declared as a {}; write `{}.{name}`",
                other.noun(),
                other.root()
            ));
        }

        let known = ctx.store_names(scope);
        diagnostic = if known.is_empty() {
            diagnostic.with_help(match scope {
                StateScope::Ephemeral => {
                    "declare it, e.g. `memory { working ephemeral { notes: string[] } }`"
                }
                StateScope::Persistent => {
                    "declare it, e.g. `memory { persistent { seen: string[] = [] } }`"
                }
            })
        } else {
            diagnostic.with_note(format!("declared: {}", known.join(", ")))
        };
        match closest_match(name, &known) {
            Some(suggestion) => diagnostic.with_help(format!("did you mean `{suggestion}`?")),
            None => diagnostic,
        }
    }

    fn infer_path(&mut self, ctx: &mut FlowCtx, path: &PathExpr) -> Ty {
        let mut current = match &path.root {
            PathRoot::State { span } | PathRoot::Memory { span } => {
                let scope = path.root.scope().expect("both arms address a store");
                let Some(first) = path.segments.first() else {
                    self.error(
                        Diagnostic::error(
                            codes::UNKNOWN_STATE_FIELD,
                            format!("`{}` must be followed by a field name", scope.root()),
                        )
                        .with_primary(*span, format!("expected `{}.field`", scope.root())),
                    );
                    return Ty::Unknown;
                };
                let Some(field) = ctx.store_field(scope, &first.text).cloned() else {
                    self.error(self.no_such_field(ctx, scope, &first.text, first.span));
                    return Ty::Unknown;
                };
                return self.walk_fields(field.ty, &path.segments[1..]);
            }
            PathRoot::Binding(ident) => match ctx.lookup(&ident.text) {
                Some(binding) => binding.ty.clone(),
                None => {
                    let mut candidates = ctx.visible_names();
                    let state_hit = ctx.state_field(&ident.text).is_some();
                    let mut diagnostic = Diagnostic::error(
                        codes::UNRESOLVED_NAME,
                        format!("cannot find `{}` in this scope", ident.text),
                    )
                    .with_primary(ident.span, "not found");
                    if state_hit {
                        diagnostic = diagnostic.with_help(format!(
                            "write `state.{}` to read working memory",
                            ident.text
                        ));
                    } else {
                        candidates.extend(ctx.state.iter().map(|param| param.name.clone()));
                        if let Some(suggestion) = closest_match(&ident.text, &candidates) {
                            diagnostic =
                                diagnostic.with_help(format!("did you mean `{suggestion}`?"));
                        }
                    }
                    self.error(diagnostic);
                    return Ty::Unknown;
                }
            },
        };
        current = self.walk_fields(current, &path.segments);
        current
    }

    fn walk_fields(&mut self, mut current: Ty, segments: &[Ident]) -> Ty {
        for segment in segments {
            if current.is_unknown() {
                return Ty::Unknown;
            }
            let Ty::Record(record_name) = &current else {
                self.error(
                    Diagnostic::error(codes::NOT_A_RECORD, format!("`{current}` has no fields"))
                        .with_primary(
                            segment.span,
                            format!("cannot read `.{}` on `{current}`", segment.text),
                        )
                        .with_help("only record types declared with `type` have fields"),
                );
                return Ty::Unknown;
            };
            let Some(record) = self.records.get(record_name) else {
                return Ty::Unknown;
            };
            let Some(field) = record.field(&segment.text).cloned() else {
                let known: Vec<String> = record
                    .fields
                    .iter()
                    .map(|field| field.name.clone())
                    .collect();
                let mut diagnostic = Diagnostic::error(
                    codes::UNKNOWN_FIELD,
                    format!("`{record_name}` has no field `{}`", segment.text),
                )
                .with_primary(segment.span, "unknown field");
                if !known.is_empty() {
                    diagnostic = diagnostic.with_note(format!("fields: {}", known.join(", ")));
                }
                if let Some(suggestion) = closest_match(&segment.text, &known) {
                    diagnostic = diagnostic.with_help(format!("did you mean `{suggestion}`?"));
                }
                self.error(diagnostic);
                return Ty::Unknown;
            };
            current = field.ty;
        }
        current
    }

    fn check_interpolations(&mut self, ctx: &mut FlowCtx, literal: &StringLit) {
        for part in &literal.parts {
            let StringPart::Interpolation(path) = part else {
                continue;
            };
            let ty = match &path.root {
                PathRoot::State { .. } | PathRoot::Memory { .. } => {
                    let scope = path.root.scope().expect("both arms address a store");
                    let Some(first) = path.segments.first() else {
                        self.error(
                            Diagnostic::error(
                                codes::UNRESOLVED_INTERPOLATION,
                                format!("`${{{}}}` must name a field", scope.root()),
                            )
                            .with_primary(
                                path.span,
                                format!("expected `${{{}.field}}`", scope.root()),
                            ),
                        );
                        continue;
                    };
                    let Some(field) = ctx.store_field(scope, &first.text).cloned() else {
                        self.error(self.no_such_field(ctx, scope, &first.text, path.span));
                        continue;
                    };
                    self.walk_fields(field.ty, &path.segments[1..])
                }
                PathRoot::Binding(ident) => {
                    let Some(binding) = ctx.lookup(&ident.text) else {
                        let mut candidates = ctx.visible_names();
                        candidates.extend(ctx.state.iter().map(|param| param.name.clone()));
                        let mut diagnostic = Diagnostic::error(
                            codes::UNRESOLVED_INTERPOLATION,
                            format!("`{}` in this prompt is not in scope", path.text()),
                        )
                        .with_primary(path.span, "unknown name")
                        .with_help(
                            "prompt placeholders are checked like any other expression, \
                             so a typo cannot silently render as empty text",
                        );
                        if let Some(suggestion) = closest_match(&ident.text, &candidates) {
                            diagnostic =
                                diagnostic.with_note(format!("did you mean `{suggestion}`?"));
                        }
                        self.error(diagnostic);
                        continue;
                    };
                    let ty = binding.ty.clone();
                    self.walk_fields(ty, &path.segments)
                }
            };
            self.interpolations.insert(path.span, ty.clone());
            if !ty.is_unknown() && !ty.is_renderable() {
                self.error(
                    Diagnostic::error(
                        codes::TYPE_MISMATCH,
                        format!("`{ty}` cannot be rendered into a prompt"),
                    )
                    .with_primary(path.span, format!("`{}` is `{ty}`", path.text()))
                    .with_help(
                        "pass binary values as tool arguments instead of interpolating them",
                    ),
                );
            }
        }
    }

    fn check_ask_args(
        &mut self,
        ctx: &mut FlowCtx,
        args: &[Arg],
        span: Span,
        policy: &BTreeMap<PolicySubject, PolicyRuleInfo>,
    ) {
        let mut prompt_seen = false;
        let mut named_seen: HashMap<String, Span> = HashMap::new();

        for arg in args {
            let arg_ty = self.check_expr(ctx, &arg.value, policy);
            match &arg.name {
                None => {
                    if prompt_seen {
                        self.error(
                            Diagnostic::error(
                                codes::ARGUMENT_COUNT_MISMATCH,
                                "`ask` takes exactly one positional argument, the prompt",
                            )
                            .with_primary(arg.span, "unexpected extra positional argument")
                            .with_help(
                                "pass additional inputs as named arguments, e.g. `context: value`",
                            ),
                        );
                        continue;
                    }
                    prompt_seen = true;
                    if !arg_ty.is_assignable_to(&Ty::String) {
                        self.error(
                            Diagnostic::error(
                                codes::TYPE_MISMATCH,
                                format!("prompt must be a `string`, found `{arg_ty}`"),
                            )
                            .with_primary(arg.value.span(), "expected a string literal"),
                        );
                    }
                }
                Some(name) => {
                    if let Some(previous) = named_seen.get(&name.text).copied() {
                        self.error(
                            Diagnostic::error(
                                codes::UNKNOWN_ARGUMENT,
                                format!("argument `{}` is given twice", name.text),
                            )
                            .with_primary(name.span, "duplicate argument")
                            .with_secondary(previous, "first given here"),
                        );
                        continue;
                    }
                    named_seen.insert(name.text.clone(), name.span);

                    let Some((_, expected)) = ASK_NAMED_ARGS
                        .iter()
                        .find(|(known, _)| *known == name.text.as_str())
                    else {
                        let known: Vec<String> = ASK_NAMED_ARGS
                            .iter()
                            .map(|(name, _)| name.to_string())
                            .collect();
                        let mut diagnostic = Diagnostic::error(
                            codes::UNKNOWN_ARGUMENT,
                            format!("`ask` has no argument named `{}`", name.text),
                        )
                        .with_primary(name.span, "unknown argument")
                        .with_note(format!("named arguments: {}", known.join(", ")));
                        if let Some(suggestion) = closest_match(&name.text, &known) {
                            diagnostic =
                                diagnostic.with_help(format!("did you mean `{suggestion}`?"));
                        }
                        self.error(diagnostic);
                        continue;
                    };

                    let expected_ty = match *expected {
                        "string" => Some(Ty::String),
                        "float" => Some(Ty::Float),
                        "int" => Some(Ty::Int),
                        _ => None,
                    };
                    if let Some(expected_ty) = expected_ty {
                        self.expect_assignable(
                            &arg_ty,
                            &expected_ty,
                            arg.value.span(),
                            &format!("argument `{}`", name.text),
                        );
                    } else if !arg_ty.is_renderable() && !arg_ty.is_unknown() {
                        self.error(
                            Diagnostic::error(
                                codes::TYPE_MISMATCH,
                                format!("`{arg_ty}` cannot be passed as model context"),
                            )
                            .with_primary(arg.value.span(), "not renderable"),
                        );
                    }
                }
            }
        }

        if !prompt_seen {
            self.error(
                Diagnostic::error(codes::MISSING_ARGUMENT, "`ask` needs a prompt")
                    .with_primary(span, "missing the positional prompt argument")
                    .with_help("write `ask<markdown>(\"...\")`"),
            );
        }
    }

    fn infer_call(
        &mut self,
        ctx: &mut FlowCtx,
        callee: &DottedName,
        args: &[Arg],
        span: Span,
        policy: &BTreeMap<PolicySubject, PolicyRuleInfo>,
    ) -> (Ty, EffectSet) {
        let name = callee.text();
        let arg_types: Vec<Ty> = args
            .iter()
            .map(|arg| self.check_expr(ctx, &arg.value, policy))
            .collect();

        if let Some(tool) = self.tools.get(&name).cloned() {
            ctx.used_tools.insert(name.clone());
            if !ctx.granted.contains(&name) {
                self.error(
                    Diagnostic::error(
                        codes::TOOL_NOT_GRANTED,
                        format!("tool `{name}` is not granted to agent `{}`", ctx.agent),
                    )
                    .with_primary(callee.span, "not listed in the `tools` block")
                    .with_secondary(tool.span, "declared here")
                    .with_help(format!("add `mcp {name}` to the agent's `tools` block")),
                );
            }
            self.check_effects_against_policy(&tool.effects, &name, span, policy);
            self.check_reach_against_policy(&tool, policy);
            let arg_order =
                self.match_arguments(&tool.params, args, &arg_types, span, &tool.name, "tool");
            self.calls.insert(
                span,
                CallInfo {
                    target: CallTarget::Tool(name),
                    result: tool.result.clone(),
                    effects: tool.effects.clone(),
                    arg_order,
                },
            );
            return (tool.result, tool.effects);
        }

        if let Some(signature) = self.signatures.get(&name).cloned() {
            if self.recursive_agents.contains(&name) {
                // The cycle was already reported; do not cascade.
                return (Ty::Unknown, EffectSet::new());
            }
            self.check_effects_against_policy(&signature.effects, &name, span, policy);
            let arg_order = self.match_arguments(
                &signature.inputs,
                args,
                &arg_types,
                span,
                &signature.name,
                "agent",
            );
            let result = signature
                .output
                .as_ref()
                .map(|output| output.content.clone())
                .unwrap_or(Ty::Unknown);
            self.calls.insert(
                span,
                CallInfo {
                    target: CallTarget::Agent(name),
                    result: result.clone(),
                    effects: signature.effects.clone(),
                    arg_order,
                },
            );
            return (result, signature.effects);
        }

        let mut candidates: Vec<String> = self.tools.keys().cloned().collect();
        candidates.extend(self.signatures.keys().cloned());
        let mut diagnostic = Diagnostic::error(
            codes::UNRESOLVED_NAME,
            format!("no tool or agent named `{name}`"),
        )
        .with_primary(callee.span, "not found")
        .with_help("declare the tool with `tool ...`, or check the spelling");
        if let Some(suggestion) = closest_match(&name, &candidates) {
            diagnostic = diagnostic.with_note(format!("`{suggestion}` is declared"));
        }
        self.error(diagnostic);
        (Ty::Unknown, EffectSet::new())
    }

    /// Default-deny enforcement.
    fn check_effects_against_policy(
        &mut self,
        effects: &EffectSet,
        callee: &str,
        span: Span,
        policy: &BTreeMap<PolicySubject, PolicyRuleInfo>,
    ) {
        for effect in effects.iter() {
            if effect.is_implicitly_granted() {
                continue;
            }
            let Some(subject) = PolicySubject::for_effect(effect) else {
                continue;
            };
            match policy.get(&subject) {
                Some(rule) if rule.decision == PolicyDecision::Allow => {}
                Some(rule) if rule.decision == PolicyDecision::RequireApproval => {
                    self.diagnostics.push(
                        Diagnostic::note(
                            codes::APPROVAL_INSERTED,
                            format!("an approval checkpoint is inserted before `{callee}`"),
                        )
                        .with_primary(span, format!("requires `{effect}`"))
                        .with_secondary(rule.span, "policy requires approval for this subject"),
                    );
                }
                Some(rule) => {
                    self.error(
                        Diagnostic::error(
                            codes::DENIED_CAPABILITY,
                            format!("`{callee}` requires the `{effect}` effect, which the policy denies"),
                        )
                        .with_primary(span, format!("needs `{effect}`"))
                        .with_secondary(rule.span, format!("`{subject}` is denied here"))
                        .with_help(format!(
                            "allow it with `{subject} allow [...]`, or call a tool that does not need `{effect}`"
                        )),
                    );
                }
                None => {
                    self.error(
                        Diagnostic::error(
                            codes::MISSING_POLICY_RULE,
                            format!("`{callee}` requires the `{effect}` effect, which no policy rule grants"),
                        )
                        .with_primary(span, format!("needs `{effect}`"))
                        .with_note("Ingot is default-deny: an effect with no rule is denied")
                        .with_help(format!(
                            "add `{subject} allow [...]` to the agent's `policy` block"
                        )),
                    );
                }
            }
        }
    }

    /// Containment: what a tool reaches must be inside what the agent granted.
    ///
    /// This is the check the values in a policy never had. Before it, adding a
    /// host to `network allow [...]` changed nothing a compiler could see, and
    /// removing one changed nothing either — the list was a claim with no
    /// reader. It has one now.
    ///
    /// A grant with no values is unbounded and contains everything, which is
    /// what keeps every artifact written before [RFC-0014] compiling. A tool
    /// that declares no reach is not checked: the policy still bounds it
    /// wherever anything bounds anything, and what is missing is only this
    /// earlier, cheaper answer.
    fn check_reach_against_policy(
        &mut self,
        tool: &ToolInfo,
        policy: &BTreeMap<PolicySubject, PolicyRuleInfo>,
    ) {
        for declared in &tool.reach {
            let Some(subject) = PolicySubject::for_effect(declared.effect) else {
                continue;
            };
            let Some(rule) = policy.get(&subject) else {
                // No rule at all is already ING4007 on the effect kind. Saying
                // it twice would bury the answer under the elaboration.
                continue;
            };
            if rule.decision == PolicyDecision::Deny {
                continue; // Already ING4001.
            }
            if rule.values.is_empty() {
                continue; // An unbounded grant contains any reach.
            }
            if rule.values.contains(&declared.value) {
                continue;
            }

            let noun = declared.effect.reach_noun();
            self.error(
                Diagnostic::error(
                    codes::REACH_BEYOND_POLICY,
                    format!(
                        "`{}` reaches the {noun} `{}`, which this agent's policy does not grant",
                        tool.name, declared.value
                    ),
                )
                .with_primary(declared.span, format!("declared {noun}"))
                .with_secondary(rule.span, format!("`{subject}` is granted here"))
                .with_note(format!(
                    "granted: {}",
                    rule.values
                        .iter()
                        .map(|value| format!("`{value}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
                .with_help(format!(
                    "add \"{}\" to `{subject} allow [...]`, or call a tool that does not need it",
                    declared.value
                )),
            );
        }
    }

    /// Normalise positional and named arguments into declaration order.
    fn match_arguments(
        &mut self,
        params: &[Param],
        args: &[Arg],
        arg_types: &[Ty],
        span: Span,
        callee: &str,
        kind: &str,
    ) -> Vec<Option<usize>> {
        let mut order: Vec<Option<usize>> = vec![None; params.len()];
        let mut positional = 0usize;

        for (index, arg) in args.iter().enumerate() {
            let target = match &arg.name {
                Some(name) => match params.iter().position(|param| param.name == name.text) {
                    Some(target) => target,
                    None => {
                        let known: Vec<String> =
                            params.iter().map(|param| param.name.clone()).collect();
                        let mut diagnostic = Diagnostic::error(
                            codes::UNKNOWN_ARGUMENT,
                            format!("{kind} `{callee}` has no parameter named `{}`", name.text),
                        )
                        .with_primary(name.span, "unknown parameter");
                        if !known.is_empty() {
                            diagnostic =
                                diagnostic.with_note(format!("parameters: {}", known.join(", ")));
                        }
                        if let Some(suggestion) = closest_match(&name.text, &known) {
                            diagnostic =
                                diagnostic.with_help(format!("did you mean `{suggestion}`?"));
                        }
                        self.error(diagnostic);
                        continue;
                    }
                },
                None => {
                    let target = positional;
                    positional += 1;
                    if target >= params.len() {
                        self.error(
                            Diagnostic::error(
                                codes::ARGUMENT_COUNT_MISMATCH,
                                format!(
                                    "{kind} `{callee}` takes {} argument(s), but more were given",
                                    params.len()
                                ),
                            )
                            .with_primary(arg.span, "unexpected argument"),
                        );
                        continue;
                    }
                    target
                }
            };

            if let Some(previous) = order[target] {
                self.error(
                    Diagnostic::error(
                        codes::UNKNOWN_ARGUMENT,
                        format!("parameter `{}` is given twice", params[target].name),
                    )
                    .with_primary(arg.span, "duplicate argument")
                    .with_secondary(args[previous].span, "first given here"),
                );
                continue;
            }
            order[target] = Some(index);

            let expected = &params[target].ty;
            self.expect_assignable(
                &arg_types[index],
                expected,
                arg.value.span(),
                &format!("parameter `{}`", params[target].name),
            );
        }

        let missing: Vec<String> = params
            .iter()
            .zip(&order)
            .filter(|(_, slot)| slot.is_none())
            .map(|(param, _)| format!("{}: {}", param.name, param.ty))
            .collect();
        if !missing.is_empty() {
            self.error(
                Diagnostic::error(
                    codes::ARGUMENT_COUNT_MISMATCH,
                    format!("{kind} `{callee}` is missing {} argument(s)", missing.len()),
                )
                .with_primary(span, format!("missing: {}", missing.join(", ")))
                .with_help(
                    "arguments may be positional or named, but every parameter must be supplied",
                ),
            );
        }

        order
    }

    fn expect_assignable(&mut self, actual: &Ty, expected: &Ty, span: Span, what: &str) {
        if actual.is_assignable_to(expected) {
            return;
        }
        self.error(
            Diagnostic::error(
                codes::TYPE_MISMATCH,
                format!("{what} expects `{expected}`, found `{actual}`"),
            )
            .with_primary(span, format!("this is `{actual}`")),
        );
    }

    // --- shared helpers ----------------------------------------------------

    fn resolve_params(&mut self, params: &[ParamDecl]) -> Vec<Param> {
        let mut resolved = Vec::new();
        let mut seen: HashMap<String, Span> = HashMap::new();
        for param in params {
            if let Some(previous) = seen.get(&param.name.text).copied() {
                self.duplicate(&param.name.text, "parameter", param.name.span, previous);
                continue;
            }
            seen.insert(param.name.text.clone(), param.name.span);
            let ty = self.resolve_type(&param.ty);
            resolved.push(Param {
                name: param.name.text.clone(),
                ty,
            });
        }
        resolved
    }

    fn resolve_type(&mut self, ty: &TypeExpr) -> Ty {
        match ty {
            TypeExpr::Named(ident) => self.resolve_named_type(ident),
            TypeExpr::List { element, .. } => Ty::list_of(self.resolve_type(element)),
            TypeExpr::Optional { inner, .. } => Ty::optional(self.resolve_type(inner)),
            TypeExpr::Union { options, .. } => Ty::union(
                options
                    .iter()
                    .map(|option| self.resolve_type(option))
                    .collect(),
            ),
        }
    }

    fn resolve_named_type(&mut self, ident: &Ident) -> Ty {
        if let Some(primitive) = Ty::from_primitive_name(&ident.text) {
            return primitive;
        }
        if self.records.contains_key(&ident.text) {
            return Ty::Record(ident.text.clone());
        }
        let mut known: Vec<String> = [
            "string", "int", "float", "bool", "json", "bytes", "text", "markdown", "file",
        ]
        .iter()
        .map(|name| name.to_string())
        .collect();
        known.extend(self.records.keys().cloned());
        let mut diagnostic = Diagnostic::error(
            codes::UNKNOWN_TYPE,
            format!("unknown type `{}`", ident.text),
        )
        .with_primary(ident.span, "not a known type");
        if let Some(suggestion) = closest_match(&ident.text, &known) {
            diagnostic = diagnostic.with_help(format!("did you mean `{suggestion}`?"));
        } else {
            diagnostic = diagnostic.with_note(format!("built-in types: {}", known[..9].join(", ")));
        }
        self.error(diagnostic);
        Ty::Unknown
    }

    fn duplicate(&mut self, name: &str, kind: &str, span: Span, previous: Span) {
        self.error(
            Diagnostic::error(
                codes::DUPLICATE_DECLARATION,
                format!("{kind} `{name}` is declared more than once"),
            )
            .with_primary(span, "duplicate declaration")
            .with_secondary(previous, "first declared here"),
        );
    }
}

// --- free functions --------------------------------------------------------

/// Find every `call X(...)` in a flow whose target names an agent.
fn collect_agent_calls(
    statements: &[Stmt],
    signatures: &BTreeMap<String, AgentSignature>,
    out: &mut Vec<(String, Span)>,
) {
    fn visit_expr(
        expr: &Expr,
        signatures: &BTreeMap<String, AgentSignature>,
        out: &mut Vec<(String, Span)>,
    ) {
        match expr {
            Expr::Call { callee, args, span } => {
                let name = callee.text();
                if signatures.contains_key(&name) {
                    out.push((name, *span));
                }
                for arg in args {
                    visit_expr(&arg.value, signatures, out);
                }
            }
            Expr::Ask { args, .. } => {
                for arg in args {
                    visit_expr(&arg.value, signatures, out);
                }
            }
            Expr::ParallelMap { source, body, .. } => {
                visit_expr(source, signatures, out);
                collect_agent_calls(body, signatures, out);
            }
            Expr::List { items, .. } => {
                for item in items {
                    visit_expr(item, signatures, out);
                }
            }
            Expr::Builtin { args, .. } => {
                for arg in args {
                    visit_expr(arg, signatures, out);
                }
            }
            Expr::FunctionCall { args, .. } => {
                for arg in args {
                    visit_expr(&arg.value, signatures, out);
                }
            }
            Expr::Unary { operand, .. } => visit_expr(operand, signatures, out),
            Expr::Binary { lhs, rhs, .. } => {
                visit_expr(lhs, signatures, out);
                visit_expr(rhs, signatures, out);
            }
            _ => {}
        }
    }

    for statement in statements {
        match statement {
            Stmt::Bind { value, .. }
            | Stmt::StateWrite { value, .. }
            | Stmt::Expr { value, .. }
            | Stmt::Emit { value, .. } => visit_expr(value, signatures, out),
            Stmt::Verify { args, .. } => {
                for arg in args {
                    visit_expr(&arg.value, signatures, out);
                }
            }
            Stmt::If {
                condition,
                then_branch,
                else_branch,
                ..
            } => {
                visit_expr(condition, signatures, out);
                collect_agent_calls(then_branch, signatures, out);
                if let Some(else_branch) = else_branch {
                    collect_agent_calls(else_branch, signatures, out);
                }
            }
            Stmt::Loop { guard, body, .. } => {
                if let Some(guard) = guard {
                    visit_expr(guard, signatures, out);
                }
                collect_agent_calls(body, signatures, out);
            }
            Stmt::Checkpoint { .. } | Stmt::Error { .. } => {}
        }
    }
}

/// The type of a literal expression, or `None` when it is not one.
///
/// One function rather than a separate "is this a literal" predicate and a type
/// inference: the two would have to agree about what counts, and a persistent
/// field's initial value is checked against its declared type immediately
/// afterwards.
///
/// An empty list is `list<unknown>`, which is assignable to any list — the
/// common case, since `= []` is how most persistent collections start.
fn literal_ty(expr: &Expr) -> Option<Ty> {
    match expr {
        Expr::Str(literal) if literal.is_plain() => Some(Ty::String),
        Expr::Int { .. } => Some(Ty::Int),
        Expr::Float { .. } => Some(Ty::Float),
        Expr::Bool { .. } => Some(Ty::Bool),
        Expr::List { items, .. } => {
            let mut element = Ty::Unknown;
            for item in items {
                let item_ty = literal_ty(item)?;
                if element.is_unknown() {
                    element = item_ty;
                } else if !item_ty.is_assignable_to(&element) {
                    // Reported as a type mismatch against the declared element
                    // type by the caller, which has a better span for it.
                    return Some(Ty::Unknown);
                }
            }
            Some(Ty::List(Box::new(element)))
        }
        // An interpolated string is not a literal: it reads a binding, and
        // there is nothing bound when an initial value is resolved.
        _ => None,
    }
}

/// The binding a path expression is rooted at, if it is a path at all.
///
/// `found.body` and `found` both answer `found`. A store root answers nothing:
/// neither kind of memory is what an artifact is emitted from.
fn expr_root_binding(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Path(path) => match &path.root {
            PathRoot::Binding(ident) => Some(ident.text.clone()),
            PathRoot::State { .. } | PathRoot::Memory { .. } => None,
        },
        _ => None,
    }
}

/// The first expression a pure body may not contain.
///
/// `allow_helper_call` is the only difference between a helper body and a
/// verifier body. Language 0.2 §7 forbids helper-to-helper calls, which is what
/// keeps inlining one level deep; a verifier is not a helper, so calling one
/// from a verifier body is still one level and stays allowed.
fn first_impure_expr(expr: &Expr, allow_helper_call: bool) -> Option<Span> {
    match expr {
        Expr::FunctionCall { args, span, .. } => {
            if allow_helper_call {
                args.iter()
                    .find_map(|arg| first_impure_expr(&arg.value, allow_helper_call))
            } else {
                Some(*span)
            }
        }
        Expr::Ask { span, .. } | Expr::Call { span, .. } | Expr::ParallelMap { span, .. } => {
            Some(*span)
        }
        Expr::List { items, .. } => items
            .iter()
            .find_map(|item| first_impure_expr(item, allow_helper_call)),
        Expr::Builtin { args, .. } => args
            .iter()
            .find_map(|arg| first_impure_expr(arg, allow_helper_call)),
        Expr::Unary { operand, .. } => first_impure_expr(operand, allow_helper_call),
        Expr::Binary { lhs, rhs, .. } => first_impure_expr(lhs, allow_helper_call)
            .or_else(|| first_impure_expr(rhs, allow_helper_call)),
        Expr::Str(_)
        | Expr::Int { .. }
        | Expr::Float { .. }
        | Expr::Bool { .. }
        | Expr::Path(_)
        | Expr::Error { .. } => None,
    }
}

/// Whether every path through `statements` emits `output`.
///
/// A bounded loop may run zero times, and a `parallel map` body may not run at
/// all, so neither counts as a guarantee.
fn definitely_emits(statements: &[Stmt], output: &str) -> bool {
    statements.iter().any(|statement| match statement {
        Stmt::Emit { output: name, .. } => name.text == output,
        Stmt::If {
            then_branch,
            else_branch: Some(else_branch),
            ..
        } => definitely_emits(then_branch, output) && definitely_emits(else_branch, output),
        _ => false,
    })
}

/// Lower bound on the number of model/tool/agent calls the flow performs.
fn min_steps(statements: &[Stmt]) -> i64 {
    fn expr_steps(expr: &Expr) -> i64 {
        match expr {
            Expr::Ask { args, .. } => {
                1 + args.iter().map(|arg| expr_steps(&arg.value)).sum::<i64>()
            }
            Expr::Call { args, .. } => {
                1 + args.iter().map(|arg| expr_steps(&arg.value)).sum::<i64>()
            }
            // The source list may be empty, so the body contributes nothing.
            Expr::ParallelMap { source, .. } => expr_steps(source),
            Expr::List { items, .. } => items.iter().map(expr_steps).sum(),
            Expr::Builtin { args, .. } => args.iter().map(expr_steps).sum(),
            Expr::FunctionCall { args, .. } => args.iter().map(|arg| expr_steps(&arg.value)).sum(),
            Expr::Unary { operand, .. } => expr_steps(operand),
            Expr::Binary { lhs, rhs, .. } => expr_steps(lhs) + expr_steps(rhs),
            _ => 0,
        }
    }

    statements
        .iter()
        .map(|statement| match statement {
            Stmt::Bind { value, .. }
            | Stmt::StateWrite { value, .. }
            | Stmt::Expr { value, .. }
            | Stmt::Emit { value, .. } => expr_steps(value),
            Stmt::Verify { args, .. } => args.iter().map(|arg| expr_steps(&arg.value)).sum(),
            Stmt::If {
                condition,
                then_branch,
                else_branch,
                ..
            } => {
                let branches = match else_branch {
                    Some(else_branch) => min_steps(then_branch).min(min_steps(else_branch)),
                    // Without an `else`, the empty path is always available.
                    None => 0,
                };
                expr_steps(condition) + branches
            }
            // A bounded loop may run zero times.
            Stmt::Loop { guard, .. } => guard.as_ref().map(expr_steps).unwrap_or(0),
            Stmt::Checkpoint { .. } | Stmt::Error { .. } => 0,
        })
        .sum()
}

/// Suggest the closest candidate, if one is close enough to be worth printing.
fn closest_match(needle: &str, candidates: &[String]) -> Option<String> {
    let threshold = match needle.len() {
        0..=3 => 1,
        4..=7 => 2,
        _ => 3,
    };
    candidates
        .iter()
        .filter(|candidate| candidate.as_str() != needle)
        .map(|candidate| (edit_distance(needle, candidate), candidate))
        .filter(|(distance, _)| *distance <= threshold)
        .min_by_key(|(distance, candidate)| (*distance, candidate.len()))
        .map(|(_, candidate)| candidate.clone())
}

/// Levenshtein distance over Unicode scalar values.
fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    let mut current = vec![0usize; b.len() + 1];
    for (i, &ai) in a.iter().enumerate() {
        current[0] = i + 1;
        for (j, &bj) in b.iter().enumerate() {
            let cost = usize::from(ai != bj);
            current[j + 1] = (previous[j + 1] + 1)
                .min(current[j] + 1)
                .min(previous[j] + cost);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[b.len()]
}

/// Why a declared reach value cannot be used, or `None` if it can.
///
/// The rules are [Language 0.1 §7.1](../../../specs/language/v0.1.md)'s, applied
/// to the other side of the comparison: a value that means something different
/// on two machines would make containment a check against a moving target.
fn invalid_reach_value(effect: Effect, value: &str) -> Option<&'static str> {
    if value.trim().is_empty() {
        return Some("a reach value may not be empty");
    }
    match effect {
        Effect::FilesystemRead | Effect::FilesystemWrite => {
            if value.starts_with('/') || value.starts_with('\\') || value.contains(':') {
                return Some("a path is relative to the workspace and may not be absolute");
            }
            if value.split(['/', '\\']).any(|segment| segment == "..") {
                return Some("a path may not climb out of the workspace with `..`");
            }
            None
        }
        Effect::Network => {
            if value.contains('*') {
                return Some("a host is matched exactly; there are no wildcards");
            }
            if value.contains('/') || value.contains(' ') {
                return Some("a host is a host name, not a URL");
            }
            None
        }
        // Refused earlier, by `takes_reach`.
        _ => None,
    }
}

#[cfg(test)]
mod unit {
    use super::{closest_match, edit_distance};

    #[test]
    fn edit_distance_matches_known_values() {
        assert_eq!(edit_distance("kitten", "sitting"), 3);
        assert_eq!(edit_distance("", "abc"), 3);
        assert_eq!(edit_distance("same", "same"), 0);
    }

    #[test]
    fn suggestions_stay_within_a_length_aware_threshold() {
        let candidates = vec!["network".to_string(), "filesystem_write".to_string()];
        assert_eq!(
            closest_match("netwrok", &candidates),
            Some("network".to_string())
        );
        assert_eq!(closest_match("completely_different", &candidates), None);
    }

    #[test]
    fn a_name_never_suggests_itself() {
        let candidates = vec!["topic".to_string()];
        assert_eq!(closest_match("topic", &candidates), None);
    }
}
