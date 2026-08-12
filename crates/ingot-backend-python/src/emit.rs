//! Lowering an Agent IR document to Python source.
//!
//! This is a **compiler backend**, not a Python interpreter for the IR. A
//! `branch` becomes an `if`, a `loop` becomes a `while`, a binding becomes a
//! local variable, and the emitted file contains no IR document at all. That
//! distinction is the whole test: embedding the JSON and interpreting it at run
//! time would let every field we did not understand pass through untouched,
//! which is exactly the failure a second backend exists to catch.
//!
//! What it will not do is guess. A node kind, a value kind, an operator or a
//! builtin this backend does not implement produces an [`EmitError`] rather than
//! a comment in the output — [Runtime 0.1 §2] applied to compilation.
//!
//! [Runtime 0.1 §2]: https://github.com/mathissdupont/ingot/blob/main/specs/runtime/v0.1.md

use std::collections::BTreeMap;
use std::fmt;

use ingot_ir::{AgentIr, ModelRequirement, Node, NodeKind, RefScope, TemplatePart, Value};

/// The prelude every generated program carries.
///
/// Written from the specification rather than from `ingot-runtime`; see RFC-0006.
/// Inlined rather than shipped beside the output, so that what `ingot build`
/// produces is one file somebody can run without installing anything.
const PRELUDE: &str = include_str!("prelude.py");

/// The IR major version this backend implements.
const SUPPORTED_IR_MAJOR: &str = "0";

/// Why an artifact could not be lowered to Python.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmitError {
    /// A node kind this backend does not implement. Refused, never skipped.
    UnsupportedNode { node: String, kind: String },
    /// A value form, operator or builtin this backend does not implement.
    UnsupportedValue { what: String },
    /// The artifact is internally inconsistent.
    Malformed(String),
    /// The IR major version is not this backend's.
    UnsupportedIrVersion { found: String },
}

impl fmt::Display for EmitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EmitError::UnsupportedNode { node, kind } => write!(
                f,
                "node `{node}` is a `{kind}`, which the python target does not implement\n  \
                 refusing to emit a program that would silently skip it; \
                 `ingot build --target python` reports every such construct before it builds"
            ),
            EmitError::UnsupportedValue { what } => write!(
                f,
                "the python target does not implement {what}\n  \
                 this is a gap in the backend rather than in the artifact"
            ),
            EmitError::Malformed(message) => write!(f, "the artifact is malformed: {message}"),
            EmitError::UnsupportedIrVersion { found } => write!(
                f,
                "this artifact declares IR version `{found}`; the python target implements \
                 major `{SUPPORTED_IR_MAJOR}`. Refusing it rather than ignoring the parts it \
                 does not understand."
            ),
        }
    }
}

impl std::error::Error for EmitError {}

/// Emit a self-contained Python program for one agent.
pub fn emit(ir: &AgentIr) -> Result<String, EmitError> {
    let major = ir.ir_version.split('.').next().unwrap_or_default();
    if major != SUPPORTED_IR_MAJOR {
        return Err(EmitError::UnsupportedIrVersion {
            found: ir.ir_version.clone(),
        });
    }

    let nodes: BTreeMap<&str, &Node> = ir
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect();

    let mut emitter = Emitter {
        ir,
        nodes,
        out: String::new(),
        loops: 0,
    };

    // An agent with no flow emits `pass`: `region` does that for any empty
    // region, and the top level is one.
    emitter.region(ir.entry.as_deref(), 1)?;
    let body = std::mem::take(&mut emitter.out);
    Ok(assemble(ir, &body))
}

struct Emitter<'a> {
    ir: &'a AgentIr,
    nodes: BTreeMap<&'a str, &'a Node>,
    out: String,
    /// Distinguishes nested loop variables without relying on node ids being
    /// valid Python identifiers.
    loops: usize,
}

impl Emitter<'_> {
    /// Emit a region: the node at `start`, then its `next`, until the chain ends.
    fn region(&mut self, start: Option<&str>, depth: usize) -> Result<(), EmitError> {
        let mut current = start.map(str::to_string);
        let mut emitted = 0usize;
        while let Some(id) = current {
            let node = *self
                .nodes
                .get(id.as_str())
                .ok_or_else(|| EmitError::Malformed(format!("no node with id `{id}`")))?;
            self.node(node, depth)?;
            emitted += 1;
            current = node.next.clone();
        }
        if emitted == 0 {
            // An empty arm is legal and Python needs a statement for it.
            self.line(depth, "pass");
        }
        Ok(())
    }

    fn node(&mut self, node: &Node, depth: usize) -> Result<(), EmitError> {
        let id = &node.id;
        self.blank();
        self.line(depth, &format!("# {id}  {}", node.kind.as_str()));
        self.line(
            depth,
            &format!("rt.node({}, {})", quote(id), quote(node.kind.as_str())),
        );

        match node.kind {
            NodeKind::LlmCall => self.llm_call(node, depth),
            NodeKind::Branch => self.branch(node, depth),
            NodeKind::Loop => self.loop_node(node, depth),
            NodeKind::Parallel => self.parallel(node, depth),
            NodeKind::Approval => self.approval(node, depth),
            NodeKind::StateRead => self.state_read(node, depth),
            NodeKind::StateWrite => self.state_write(node, depth),
            NodeKind::ArtifactEmit => self.artifact_emit(node, depth),
            NodeKind::Verify => self.verify(node, depth),
            NodeKind::Checkpoint => {
                let label = node.label.clone().unwrap_or_default();
                self.line(
                    depth,
                    &format!("rt.checkpoint({}, {})", quote(id), quote(&label)),
                );
                Ok(())
            }
            // Refused rather than skipped. `report::analyse` already told the
            // operator this would happen; reaching here means they were told and
            // the build proceeded anyway, so the failure has to be loud.
            NodeKind::ToolCall | NodeKind::AgentCall => Err(EmitError::UnsupportedNode {
                node: id.clone(),
                kind: node.kind.as_str().to_string(),
            }),
        }
    }

    fn llm_call(&mut self, node: &Node, depth: usize) -> Result<(), EmitError> {
        let response_type = node
            .response_type
            .clone()
            .ok_or_else(|| EmitError::Malformed(format!("`{}` has no responseType", node.id)))?;
        let prompt = node
            .prompt
            .as_ref()
            .ok_or_else(|| EmitError::Malformed(format!("`{}` has no prompt", node.id)))?;

        // `system` is forwarded; `temperature` and `max_tokens` deliberately are
        // not. Provider support for them varies, and an artifact that behaves
        // differently per provider for reasons it never mentions is the opposite
        // of portable. Everything else named is context.
        let mut system = "None".to_string();
        let mut context: Vec<String> = Vec::new();
        for argument in &node.args {
            match argument.name.as_str() {
                "system" => system = self.value(&argument.value)?,
                "temperature" | "max_tokens" => {}
                name => {
                    let rendered = self.value(&argument.value)?;
                    context.push(format!("({}, {rendered})", quote(name)));
                }
            }
        }

        self.line(depth, &format!("{} = rt.ask(", binding_of(node)));
        self.line(depth + 1, &format!("node={},", quote(&node.id)));
        self.line(depth + 1, "effects=[\"model_access\"],");
        self.line(depth + 1, &format!("prompt={},", self.value(prompt)?));
        self.line(
            depth + 1,
            &format!("response_type={},", quote(&response_type)),
        );
        // No `max_tokens` here on purpose. Runtime 0.3 §4: the ceiling belongs
        // to the transport, and an artifact must not be able to select one. The
        // runtime asks the provider at the call.
        self.line(depth + 1, &format!("context=[{}],", context.join(", ")));
        self.line(depth + 1, &format!("system={system},"));
        self.line(depth, ")");
        Ok(())
    }

    fn branch(&mut self, node: &Node, depth: usize) -> Result<(), EmitError> {
        let condition = node
            .condition
            .as_ref()
            .ok_or_else(|| EmitError::Malformed(format!("`{}` has no condition", node.id)))?;
        let rendered = self.value(condition)?;

        self.line(depth, &format!("if {rendered}:"));
        self.line(
            depth + 1,
            &format!("rt.branch({}, \"then\")", quote(&node.id)),
        );
        self.region(node.then.as_deref(), depth + 1)?;
        self.line(depth, "else:");
        self.line(
            depth + 1,
            &format!("rt.branch({}, \"else\")", quote(&node.id)),
        );
        self.region(node.otherwise.as_deref(), depth + 1)?;
        Ok(())
    }

    /// A `verify`, which is a `branch`'s condition without the branching.
    ///
    /// The check is inlined into the artifact, so this renders the same value
    /// form `branch` renders and hands the answer to the runtime. A node with
    /// no `condition` is a verifier declared without a body: it reports
    /// `notPerformed`, and must not report `passed`.
    fn verify(&mut self, node: &Node, depth: usize) -> Result<(), EmitError> {
        let verifier = node
            .verifier
            .as_ref()
            .ok_or_else(|| EmitError::Malformed(format!("`{}` names no verifier", node.id)))?;

        // Arguments are not rendered. Anything in one that does work — a state
        // read, a call — was hoisted into its own node before this one, so what
        // is left is refs and literals, and the condition already names the
        // ones the check depends on.
        match &node.condition {
            Some(condition) => {
                let rendered = self.value(condition)?;
                self.line(
                    depth,
                    &format!(
                        "rt.verify({}, {}, {rendered})",
                        quote(&node.id),
                        quote(verifier)
                    ),
                );
            }
            None => self.line(
                depth,
                &format!("rt.not_verified({}, {})", quote(&node.id), quote(verifier)),
            ),
        }
        Ok(())
    }

    fn loop_node(&mut self, node: &Node, depth: usize) -> Result<(), EmitError> {
        // The static bound is enforced here, independently of the guard, so a
        // guard that never falsifies still terminates. IR 0.1 §4 requires this
        // of every backend rather than trusting the guard.
        let max = node.max_iterations.unwrap_or(0).max(0);
        self.loops += 1;
        let variable = format!("_i{}", self.loops);

        self.line(depth, &format!("for {variable} in range({max}):"));
        if let Some(guard) = &node.guard {
            let rendered = self.value(guard)?;
            self.line(depth + 1, &format!("if not ({rendered}):"));
            self.line(depth + 2, "break");
        }
        self.line(
            depth + 1,
            &format!("rt.iteration({}, {variable})", quote(&node.id)),
        );
        self.region(node.body.as_deref(), depth + 1)?;
        Ok(())
    }

    fn parallel(&mut self, node: &Node, depth: usize) -> Result<(), EmitError> {
        let mode = node.mode.as_deref().unwrap_or("map");
        if mode != "map" {
            return Err(EmitError::UnsupportedValue {
                what: format!("`parallel {mode}`; IR 0.1 has only `map`"),
            });
        }
        let source = node
            .source
            .as_ref()
            .ok_or_else(|| EmitError::Malformed(format!("`{}` has no source", node.id)))?;
        let binder = node
            .binder
            .as_ref()
            .ok_or_else(|| EmitError::Malformed(format!("`{}` has no binder", node.id)))?;
        let body = node
            .body
            .as_deref()
            .ok_or_else(|| EmitError::Malformed(format!("`{}` has no body", node.id)))?;

        // Sequential, like the reference interpreter, and for the same reason:
        // the compiler guarantees a `parallel` body writes no state, emits
        // nothing and holds no checkpoint (ING6005), so iterations cannot
        // observe one another and only the wall clock differs.
        self.loops += 1;
        let index = format!("_i{}", self.loops);
        let collected = format!("_out{}", self.loops);
        let items = format!("_src{}", self.loops);

        let rendered = self.value(source)?;
        self.line(depth, &format!("{items} = {rendered}"));
        self.line(depth, &format!("{collected} = []"));
        self.line(depth, &format!("for {index} in range(len({items})):"));
        self.line(
            depth + 1,
            &format!("rt.element({}, {index}, len({items}))", quote(&node.id)),
        );
        self.line(depth + 1, &format!("{} = {items}[{index}]", local(binder)));
        self.region(Some(body), depth + 1)?;

        // IR 0.1 §4.2: the value of an iteration is the result of the last node
        // in the body, and the container's binding receives the list of those.
        let last = self.last_of_region(body)?;
        let value = match binding_name(last) {
            Some(_) => binding_of(last),
            None => "None".to_string(),
        };
        self.line(depth + 1, &format!("{collected}.append({value})"));
        self.line(depth, &format!("{} = {collected}", binding_of(node)));
        Ok(())
    }

    fn approval(&mut self, node: &Node, depth: usize) -> Result<(), EmitError> {
        let reason = node.label.clone().unwrap_or_default();
        let effects: Vec<String> = node.effects.iter().map(|effect| quote(effect)).collect();
        self.line(
            depth,
            &format!(
                "rt.approve({}, [{}], {})",
                quote(&node.id),
                effects.join(", "),
                quote(&reason)
            ),
        );
        Ok(())
    }

    fn state_read(&mut self, node: &Node, depth: usize) -> Result<(), EmitError> {
        let field = node
            .field
            .as_ref()
            .ok_or_else(|| EmitError::Malformed(format!("`{}` has no field", node.id)))?;
        self.line(
            depth,
            &format!(
                "{} = rt.{}({}, {})",
                binding_of(node),
                reader_for(node),
                quote(&node.id),
                quote(field)
            ),
        );
        Ok(())
    }

    fn state_write(&mut self, node: &Node, depth: usize) -> Result<(), EmitError> {
        let field = node
            .field
            .as_ref()
            .ok_or_else(|| EmitError::Malformed(format!("`{}` has no field", node.id)))?;
        let value = node
            .value
            .as_ref()
            .ok_or_else(|| EmitError::Malformed(format!("`{}` has no value", node.id)))?;
        let rendered = self.value(value)?;
        self.line(
            depth,
            &format!(
                "rt.{}({}, {}, {rendered})",
                writer_for(node),
                quote(&node.id),
                quote(field)
            ),
        );
        Ok(())
    }

    fn artifact_emit(&mut self, node: &Node, depth: usize) -> Result<(), EmitError> {
        let output = node
            .output
            .as_ref()
            .ok_or_else(|| EmitError::Malformed(format!("`{}` has no output", node.id)))?;
        let value = node
            .value
            .as_ref()
            .ok_or_else(|| EmitError::Malformed(format!("`{}` has no value", node.id)))?;
        let declared = self.ir.outputs.get(output).ok_or_else(|| {
            EmitError::Malformed(format!("`{}` emits undeclared output `{output}`", node.id))
        })?;
        let rendered = self.value(value)?;
        self.line(
            depth,
            &format!(
                "rt.emit({}, {}, {}, {rendered})",
                quote(&node.id),
                quote(output),
                quote(&content_type(declared))
            ),
        );
        Ok(())
    }

    /// The last node of a region, for `parallel`'s iteration value.
    fn last_of_region(&self, start: &str) -> Result<&Node, EmitError> {
        let mut current = start.to_string();
        loop {
            let node = *self
                .nodes
                .get(current.as_str())
                .ok_or_else(|| EmitError::Malformed(format!("no node with id `{current}`")))?;
            match &node.next {
                Some(next) => current = next.clone(),
                None => return Ok(node),
            }
        }
    }

    // --- values -----------------------------------------------------------

    fn value(&self, value: &Value) -> Result<String, EmitError> {
        match value {
            Value::Literal { value, .. } => Ok(python_literal(value)),
            Value::Ref { scope, path } => self.reference(*scope, path),
            Value::List { items } => {
                let rendered: Result<Vec<String>, EmitError> =
                    items.iter().map(|item| self.value(item)).collect();
                Ok(format!("[{}]", rendered?.join(", ")))
            }
            Value::Template { parts } => {
                let mut rendered: Vec<String> = Vec::new();
                for part in parts {
                    match part {
                        TemplatePart::Text { value } => {
                            rendered.push(format!("(\"text\", {})", quote(value)));
                        }
                        TemplatePart::Value { value, ty } => {
                            rendered.push(format!(
                                "(\"value\", {}, {})",
                                self.value(value)?,
                                quote(ty)
                            ));
                        }
                    }
                }
                Ok(format!("render([{}])", rendered.join(", ")))
            }
            Value::Unary { op, operand } => {
                let inner = self.value(operand)?;
                match op.as_str() {
                    "!" => Ok(format!("(not {inner})")),
                    "-" => Ok(format!("(-{inner})")),
                    other => Err(EmitError::UnsupportedValue {
                        what: format!("the unary operator `{other}`"),
                    }),
                }
            }
            Value::Binary { op, lhs, rhs } => {
                let left = self.value(lhs)?;
                let right = self.value(rhs)?;
                // Python's operators mean the same thing for these, with two
                // renames. Deliberately no `is`/`==` subtlety: Ingot compares
                // values, and so does Python's `==`.
                let python = match op.as_str() {
                    "==" | "!=" | "<" | "<=" | ">" | ">=" | "+" | "-" | "*" | "/" => op.as_str(),
                    "&&" => "and",
                    "||" => "or",
                    other => {
                        return Err(EmitError::UnsupportedValue {
                            what: format!("the binary operator `{other}`"),
                        })
                    }
                };
                Ok(format!("({left} {python} {right})"))
            }
            Value::Builtin { name, args } => {
                let rendered: Result<Vec<String>, EmitError> =
                    args.iter().map(|arg| self.value(arg)).collect();
                let rendered = rendered?;
                match name.as_str() {
                    "len" => Ok(format!("len({})", rendered.join(", "))),
                    other => Err(EmitError::UnsupportedValue {
                        what: format!("the builtin `{other}`"),
                    }),
                }
            }
            // IR 0.1 §5: `unknown` appears only where the source failed to
            // check, so a successful build never carries one and a backend may
            // treat it as a hard error.
            Value::Unknown => Err(EmitError::Malformed(
                "the artifact contains an `unknown` value, which only a failed build emits"
                    .to_string(),
            )),
        }
    }

    fn reference(&self, scope: RefScope, path: &[String]) -> Result<String, EmitError> {
        let (root, fields) = path
            .split_first()
            .ok_or_else(|| EmitError::Malformed("a reference with an empty path".to_string()))?;

        let mut expression = match scope {
            RefScope::Input => format!("rt.inputs[{}]", quote(root)),
            RefScope::Binding => local(root),
            // A store read is a node in IR 0.1 (§4.4), so a `state` or
            // `memory` reference here is one the compiler should have lowered.
            // Refusing beats reading a dict that may never have been written.
            RefScope::State | RefScope::Memory => {
                let store = match scope {
                    RefScope::Memory => "memory",
                    _ => "state",
                };
                return Err(EmitError::UnsupportedValue {
                    what: format!(
                        "an inline `{store}.{root}` reference; IR 0.1 lowers store reads to \
                         `state.read` nodes"
                    ),
                });
            }
        };
        for field in fields {
            expression.push_str(&format!("[{}]", quote(field)));
        }
        Ok(expression)
    }

    // --- output ------------------------------------------------------------

    fn line(&mut self, depth: usize, text: &str) {
        for _ in 0..depth {
            self.out.push_str("    ");
        }
        self.out.push_str(text);
        self.out.push('\n');
    }

    fn blank(&mut self) {
        if !self.out.is_empty() && !self.out.ends_with("\n\n") {
            self.out.push('\n');
        }
    }
}

/// The local variable a node's result lives in.
///
/// A `state.read` binding is spelled `$state.<field>` (IR 0.1 §4.4), which is
/// deliberately not a source-level name and is deliberately not a Python one
/// either, so it gets mangled the same way every time.
fn binding_of(node: &Node) -> String {
    match binding_name(node) {
        Some(name) => local(name),
        // A call whose result nothing reads still has to be made, so the value
        // goes to Python's conventional throwaway rather than nowhere.
        None => "_".to_string(),
    }
}

fn binding_name(node: &Node) -> Option<&str> {
    node.binding.as_deref()
}

/// Which of the runtime's two readers a `state.read` node calls.
fn reader_for(node: &Node) -> &'static str {
    match node.scope {
        Some(RefScope::Memory) => "memory_read",
        _ => "state_read",
    }
}

fn writer_for(node: &Node) -> &'static str {
    match node.scope {
        Some(RefScope::Memory) => "memory_write",
        _ => "state_write",
    }
}

/// A binding name as a Python identifier.
///
/// Prefixed rather than used directly: an Ingot binding may be spelled `class`
/// or `lambda`, and `$state.x` is not an identifier at all.
fn local(name: &str) -> String {
    let mangled: String = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    format!("b_{mangled}")
}

/// `artifact<markdown>` and `markdown` both mean markdown content.
fn content_type(declared: &str) -> String {
    declared
        .strip_prefix("artifact<")
        .and_then(|rest| rest.strip_suffix('>'))
        .unwrap_or(declared)
        .to_string()
}

/// A JSON value as a Python expression.
///
/// Python's `True`/`False`/`None` differ from JSON's, and nothing else does —
/// Python's number and string literal syntax is a superset of JSON's for the
/// values `serde_json` produces.
fn python_literal(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => "None".to_string(),
        serde_json::Value::Bool(true) => "True".to_string(),
        serde_json::Value::Bool(false) => "False".to_string(),
        serde_json::Value::Number(number) => number.to_string(),
        serde_json::Value::String(text) => quote(text),
        serde_json::Value::Array(items) => format!(
            "[{}]",
            items
                .iter()
                .map(python_literal)
                .collect::<Vec<String>>()
                .join(", ")
        ),
        serde_json::Value::Object(fields) => format!(
            "{{{}}}",
            fields
                .iter()
                .map(|(key, value)| format!("{}: {}", quote(key), python_literal(value)))
                .collect::<Vec<String>>()
                .join(", ")
        ),
    }
}

/// A Python string literal.
///
/// Produced through `serde_json`, whose escaping Python's parser accepts for
/// every sequence it emits. Doing it by hand is how a backslash or a newline in
/// a prompt turns into a syntax error in the generated file.
fn quote(text: &str) -> String {
    serde_json::to_string(text).expect("a string is always serialisable")
}

/// The generated file: the prelude, the artifact's declarations, the flow.
fn assemble(ir: &AgentIr, body: &str) -> String {
    let mut out = String::new();

    out.push_str("#!/usr/bin/env python3\n");
    out.push_str(&format!(
        "# Generated by the ingot python backend from {}.\n",
        ir.agent
    ));
    out.push_str("# Do not edit: rebuild with `ingot build --target python`.\n");
    out.push_str(&format!(
        "#\n# Agent IR {} (language {}) -> Python 3, standard library only.\n",
        ir.ir_version, ir.language
    ));
    out.push_str(
        "# This program enforces the artifact's policy and budgets itself. It does not\n\
         # trust the compiler that produced it: whoever runs an artifact is frequently\n\
         # not whoever built it (Runtime 0.1 §7).\n",
    );
    if let Some(doc) = &ir.doc {
        out.push_str("#\n");
        for line in doc.lines() {
            out.push_str(&format!("# {line}\n"));
        }
    }
    out.push('\n');

    out.push_str(PRELUDE.trim_end());
    out.push_str("\n\n\n");

    out.push_str(
        "# --- this agent ---------------------------------------------------------------\n\n",
    );
    out.push_str(&format!("AGENT = {}\n", quote(&ir.agent)));
    out.push_str(&format!("IR_VERSION = {}\n", quote(&ir.ir_version)));
    out.push_str(&format!("INPUTS = {}\n", py_map(&ir.inputs)));
    out.push_str(&format!(
        "OUTPUTS = {}\n",
        py_map(
            &ir.outputs
                .iter()
                .map(|(name, ty)| (name.clone(), content_type(ty)))
                .collect()
        )
    ));
    out.push_str(&format!("PERSISTENT = {}\n", py_json(&ir.persistent)));
    out.push_str(&format!("TYPES = {}\n", py_json(&ir.types)));
    out.push_str(&format!("POLICY = {}\n", py_json(&ir.policy)));
    out.push_str(&format!("BUDGET = {}\n", py_json(&budget(ir))));
    out.push_str(&format!("MODEL_REFERENCE = {}\n", model_reference(ir)));
    out.push_str("\n\n");

    out.push_str("def flow(rt):\n");
    out.push_str(body);
    out.push_str("\n\n");

    out.push_str("if __name__ == \"__main__\":\n");
    out.push_str("    raise SystemExit(\n");
    out.push_str("        main(\n");
    out.push_str("            agent=AGENT,\n");
    out.push_str("            ir_version=IR_VERSION,\n");
    out.push_str("            declared_inputs=INPUTS,\n");
    out.push_str("            declared_outputs=OUTPUTS,\n");
    out.push_str("            declared_persistent=PERSISTENT,\n");
    out.push_str("            types=TYPES,\n");
    out.push_str("            policy=POLICY,\n");
    out.push_str("            budget=BUDGET,\n");
    out.push_str("            model_reference=MODEL_REFERENCE,\n");
    out.push_str("            flow=flow,\n");
    out.push_str("        )\n");
    out.push_str("    )\n");
    out
}

/// The step ceiling and the token limit, as the artifact states them.
fn budget(ir: &AgentIr) -> serde_json::Value {
    let mut budget = serde_json::Map::new();
    if let Some(steps) = ir.budget.steps.filter(|steps| *steps >= 0) {
        budget.insert("maxSteps".to_string(), serde_json::Value::from(steps));
    }
    if let Some(tokens) = ir.budget.tokens.filter(|tokens| *tokens >= 0) {
        budget.insert("tokens".to_string(), serde_json::Value::from(tokens));
    }
    serde_json::Value::Object(budget)
}

/// The `vendor/model` reference, when the artifact pinned one.
fn model_reference(ir: &AgentIr) -> String {
    match &ir.requirements.model {
        ModelRequirement::Exact { reference } => quote(reference),
        _ => "None".to_string(),
    }
}

fn py_map(map: &BTreeMap<String, String>) -> String {
    if map.is_empty() {
        return "{}".to_string();
    }
    let entries: Vec<String> = map
        .iter()
        .map(|(key, value)| format!("{}: {}", quote(key), quote(value)))
        .collect();
    format!("{{{}}}", entries.join(", "))
}

/// Declarative data as a Python literal, through JSON.
///
/// Sorted by construction — the IR uses `BTreeMap` — so the generated file is
/// byte-identical on every platform, which is what makes it reviewable.
fn py_json<T: serde::Serialize>(value: &T) -> String {
    let json = serde_json::to_value(value).expect("IR data is always serialisable");
    python_literal(&json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ingot_ir::node::Argument;

    fn agent(source: &str) -> AgentIr {
        AgentIr::from_json(source).expect("the fixture must parse")
    }

    fn bare() -> AgentIr {
        agent(
            r#"{"irVersion":"0.1","language":"0.1","agent":"p.A","inputs":{},"outputs":{},
                "types":{},"requirements":{"model":{"mode":"unspecified"}},"tools":[],
                "state":{},"budget":{},"policy":{},"effects":[],"nodes":[]}"#,
        )
    }

    /// A one-node program, so a test can assert one construct at a time.
    fn with(node: Node) -> AgentIr {
        let mut ir = bare();
        ir.entry = Some(node.id.clone());
        ir.nodes.push(node);
        ir
    }

    #[test]
    fn an_empty_flow_still_produces_a_runnable_program() {
        let text = emit(&bare()).unwrap();
        assert!(text.contains("def flow(rt):"), "{text}");
        assert!(text.contains("    pass"), "{text}");
        assert!(text.starts_with("#!/usr/bin/env python3\n"), "{text}");
    }

    #[test]
    fn a_branch_becomes_a_real_if_rather_than_an_interpreted_one() {
        // The point of a compiler backend: the output has no IR in it.
        let mut branch = Node::new("n2", NodeKind::Branch);
        branch.condition = Some(Value::bool(true));
        branch.then = Some("n0".to_string());
        branch.otherwise = Some("n1".to_string());

        let mut ir = with(branch);
        ir.nodes.push(Node::new("n0", NodeKind::Checkpoint));
        ir.nodes.push(Node::new("n1", NodeKind::Checkpoint));

        let text = emit(&ir).unwrap();
        assert!(text.contains("if True:"), "{text}");
        assert!(text.contains("else:"), "{text}");
        assert!(
            !text.contains("\"kind\": \"branch\""),
            "the IR must not be embedded:\n{text}"
        );
    }

    #[test]
    fn a_loop_bounds_itself_independently_of_its_guard() {
        // IR 0.1 §4: the static bound is every backend's job, not the guard's.
        let mut node = Node::new("n0", NodeKind::Loop);
        node.max_iterations = Some(7);
        node.guard = Some(Value::bool(true));
        node.body = Some("n1".to_string());

        let mut ir = with(node);
        ir.nodes.push(Node::new("n1", NodeKind::Checkpoint));

        let text = emit(&ir).unwrap();
        assert!(text.contains("range(7)"), "{text}");
        assert!(text.contains("if not (True):"), "{text}");
        assert!(text.contains("break"), "{text}");
    }

    #[test]
    fn a_missing_bound_is_zero_iterations_rather_than_unbounded() {
        let mut node = Node::new("n0", NodeKind::Loop);
        node.body = Some("n1".to_string());
        let mut ir = with(node);
        ir.nodes.push(Node::new("n1", NodeKind::Checkpoint));
        assert!(emit(&ir).unwrap().contains("range(0)"));
    }

    #[test]
    fn a_map_collects_the_last_value_of_each_iteration() {
        // IR 0.1 §4.2, stated normatively and easy to get wrong.
        let mut node = Node::new("n0", NodeKind::Parallel);
        node.mode = Some("map".to_string());
        node.binder = Some("hit".to_string());
        node.binding = Some("titles".to_string());
        node.source = Some(Value::List {
            items: vec![Value::string("a")],
        });
        node.body = Some("n1".to_string());

        let mut inner = Node::new("n1", NodeKind::StateRead);
        inner.field = Some("x".to_string());
        inner.binding = Some("$state.x".to_string());

        let mut ir = with(node);
        ir.nodes.push(inner);

        let text = emit(&ir).unwrap();
        assert!(text.contains("b_hit = _src1[_i1]"), "{text}");
        assert!(text.contains(".append(b__state_x)"), "{text}");
        assert!(text.contains("b_titles = _out1"), "{text}");
    }

    #[test]
    fn a_node_kind_this_target_cannot_do_is_refused_not_commented_out() {
        for kind in [NodeKind::ToolCall, NodeKind::AgentCall] {
            let error = emit(&with(Node::new("n0", kind))).unwrap_err();
            let text = error.to_string();
            assert!(text.contains(kind.as_str()), "{text}");
            assert!(text.contains("silently skip"), "{text}");
        }
    }

    #[test]
    fn a_verify_with_a_condition_renders_the_check_in_place() {
        let mut node = Node::new("n0", NodeKind::Verify);
        node.verifier = Some("MinSources".to_string());
        node.condition = Some(Value::Binary {
            op: ">=".to_string(),
            lhs: Box::new(Value::Builtin {
                name: "len".to_string(),
                args: vec![Value::Ref {
                    scope: ingot_ir::RefScope::Binding,
                    path: vec!["found".to_string(), "sources".to_string()],
                }],
            }),
            rhs: Box::new(Value::int(3)),
        });
        let code = emit(&with(node)).expect("a check with a body is executable");
        assert!(code.contains("rt.verify("), "{code}");
        assert!(code.contains("\"MinSources\""), "{code}");
        // The check itself, not a call out to something that holds it.
        assert!(code.contains("len("), "{code}");
    }

    #[test]
    fn a_verify_without_a_condition_reports_not_performed() {
        let mut node = Node::new("n0", NodeKind::Verify);
        node.verifier = Some("CitationCheck".to_string());
        let code = emit(&with(node)).expect("a bodyless verifier still runs");
        assert!(code.contains("rt.not_verified("), "{code}");
        assert!(
            !code.contains("rt.verify("),
            "a check nothing performed must not look like one that passed: {code}"
        );
    }

    #[test]
    fn an_unknown_value_is_a_hard_error() {
        // IR 0.1 §5: only a failed build emits one.
        let mut node = Node::new("n0", NodeKind::StateWrite);
        node.field = Some("x".to_string());
        node.value = Some(Value::Unknown);
        let error = emit(&with(node)).unwrap_err();
        assert!(error.to_string().contains("failed build"), "{error}");
    }

    #[test]
    fn an_ir_major_this_backend_does_not_implement_is_refused() {
        let mut ir = bare();
        ir.ir_version = "1.0".to_string();
        let error = emit(&ir).unwrap_err();
        assert!(error.to_string().contains("Refusing"), "{error}");
    }

    #[test]
    fn a_binding_that_is_not_a_python_identifier_is_mangled_consistently() {
        // `$state.<field>` is deliberately not a source name, and is not a
        // Python one either.
        assert_eq!(local("$state.notes"), "b__state_notes");
        assert_eq!(local("class"), "b_class");
        assert_eq!(local("plain"), "b_plain");
    }

    #[test]
    fn a_prompt_containing_a_quote_or_a_backslash_is_still_valid_python() {
        let mut node = Node::new("n0", NodeKind::LlmCall);
        node.response_type = Some("markdown".to_string());
        node.binding = Some("out".to_string());
        node.prompt = Some(Value::string("say \"hi\"\\ and a\nnewline"));

        let mut ir = with(node);
        ir.outputs.insert("x".to_string(), "markdown".to_string());

        let text = emit(&ir).unwrap();
        assert!(text.contains(r#""say \"hi\"\\ and a\nnewline""#), "{text}");
    }

    #[test]
    fn tuning_hints_are_not_forwarded_and_everything_else_is_context() {
        // Provider support for temperature varies, and an artifact that behaves
        // differently per provider for reasons it never mentions is the opposite
        // of portable.
        let mut node = Node::new("n0", NodeKind::LlmCall);
        node.response_type = Some("markdown".to_string());
        node.binding = Some("out".to_string());
        node.prompt = Some(Value::string("hello"));
        node.args = vec![
            Argument {
                name: "temperature".to_string(),
                value: Value::int(1),
            },
            Argument {
                name: "max_tokens".to_string(),
                value: Value::int(99),
            },
            Argument {
                name: "system".to_string(),
                value: Value::string("Be brief"),
            },
            Argument {
                name: "entries".to_string(),
                value: Value::List { items: vec![] },
            },
        ];

        let text = emit(&with(node)).unwrap();
        assert!(text.contains("system=\"Be brief\""), "{text}");
        assert!(text.contains("context=[(\"entries\", [])]"), "{text}");
        assert!(!text.contains("temperature"), "{text}");
        assert!(!text.contains("99"), "{text}");
    }

    #[test]
    fn operators_map_onto_pythons_own() {
        let ir = bare();
        let emitter = Emitter {
            ir: &ir,
            nodes: BTreeMap::new(),
            out: String::new(),
            loops: 0,
        };
        let both = |op: &str| Value::Binary {
            op: op.to_string(),
            lhs: Box::new(Value::int(1)),
            rhs: Box::new(Value::int(2)),
        };
        assert_eq!(emitter.value(&both(">=")).unwrap(), "(1 >= 2)");
        assert_eq!(emitter.value(&both("&&")).unwrap(), "(1 and 2)");
        assert_eq!(emitter.value(&both("||")).unwrap(), "(1 or 2)");
        assert_eq!(
            emitter
                .value(&Value::Unary {
                    op: "!".to_string(),
                    operand: Box::new(Value::bool(false))
                })
                .unwrap(),
            "(not False)"
        );
        assert_eq!(
            emitter
                .value(&Value::Builtin {
                    name: "len".to_string(),
                    args: vec![Value::List { items: vec![] }]
                })
                .unwrap(),
            "len([])"
        );

        let error = emitter.value(&both("**")).unwrap_err();
        assert!(error.to_string().contains("`**`"), "{error}");
    }

    #[test]
    fn an_artifact_type_loses_its_wrapper_for_the_content_type() {
        assert_eq!(content_type("artifact<markdown>"), "markdown");
        assert_eq!(content_type("markdown"), "markdown");
        assert_eq!(content_type("string"), "string");
    }

    #[test]
    fn the_program_carries_the_policy_it_has_to_enforce() {
        let mut ir = bare();
        ir.policy.insert(
            "network".to_string(),
            ingot_ir::PolicyRule {
                decision: ingot_ir::Decision::Deny,
                values: Vec::new(),
                qualifier: None,
            },
        );
        let text = emit(&ir).unwrap();
        assert!(text.contains("POLICY = {\"network\""), "{text}");
        assert!(text.contains("\"decision\": \"deny\""), "{text}");
    }

    #[test]
    fn the_generated_program_holds_no_environment_value() {
        // Build output may be committed. A build that could embed a key would
        // make that dangerous, so there must be no path from the environment
        // into the emitted text at all.
        std::env::set_var("INGOT_EMIT_LEAK_PROBE", "sk-live-do-not-embed");
        let text = emit(&bare()).unwrap();
        std::env::remove_var("INGOT_EMIT_LEAK_PROBE");
        assert!(!text.contains("sk-live-do-not-embed"), "{text}");
        assert!(!text.contains("INGOT_EMIT_LEAK_PROBE"), "{text}");
    }

    #[test]
    fn the_same_artifact_emits_the_same_bytes() {
        let ir = bare();
        assert_eq!(emit(&ir).unwrap(), emit(&ir).unwrap());
    }
}
