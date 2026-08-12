//! What a target can and cannot do with an artifact, decided before it is built.
//!
//! The report is the honest half of having more than one backend. A target that
//! silently drops a construct it does not implement produces an agent that runs
//! and is wrong; naming the construct, counting it, and refusing the build is the
//! same rule [Runtime 0.1 §2] states for execution, applied to compilation.
//!
//! Three verdicts, and the distinction between the middle two is the whole point:
//!
//! * [`Support::Supported`] — the target does this.
//! * [`Support::Degraded`] — the observable result is the same; something else is
//!   weaker. `parallel` executed sequentially is the example: the compiler
//!   guarantees a `parallel` body cannot observe another iteration, so only the
//!   wall clock differs.
//! * [`Support::Unimplemented`] — the target cannot do this. The build is refused
//!   unless the operator says otherwise, in writing, on the command line.
//!
//! [Runtime 0.1 §2]: https://github.com/mathissdupont/ingot/blob/main/specs/runtime/v0.1.md

use std::collections::BTreeMap;

use ingot_ir::{AgentIr, NodeKind};
use serde::{Deserialize, Serialize};

/// How well a target handles one construct.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Support {
    Supported,
    Degraded,
    Unimplemented,
}

impl Support {
    pub fn as_str(self) -> &'static str {
        match self {
            Support::Supported => "supported",
            Support::Degraded => "degraded",
            Support::Unimplemented => "not implemented",
        }
    }

    /// Whether a build carrying this may proceed without being asked twice.
    pub fn buildable(self) -> bool {
        !matches!(self, Support::Unimplemented)
    }
}

/// One construct, and what this target does about it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    /// The IR node kind, spelled as the IR spells it.
    pub construct: String,
    pub support: Support,
    /// How many nodes of this kind the agent has. A count rather than a flag,
    /// because "one `verify`" and "forty" are different conversations.
    pub nodes: usize,
    /// Why, in enough detail to act on. Never "unsupported".
    pub reason: String,
}

/// What a target would make of one agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentReport {
    pub agent: String,
    /// Sorted by severity, worst first, so the reason a build failed is the first
    /// thing on the screen.
    pub findings: Vec<Finding>,
}

impl AgentReport {
    /// Whether every construct this agent uses is implemented.
    pub fn buildable(&self) -> bool {
        self.findings
            .iter()
            .all(|finding| finding.support.buildable())
    }

    pub fn unimplemented(&self) -> Vec<&Finding> {
        self.findings
            .iter()
            .filter(|finding| finding.support == Support::Unimplemented)
            .collect()
    }
}

/// What a target would make of a whole program.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    /// The target's name, as `--target` spells it.
    pub target: String,
    pub agents: Vec<AgentReport>,
}

impl Report {
    pub fn buildable(&self) -> bool {
        self.agents.iter().all(AgentReport::buildable)
    }

    /// Agents that cannot be built for this target.
    pub fn blocked(&self) -> Vec<&AgentReport> {
        self.agents
            .iter()
            .filter(|agent| !agent.buildable())
            .collect()
    }

    /// Every construct anywhere in the program this target does not implement.
    ///
    /// Named so that `ingot build --target python --json | jq -e
    /// '.unimplemented == []'` reads as a deployment gate.
    pub fn unimplemented(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .agents
            .iter()
            .flat_map(AgentReport::unimplemented)
            .map(|finding| finding.construct.clone())
            .collect();
        names.sort();
        names.dedup();
        names
    }

    /// Prose, for a terminal.
    pub fn render(&self) -> String {
        use std::fmt::Write;
        let mut out = String::new();
        let _ = writeln!(out, "report for target `{}`", self.target);

        for agent in &self.agents {
            let _ = writeln!(out, "  {}", agent.agent);
            if agent.findings.is_empty() {
                let _ = writeln!(out, "    (nothing this target has to say)");
                continue;
            }
            for finding in &agent.findings {
                let _ = writeln!(
                    out,
                    "    {:<16} {:>2} node{}   {}",
                    finding.construct,
                    finding.nodes,
                    if finding.nodes == 1 { " " } else { "s" },
                    finding.support.as_str()
                );
                for line in wrap(&finding.reason, 64) {
                    let _ = writeln!(out, "                     {line}");
                }
            }
        }

        let blocked = self.blocked().len();
        let _ = writeln!(out);
        if blocked == 0 {
            let _ = writeln!(
                out,
                "every construct these {} agent(s) use is implemented for `{}`",
                self.agents.len(),
                self.target
            );
        } else {
            let _ = writeln!(
                out,
                "{blocked} of {} agent(s) cannot be built for `{}` without --allow-unimplemented",
                self.agents.len(),
                self.target
            );
        }
        out.trim_end().to_string()
    }
}

/// Wrap at a word boundary. Reasons are prose and a terminal is not infinite.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        if !current.is_empty() && current.chars().count() + 1 + word.chars().count() > width {
            lines.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

/// The Python backend's verdict on one node, and the construct it counts as.
///
/// A table rather than a `match` scattered through the emitter, so that what this
/// target supports is one readable list — and so that adding a node kind to the
/// IR makes this fail to compile rather than silently reporting the new kind as
/// supported.
///
fn verdict(kind: NodeKind) -> (Support, &'static str) {
    match kind {
        NodeKind::LlmCall => (Support::Supported, ""),
        NodeKind::Branch => (Support::Supported, ""),
        NodeKind::StateRead => (Support::Supported, ""),
        NodeKind::StateWrite => (Support::Supported, ""),
        NodeKind::ArtifactEmit => (Support::Supported, ""),
        NodeKind::Approval => (
            Support::Degraded,
            "the gate is asked on a terminal, and denied when there is no \
             terminal to ask; an unattended run therefore stops here rather than \
             proceeding, which is the safe direction but is not the same as \
             having a decider",
        ),
        NodeKind::Loop => (Support::Supported, ""),
        NodeKind::Checkpoint => (
            Support::Degraded,
            "emitted as an event; a run cannot be resumed from one, the same \
             limitation the reference interpreter has (GAP-008)",
        ),
        NodeKind::Parallel => (
            Support::Degraded,
            "executed sequentially. The result is identical: the compiler \
             guarantees a `parallel` body contains no state write, no emission \
             and no checkpoint (ING6005), so iterations cannot observe one \
             another and only the wall clock differs (GAP-010)",
        ),
        // Both cases match the reference interpreter exactly: a `verify`
        // carrying a `condition` is evaluated and reports `passed` or `failed`,
        // and one without a body reports `notPerformed`. Neither is a
        // limitation of this target, so neither is a finding — the compiler's
        // ING6006 is where a bodyless verifier gets said.
        NodeKind::Verify => (Support::Supported, ""),
        NodeKind::ToolCall => (
            Support::Unimplemented,
            "an MCP client over stdio is not written for this target yet; an \
             agent that declares a tool would silently do nothing without one",
        ),
        NodeKind::AgentCall => (
            Support::Unimplemented,
            "a nested run needs the sub-agent's own artifact and its own policy \
             check, and this target does not carry a second agent yet",
        ),
    }
}

/// What the Python target makes of one agent.
pub fn analyse_agent(ir: &AgentIr) -> AgentReport {
    let mut counts: BTreeMap<&'static str, (NodeKind, usize)> = BTreeMap::new();
    for node in &ir.nodes {
        let entry = counts.entry(node.kind.as_str()).or_insert((node.kind, 0));
        entry.1 += 1;
    }

    let mut findings: Vec<Finding> = counts
        .into_iter()
        .filter_map(|(name, (kind, nodes))| {
            let (support, reason) = verdict(kind);
            // A supported construct is not news. Reporting every `llm.call` as
            // fine would bury the one line that matters.
            if support == Support::Supported {
                return None;
            }
            Some(Finding {
                construct: name.to_string(),
                support,
                nodes,
                reason: reason.to_string(),
            })
        })
        .collect();

    // Worst first, then by name so two runs of the same artifact report the same
    // order.
    findings.sort_by(|left, right| {
        right
            .support
            .cmp(&left.support)
            .then_with(|| left.construct.cmp(&right.construct))
    });

    AgentReport {
        agent: ir.agent.clone(),
        findings,
    }
}

/// What the Python target makes of a program.
pub fn analyse(target: &str, agents: &[AgentIr]) -> Report {
    Report {
        target: target.to_string(),
        agents: agents.iter().map(analyse_agent).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ingot_ir::Node;

    fn agent(kinds: &[NodeKind]) -> AgentIr {
        let mut ir = AgentIr::from_json(
            r#"{"irVersion":"0.1","language":"0.1","agent":"p.A","inputs":{},"outputs":{},
                "types":{},"requirements":{"model":{"mode":"unspecified"}},"tools":[],
                "state":{},"budget":{},"policy":{},"effects":[],"nodes":[]}"#,
        )
        .expect("the fixture must parse");
        for (index, kind) in kinds.iter().enumerate() {
            ir.nodes.push(Node::new(format!("n{index}"), *kind));
        }
        ir
    }

    #[test]
    fn a_supported_construct_is_not_reported() {
        // Listing every `llm.call` as fine would bury the line that matters.
        let report = analyse_agent(&agent(&[NodeKind::LlmCall, NodeKind::ArtifactEmit]));
        assert!(report.findings.is_empty(), "{:?}", report.findings);
        assert!(report.buildable());
    }

    #[test]
    fn an_unimplemented_construct_blocks_the_build_and_says_which_and_how_many() {
        let report = analyse_agent(&agent(&[
            NodeKind::LlmCall,
            NodeKind::ToolCall,
            NodeKind::ToolCall,
        ]));
        assert!(!report.buildable());
        let finding = &report.findings[0];
        assert_eq!(finding.construct, "tool.call");
        assert_eq!(finding.nodes, 2, "a count, not a flag");
        assert!(finding.reason.contains("MCP"), "{}", finding.reason);
    }

    #[test]
    fn verify_is_not_a_finding_because_this_target_matches_the_reference() {
        // Both a check with a body and one without behave exactly as the
        // reference interpreter behaves. A finding here would be noise, and
        // noise is what buries the line that matters.
        let report = analyse_agent(&agent(&[NodeKind::LlmCall, NodeKind::Verify]));
        assert!(report.findings.is_empty(), "{:?}", report.findings);
        assert!(report.buildable());
    }

    #[test]
    fn a_degraded_construct_does_not_block_the_build() {
        // `parallel` run sequentially produces the same result. Refusing over it
        // would make the report a nuisance rather than a signal.
        let report = analyse_agent(&agent(&[NodeKind::Parallel]));
        assert!(report.buildable());
        assert_eq!(report.findings[0].support, Support::Degraded);
        assert!(report.findings[0].reason.contains("only the wall clock"));
    }

    #[test]
    fn the_worst_finding_is_first() {
        let report = analyse_agent(&agent(&[
            NodeKind::Parallel,
            NodeKind::ToolCall,
            NodeKind::Checkpoint,
        ]));
        assert_eq!(report.findings[0].support, Support::Unimplemented);
        assert!(report
            .findings
            .iter()
            .skip(1)
            .all(|finding| finding.support == Support::Degraded));
    }

    #[test]
    fn the_report_is_the_same_order_every_run() {
        let ir = agent(&[
            NodeKind::Checkpoint,
            NodeKind::Parallel,
            NodeKind::ToolCall,
            NodeKind::AgentCall,
        ]);
        let once = analyse("python", std::slice::from_ref(&ir));
        let again = analyse("python", std::slice::from_ref(&ir));
        assert_eq!(once, again);
        let names: Vec<&str> = once.agents[0]
            .findings
            .iter()
            .map(|finding| finding.construct.as_str())
            .collect();
        assert_eq!(
            names,
            vec!["agent.call", "tool.call", "checkpoint", "parallel"]
        );
    }

    #[test]
    fn a_program_is_blocked_by_any_one_agent() {
        let report = analyse(
            "python",
            &[agent(&[NodeKind::LlmCall]), agent(&[NodeKind::ToolCall])],
        );
        assert!(!report.buildable());
        assert_eq!(report.blocked().len(), 1);
        assert_eq!(report.unimplemented(), vec!["tool.call".to_string()]);
    }

    #[test]
    fn a_clean_program_reports_an_empty_gate() {
        // The shape a deployment gate checks: `.unimplemented == []`.
        let report = analyse("python", &[agent(&[NodeKind::LlmCall])]);
        assert!(report.unimplemented().is_empty());
        let json = serde_json::to_string(&report).unwrap();
        assert!(json.contains("\"target\":\"python\""), "{json}");
    }

    #[test]
    fn the_prose_names_the_target_and_the_verdict() {
        let text = analyse("python", &[agent(&[NodeKind::ToolCall])]).render();
        assert!(text.contains("report for target `python`"), "{text}");
        assert!(text.contains("not implemented"), "{text}");
        assert!(text.contains("--allow-unimplemented"), "{text}");
        assert!(
            text.lines().all(|line| line.chars().count() <= 100),
            "reasons must wrap:\n{text}"
        );
    }

    #[test]
    fn a_reason_is_never_merely_unsupported() {
        // Every verdict has to be actionable. "unsupported" tells nobody
        // anything, and this is the test that keeps it that way.
        for kind in [
            NodeKind::LlmCall,
            NodeKind::ToolCall,
            NodeKind::AgentCall,
            NodeKind::Branch,
            NodeKind::Parallel,
            NodeKind::Loop,
            NodeKind::Approval,
            NodeKind::Verify,
            NodeKind::StateRead,
            NodeKind::StateWrite,
            NodeKind::ArtifactEmit,
            NodeKind::Checkpoint,
        ] {
            let (support, reason) = verdict(kind);
            if support == Support::Supported {
                assert!(reason.is_empty(), "{kind:?} needs no reason");
            } else {
                assert!(
                    reason.len() > 40,
                    "{kind:?} has a reason nobody could act on: {reason}"
                );
            }
        }
    }
}
