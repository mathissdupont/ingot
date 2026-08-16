//! A deterministic, non-interactive renderer for the portable run events.
//!
//! The renderer consumes events in order and never rewrites them. It enriches
//! them from Agent IR where that is safe: agent/node provenance, budget ceilings,
//! static prompt text, source ranges, and context names. Runtime-derived values
//! are redacted because Agent IR carries no secret classification that could
//! make selective disclosure honest.

use std::collections::BTreeMap;

use ingot_ir::{AgentIr, Node, RefScope, SourceSpan, TemplatePart, Value};
use ingot_runtime::{RunEvent, VerifyOutcome};
use ingot_source::{FileId, SourceMap};

const MAX_PROMPT_CHARS: usize = 240;

#[derive(Debug)]
struct Frame {
    agent: String,
    node: Option<String>,
    observed_steps: u32,
    observed_tokens: u64,
}

/// Stateful because nested agents and cumulative budgets need the preceding
/// events. It writes plain text only: no cursor control, colour or TTY state.
pub struct HumanTrace {
    agents: BTreeMap<String, AgentIr>,
    sources: Option<SourceContext>,
    frames: Vec<Frame>,
    sequence: u64,
}

#[derive(Debug)]
struct SourceContext {
    files: BTreeMap<String, SourceSnapshot>,
}

#[derive(Debug)]
struct SourceSnapshot {
    text: String,
    line_starts: Vec<u32>,
}

impl HumanTrace {
    pub fn new(agents: &[AgentIr]) -> Self {
        Self {
            agents: agents
                .iter()
                .map(|agent| (agent.agent.clone(), agent.clone()))
                .collect(),
            sources: None,
            frames: Vec::new(),
            sequence: 0,
        }
    }

    pub fn with_sources(agents: &[AgentIr], sources: &SourceMap, root: FileId) -> Self {
        let mut trace = Self::new(agents);
        trace.sources = Some(SourceContext::new(sources, root));
        trace
    }

    /// Render exactly one block for exactly one event.
    pub fn render(&mut self, event: &RunEvent) -> String {
        self.sequence += 1;
        let mut detail = Vec::new();
        let headline = match event {
            RunEvent::RunStarted { agent, provider } => {
                self.frames.push(Frame {
                    agent: agent.clone(),
                    node: None,
                    observed_steps: 0,
                    observed_tokens: 0,
                });
                if let Some(ir) = self.agents.get(agent) {
                    detail.push(format!("budget {}", budget(ir)));
                }
                format!("run.started  {agent}  (provider: {provider})")
            }
            RunEvent::NodeStarted { node, kind } => {
                if let Some(frame) = self.frames.last_mut() {
                    frame.node = Some(node.clone());
                }
                let agent = self.current_agent();
                if let Some(ir_node) = self.current_node(node) {
                    if let Some(prompt) = ir_node.prompt.as_ref() {
                        detail.push(format!("prompt {}", safe_prompt(prompt)));
                    }
                    detail.push(self.source_detail(ir_node));
                    let context: Vec<&str> = ir_node
                        .args
                        .iter()
                        .map(|argument| argument.name.as_str())
                        .filter(|name| !matches!(*name, "system" | "temperature" | "max_tokens"))
                        .collect();
                    if !context.is_empty() {
                        detail.push(format!(
                            "context {}",
                            context
                                .iter()
                                .map(|name| format!("{name}=<redacted>"))
                                .collect::<Vec<_>>()
                                .join(", ")
                        ));
                    }
                } else {
                    detail
                        .push("source span unavailable: node not present in Agent IR".to_string());
                }
                format!("node.started {agent}:{node}  {kind}")
            }
            RunEvent::ModelCall {
                node,
                model,
                response_type,
                usage,
            } => {
                self.charge(1, usage.total());
                detail.push(self.progress());
                format!(
                    "model.call   {}:{node}  {model} -> {response_type}  ({} in, {} out)",
                    self.current_agent(),
                    usage.input_tokens,
                    usage.output_tokens
                )
            }
            RunEvent::ToolCall {
                node,
                tool,
                effects,
            } => {
                self.charge(1, 0);
                detail.push(self.progress());
                format!(
                    "tool.call    {}:{node}  {tool}  effects=[{}]",
                    self.current_agent(),
                    effects.join(", ")
                )
            }
            RunEvent::AgentCall { node, agent } => {
                self.charge(1, 0);
                detail.push(self.progress());
                format!("agent.call   {}:{node} -> {agent}", self.current_agent())
            }
            RunEvent::ConsultationAsked {
                node,
                index,
                question,
                choices,
            } => {
                // The question is shown in full. Unlike a prompt it is not
                // redacted: a person is about to be asked it out loud, so
                // hiding it here would hide it from the only reader who could
                // check what was asked.
                detail.push(format!("asks {question}"));
                if !choices.is_empty() {
                    detail.push(format!("choices {}", choices.join(" | ")));
                }
                format!("consult.ask  {}:{node}  #{index}", self.current_agent())
            }
            RunEvent::ConsultationAnswered {
                node,
                index,
                answer,
            } => {
                detail.push(format!("answer {answer}"));
                format!("consult.said {}:{node}  #{index}", self.current_agent())
            }
            RunEvent::ApprovalRequested {
                node,
                effects,
                reason,
            } => format!(
                "approval.ask {}:{node}  [{}]  {reason}",
                self.current_agent(),
                effects.join(", ")
            ),
            RunEvent::ApprovalDecided { node, allowed } => format!(
                "approval.{} {}:{node}",
                if *allowed { "yes" } else { "no" },
                self.current_agent()
            ),
            RunEvent::StateWritten { node, field } => {
                format!("state.write  {}:{node}  {field}", self.current_agent())
            }
            RunEvent::Verified {
                node,
                verifier,
                outcome,
            } => format!(
                "verify.{:<6} {}:{node}  {verifier}",
                match outcome {
                    VerifyOutcome::NotPerformed => "none",
                    VerifyOutcome::Passed => "pass",
                    VerifyOutcome::Failed => "fail",
                },
                self.current_agent()
            ),
            RunEvent::Checkpoint { node, label } => {
                format!("checkpoint   {}:{node}  \"{label}\"", self.current_agent())
            }
            RunEvent::BranchTaken { node, arm } => {
                format!("branch       {}:{node}  {arm}", self.current_agent())
            }
            RunEvent::LoopIteration { node, iteration } => format!(
                "loop         {}:{node}  iteration={iteration}",
                self.current_agent()
            ),
            RunEvent::MapIteration { node, index, total } => format!(
                "map          {}:{node}  element={}/{total}",
                self.current_agent(),
                index + 1
            ),
            RunEvent::Emitted { node, output } => {
                format!(
                    "artifact.emit {}:{node}  emit {output}",
                    self.current_agent()
                )
            }
            RunEvent::RunFinished { steps, usage } => {
                let agent = self.current_agent().to_string();
                if let Some(ir) = self.agents.get(&agent) {
                    detail.push(format!(
                        "final steps {}/{}; tokens {}/{}",
                        steps,
                        limit(ir.budget.steps),
                        usage.total(),
                        limit(ir.budget.tokens)
                    ));
                }
                let headline = format!(
                    "run.finished {agent}  done: {steps} step(s), {} token(s)",
                    usage.total()
                );
                self.frames.pop();
                headline
            }
            RunEvent::RunFailed { reason } => {
                let agent = self.current_agent().to_string();
                let node = self
                    .frames
                    .last()
                    .and_then(|frame| frame.node.as_deref())
                    .unwrap_or("before-entry");
                let headline = format!("run.failed   {agent}:{node}  {reason}");
                self.frames.pop();
                headline
            }
            RunEvent::RunStopped { node, label } => {
                let agent = self.current_agent().to_string();
                let headline = format!("run.stopped  {agent}:{node}  \"{label}\"");
                self.frames.pop();
                headline
            }
        };

        let mut output = format!("trace[{:04}] {headline}", self.sequence);
        for line in detail {
            output.push_str("\n             ");
            output.push_str(&line);
        }
        output
    }

    fn current_agent(&self) -> &str {
        self.frames
            .last()
            .map(|frame| frame.agent.as_str())
            .unwrap_or("unknown-agent")
    }

    fn current_node(&self, node: &str) -> Option<&Node> {
        self.agents
            .get(self.current_agent())
            .and_then(|agent| agent.node(node))
    }

    fn source_detail(&self, node: &Node) -> String {
        let Some(span) = node.source_span.as_ref() else {
            return "source span unavailable in Agent IR".to_string();
        };
        match self
            .sources
            .as_ref()
            .and_then(|sources| sources.render(span))
        {
            Some(rendered) => format!("source {rendered}"),
            None => format!(
                "source {} bytes {}..{} (source unavailable locally)",
                span.source, span.start, span.end
            ),
        }
    }

    fn charge(&mut self, steps: u32, tokens: u64) {
        // A nested agent's work counts toward its caller's total too. Frames are
        // the live call stack, so charging each one mirrors the runtime report.
        for frame in &mut self.frames {
            frame.observed_steps += steps;
            frame.observed_tokens += tokens;
        }
    }

    fn progress(&self) -> String {
        let Some(frame) = self.frames.last() else {
            return "budget unavailable".to_string();
        };
        let Some(agent) = self.agents.get(&frame.agent) else {
            return format!(
                "observed steps {}; tokens {}",
                frame.observed_steps, frame.observed_tokens
            );
        };
        format!(
            "observed steps {}/{}; tokens {}/{}",
            frame.observed_steps,
            limit(agent.budget.steps),
            frame.observed_tokens,
            limit(agent.budget.tokens)
        )
    }
}

impl SourceContext {
    fn new(sources: &SourceMap, root: FileId) -> Self {
        let files = sources
            .files()
            .map(|file| {
                (
                    sources.portable_name(file.id(), root),
                    SourceSnapshot::new(file.text()),
                )
            })
            .collect();
        Self { files }
    }

    fn render(&self, span: &SourceSpan) -> Option<String> {
        let source = self.files.get(&span.source)?;
        Some(format!(
            "{}:{}..{}",
            span.source,
            source.line_col(span.start),
            source.line_col(span.end)
        ))
    }
}

impl SourceSnapshot {
    fn new(text: &str) -> Self {
        let mut line_starts = vec![0u32];
        for (index, byte) in text.bytes().enumerate() {
            if byte == b'\n' {
                line_starts.push(index as u32 + 1);
            }
        }
        Self {
            text: text.to_string(),
            line_starts,
        }
    }

    fn line_col(&self, offset: u32) -> String {
        let offset = offset.min(self.text.len() as u32);
        let line_index = match self.line_starts.binary_search(&offset) {
            Ok(exact) => exact,
            Err(next) => next.saturating_sub(1),
        };
        let line_start = self.line_starts[line_index] as usize;
        let column = self.text[line_start..offset as usize].chars().count() as u32 + 1;
        format!("{}:{}", line_index as u32 + 1, column)
    }
}

fn budget(agent: &AgentIr) -> String {
    format!(
        "steps<={} tokens<={}",
        limit(agent.budget.steps),
        limit(agent.budget.tokens)
    )
}

fn limit(value: Option<i64>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "unbounded".to_string())
}

/// Static text is source-authored and visible. Every runtime-derived insertion
/// is replaced with its provenance and type; values never enter the trace.
fn safe_prompt(value: &Value) -> String {
    match value {
        Value::Literal { value, .. } => value
            .as_str()
            .map(quoted_one_line)
            .unwrap_or_else(|| "<non-text literal>".to_string()),
        Value::Template { parts } => {
            let mut rendered = String::new();
            for part in parts {
                match part {
                    TemplatePart::Text { value } => rendered.push_str(value),
                    TemplatePart::Value { value, ty } => {
                        rendered.push_str(&format!("<redacted {}:{ty}>", provenance(value)))
                    }
                }
            }
            quoted_one_line(&rendered)
        }
        other => format!("<redacted {}>", provenance(other)),
    }
}

fn quoted_one_line(text: &str) -> String {
    // A source-authored prompt must not inject terminal controls into a log.
    // `escape_default` also keeps the surrounding quotes unambiguous.
    let compact: String = text.escape_default().collect();
    let total = compact.chars().count();
    if total <= MAX_PROMPT_CHARS {
        return format!("\"{compact}\"");
    }
    let prefix: String = compact.chars().take(MAX_PROMPT_CHARS).collect();
    format!("\"{prefix}…\" ({total} chars)")
}

fn provenance(value: &Value) -> String {
    match value {
        Value::Ref { scope, path } => format!(
            "{}.{}",
            match scope {
                RefScope::Input => "input",
                RefScope::Binding => "binding",
                RefScope::State => "state",
                RefScope::Memory => "memory",
            },
            path.join(".")
        ),
        Value::Literal { .. } => "literal".to_string(),
        Value::List { .. } => "list".to_string(),
        Value::Template { .. } => "template".to_string(),
        Value::Unary { .. } => "expression".to_string(),
        Value::Binary { .. } => "expression".to_string(),
        Value::Builtin { name, .. } => format!("builtin.{name}"),
        Value::Unknown => "unknown".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ingot_ir::{Budget, Requirements, IR_VERSION};
    use ingot_runtime::Usage;

    fn agent() -> AgentIr {
        AgentIr {
            ir_version: IR_VERSION.to_string(),
            language: "0.1".to_string(),
            agent: "demo.Trace".to_string(),
            doc: None,
            inputs: BTreeMap::new(),
            outputs: BTreeMap::new(),
            types: BTreeMap::new(),
            requirements: Requirements {
                model: ingot_ir::ModelRequirement::Unspecified,
            },
            tools: Vec::new(),
            state: BTreeMap::new(),
            persistent: BTreeMap::new(),
            budget: Budget {
                steps: Some(4),
                tokens: Some(1000),
                cost: None,
            },
            policy: BTreeMap::new(),
            effects: Vec::new(),
            entry: None,
            nodes: Vec::new(),
        }
    }

    #[test]
    fn the_human_trace_preserves_the_json_event_order() {
        let events = [
            RunEvent::RunStarted {
                agent: "demo.Trace".to_string(),
                provider: "replay".to_string(),
            },
            RunEvent::NodeStarted {
                node: "n0".to_string(),
                kind: "llm.call".to_string(),
            },
            RunEvent::ModelCall {
                node: "n0".to_string(),
                model: "cassette/model".to_string(),
                response_type: "markdown".to_string(),
                usage: Usage {
                    input_tokens: 4,
                    output_tokens: 2,
                    cache_read_tokens: 0,
                },
            },
            RunEvent::Emitted {
                node: "n1".to_string(),
                output: "brief".to_string(),
            },
            RunEvent::RunFinished {
                steps: 1,
                usage: Usage {
                    input_tokens: 4,
                    output_tokens: 2,
                    cache_read_tokens: 0,
                },
            },
        ];
        let json_order: Vec<String> = events
            .iter()
            .map(|event| {
                serde_json::from_str::<serde_json::Value>(&event.to_json_line()).unwrap()["event"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect();
        let mut trace = HumanTrace::new(&[agent()]);
        let rendered: Vec<String> = events.iter().map(|event| trace.render(event)).collect();

        assert_eq!(
            json_order,
            vec![
                "runStarted",
                "nodeStarted",
                "modelCall",
                "emitted",
                "runFinished"
            ]
        );
        for (index, (line, label)) in rendered
            .iter()
            .zip([
                "run.started",
                "node.started",
                "model.call",
                "artifact.emit",
                "run.finished",
            ])
            .enumerate()
        {
            assert!(
                line.starts_with(&format!("trace[{:04}] {label}", index + 1)),
                "event moved or disappeared: {rendered:#?}"
            );
        }
    }

    #[test]
    fn node_started_resolves_local_source_spans() {
        let compilation = ingot_compiler::compile_source(
            "main.ing",
            r#"
language 0.1
package demo

agent Trace(topic: string) -> report<markdown> {
  flow {
    emit report = ask<markdown>("hello ${topic}")
  }
}
"#,
        );
        assert!(
            !compilation.has_errors(),
            "expected clean compile:\n{}",
            compilation.render_diagnostics(ingot_diagnostics::ColorChoice::Never)
        );
        let agent = compilation.primary_agent().unwrap();
        let mut trace =
            HumanTrace::with_sources(&compilation.agents, &compilation.sources, compilation.file);
        trace.render(&RunEvent::RunStarted {
            agent: agent.agent.clone(),
            provider: "replay".to_string(),
        });
        let rendered = trace.render(&RunEvent::NodeStarted {
            node: agent.entry.clone().unwrap(),
            kind: "llm.call".to_string(),
        });
        assert!(
            rendered.contains("source main.ing:"),
            "trace should resolve source span, got:\n{rendered}"
        );
        assert!(!rendered.contains("unavailable"));
    }

    #[test]
    fn node_started_keeps_byte_span_when_source_is_missing() {
        let mut agent = agent();
        let mut node = Node::new("n0", ingot_ir::NodeKind::Checkpoint);
        node.source_span = Some(SourceSpan {
            source: "main.ing".to_string(),
            start: 1,
            end: 4,
        });
        agent.entry = Some("n0".to_string());
        agent.nodes.push(node);
        let mut trace = HumanTrace::new(&[agent]);
        trace.render(&RunEvent::RunStarted {
            agent: "demo.Trace".to_string(),
            provider: "replay".to_string(),
        });
        let rendered = trace.render(&RunEvent::NodeStarted {
            node: "n0".to_string(),
            kind: "checkpoint".to_string(),
        });
        assert!(rendered.contains("source main.ing bytes 1..4"));
        assert!(rendered.contains("source unavailable locally"));
    }
}
