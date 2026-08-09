//! Guardrails for model-assisted `.ing` authoring.
//!
//! The authoring model is allowed to propose ordinary source. It is not allowed
//! to approve new capabilities for itself, to invent a tool the project does not
//! route, or to put a credential value anywhere. This module owns those three
//! rules and the bounded, compiler-verified loop they wrap.
//!
//! Where the candidate source comes from is deliberately behind a trait. A file
//! on disk and a live provider are the same loop: propose, compile, show the
//! operator what happened, and stop at a stated ceiling. That is what keeps the
//! provider-backed path from acquiring a second, weaker set of rules.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result};
use ingot_compiler::compile_source;
use ingot_diagnostics::Severity;
use ingot_mcp::ToolDescriptor;
use ingot_package::secrets;
use ingot_runtime::schema::ResponseShape;
use ingot_runtime::{CompletionRequest, ModelProvider, ModelSelection, Usage};
use ingot_syntax::{PolicyAction, Program};

/// The name the compiler gives a candidate revision in its diagnostics.
const CANDIDATE_NAME: &str = "candidate.ing";

/// Diagnostic code for a tool the project does not route.
const UNROUTED_TOOL: &str = "AUTHORING_UNROUTED_TOOL";

/// A capability newly granted by candidate source.
///
/// Only a grant is a proposal. `deny` and `require approval` narrow what an
/// agent may do, and the language is default-deny
/// ([Language 0.1 §7](../../../specs/language/v0.1.md)), so a withdrawn rule
/// grants nothing either. Asking an operator to accept a restriction would train
/// them to accept the list without reading it, which is the failure this whole
/// mechanism exists to prevent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PolicyProposal {
    pub(crate) agent: String,
    pub(crate) subject: String,
    pub(crate) action: String,
}

/// Result of a bounded compiler-verified authoring repair loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RepairLoop {
    attempts: Vec<RepairAttempt>,
    outcome: RepairOutcome,
    accepted_proposals: Vec<PolicyProposal>,
    usage: Usage,
}

impl RepairLoop {
    pub(crate) fn attempts(&self) -> &[RepairAttempt] {
        &self.attempts
    }

    pub(crate) fn outcome(&self) -> &RepairOutcome {
        &self.outcome
    }

    /// Policy grants the operator accepted in advance, and which this loop
    /// therefore carried rather than stopped on. Empty on the ordinary path.
    pub(crate) fn accepted_proposals(&self) -> &[PolicyProposal] {
        &self.accepted_proposals
    }

    /// What the authoring calls cost, when a provider made them.
    pub(crate) fn usage(&self) -> Usage {
        self.usage
    }

    pub(crate) fn accepted_source(&self) -> Option<&str> {
        match &self.outcome {
            RepairOutcome::Compiled { source } => Some(source),
            RepairOutcome::PolicyProposals { .. }
            | RepairOutcome::RetryCeilingReached
            | RepairOutcome::CredentialRefused { .. } => None,
        }
    }
}

/// One source revision checked by the repair loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RepairAttempt {
    pub(crate) number: usize,
    pub(crate) source: String,
    pub(crate) diagnostics: Vec<AuthoringDiagnostic>,
}

impl RepairAttempt {
    pub(crate) fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error)
    }
}

/// A compact, model-safe diagnostic summary for repair prompts and logs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AuthoringDiagnostic {
    pub(crate) code: &'static str,
    pub(crate) severity: Severity,
    pub(crate) message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RepairOutcome {
    Compiled {
        source: String,
    },
    PolicyProposals {
        proposals: Vec<PolicyProposal>,
    },
    RetryCeilingReached,
    /// A candidate carried something shaped like a credential value.
    ///
    /// This ends the loop rather than becoming a repair prompt: feeding the
    /// offending source back to the model would put the value in a second
    /// place. The report names where it was, never what it was.
    CredentialRefused {
        finding: secrets::Finding,
    },
}

/// How far a loop may go, and what the operator has already agreed to.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Limits {
    /// Follow-up proposals the loop may consume after the first one.
    pub(crate) max_repairs: usize,
    /// Whether the operator read the policy grants and accepted them.
    ///
    /// Acceptance is per invocation and applies to whatever the proposal turns
    /// out to request: the loop still lists every grant it carried, so the
    /// record of what was accepted survives in the output.
    pub(crate) accept_policy: bool,
}

/// Somewhere candidate `.ing` source comes from.
///
/// Implemented by files on disk and by a model provider, so both reach the
/// compiler, the policy comparison and the credential scan through the same
/// path.
pub(crate) trait CandidateSource {
    /// The next proposal, or `None` when this source has no more to offer.
    fn propose(&mut self, feedback: Option<Feedback<'_>>) -> Result<Option<String>>;

    /// What the proposals cost so far. Zero for anything that is not a provider.
    fn usage(&self) -> Usage {
        Usage::default()
    }
}

/// What the previous attempt produced, for a source that can react to it.
pub(crate) struct Feedback<'a> {
    pub(crate) source: &'a str,
    pub(crate) diagnostics: &'a [AuthoringDiagnostic],
}

/// Run compiler-verified repair attempts without allowing automatic policy
/// widening, an invented tool, or a credential value in source.
///
/// The loop is deliberately bounded and deterministic: it consumes the first
/// proposal plus at most `limits.max_repairs` follow-ups, and never invents
/// another candidate after that.
pub(crate) fn author(
    previous_source: &str,
    candidates: &mut dyn CandidateSource,
    tools: &ToolContext,
    limits: Limits,
) -> Result<RepairLoop> {
    let previous_grants = granted(&compile_source("previous.ing", previous_source).program);
    let mut attempts: Vec<RepairAttempt> = Vec::new();
    let mut accepted_proposals: Vec<PolicyProposal> = Vec::new();
    let mut feedback: Option<(String, Vec<AuthoringDiagnostic>)> = None;

    for index in 0..=limits.max_repairs {
        let candidate =
            candidates.propose(feedback.as_ref().map(|(source, diagnostics)| Feedback {
                source,
                diagnostics,
            }))?;
        let Some(candidate) = candidate else { break };

        // Before the compiler, before the policy comparison, and before the
        // source is written anywhere: a credential must not travel further.
        if let Some(finding) = secrets::scan(&candidate) {
            return Ok(RepairLoop {
                attempts,
                outcome: RepairOutcome::CredentialRefused { finding },
                accepted_proposals,
                usage: candidates.usage(),
            });
        }

        let compilation = compile_source(CANDIDATE_NAME, &candidate);
        let proposals = new_grants(&previous_grants, &granted(&compilation.program));
        if !proposals.is_empty() {
            if !limits.accept_policy {
                return Ok(RepairLoop {
                    attempts,
                    outcome: RepairOutcome::PolicyProposals { proposals },
                    accepted_proposals,
                    usage: candidates.usage(),
                });
            }
            for proposal in proposals {
                if !accepted_proposals.contains(&proposal) {
                    accepted_proposals.push(proposal);
                }
            }
        }

        let mut diagnostics: Vec<AuthoringDiagnostic> = compilation
            .diagnostics
            .iter()
            .map(|diagnostic| AuthoringDiagnostic {
                code: diagnostic.code,
                severity: diagnostic.severity,
                message: diagnostic.message.clone(),
            })
            .collect();
        diagnostics.extend(tools.unrouted(&compilation.program));

        let has_errors = diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error);
        attempts.push(RepairAttempt {
            number: index + 1,
            source: candidate.clone(),
            diagnostics: diagnostics.clone(),
        });

        if !has_errors {
            return Ok(RepairLoop {
                attempts,
                outcome: RepairOutcome::Compiled { source: candidate },
                accepted_proposals,
                usage: candidates.usage(),
            });
        }
        feedback = Some((candidate, diagnostics));
    }

    Ok(RepairLoop {
        attempts,
        outcome: RepairOutcome::RetryCeilingReached,
        accepted_proposals,
        usage: candidates.usage(),
    })
}

// --- candidates from files --------------------------------------------------

/// Follow-up proposals prepared in advance, as `--repair-candidate` supplies
/// them. Deterministic, offline, and the shape the review path uses.
pub(crate) struct FixedCandidates<'a> {
    initial: &'a str,
    repairs: std::slice::Iter<'a, String>,
}

impl<'a> FixedCandidates<'a> {
    pub(crate) fn new(initial: &'a str, repairs: &'a [String]) -> FixedCandidates<'a> {
        FixedCandidates {
            initial,
            repairs: repairs.iter(),
        }
    }
}

impl CandidateSource for FixedCandidates<'_> {
    fn propose(&mut self, feedback: Option<Feedback<'_>>) -> Result<Option<String>> {
        match feedback {
            None => Ok(Some(self.initial.to_string())),
            Some(_) => Ok(self.repairs.next().cloned()),
        }
    }
}

// --- candidates from a model ------------------------------------------------

/// What the authoring model is asked to write.
pub(crate) struct AuthoringRequest {
    /// The workflow in the operator's own words.
    pub(crate) workflow: String,
    /// The source being changed, empty when the project is new.
    pub(crate) previous_source: String,
    /// The package line generated source should carry, when the project name
    /// yields a usable identifier.
    pub(crate) package: Option<String>,
}

/// A candidate source backed by a live model provider.
///
/// Every call is one non-streaming completion asking for prose, because what
/// comes back is source rather than a structured value. The compiler is what
/// validates it; the provider is never trusted to have produced something that
/// compiles.
pub(crate) struct ModelAuthor<'a> {
    provider: &'a mut dyn ModelProvider,
    request: AuthoringRequest,
    tools: &'a ToolContext,
    calls: usize,
    usage: Usage,
}

impl<'a> ModelAuthor<'a> {
    pub(crate) fn new(
        provider: &'a mut dyn ModelProvider,
        request: AuthoringRequest,
        tools: &'a ToolContext,
    ) -> ModelAuthor<'a> {
        ModelAuthor {
            provider,
            request,
            tools,
            calls: 0,
            usage: Usage::default(),
        }
    }

    pub(crate) fn calls(&self) -> usize {
        self.calls
    }

    fn complete(&mut self, prompt: String) -> Result<String> {
        let request = CompletionRequest {
            node: format!("authoring.{}", self.calls),
            model: ModelSelection::Default,
            system: Some(SYSTEM_PROMPT.to_string()),
            prompt,
            context: Vec::new(),
            response_type: "text".to_string(),
            shape: ResponseShape::Prose,
            max_tokens: 8_192,
        };
        self.calls += 1;

        let response = self
            .provider
            .complete(&request)
            .map_err(|error| anyhow::anyhow!("{error}"))
            .context("the authoring model call failed")?;
        self.usage.add(response.usage);

        let text = response
            .value
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| anyhow::anyhow!("the authoring model returned no text"))?;
        Ok(extract_source(&text))
    }
}

impl CandidateSource for ModelAuthor<'_> {
    fn propose(&mut self, feedback: Option<Feedback<'_>>) -> Result<Option<String>> {
        let prompt = match feedback {
            None => initial_prompt(&self.request, self.tools),
            Some(feedback) => repair_prompt(&self.request, feedback),
        };
        self.complete(prompt).map(Some)
    }

    fn usage(&self) -> Usage {
        self.usage
    }
}

const SYSTEM_PROMPT: &str = "\
You write Ingot, a small statically typed language for agents. Every program is \
checked before it runs: types, effects, policy and budgets are all compile-time. \
Reply with one fenced code block containing the complete source file and nothing \
else — no explanation before or after it.";

/// A compact, accurate description of the subset an authored file may use.
///
/// Deliberately narrow. Everything here appears in the maintained templates, so
/// what the model is shown is what the toolchain is tested against, and a
/// feature the guide omits is one the compiler will reject rather than one that
/// silently half-works.
const LANGUAGE_GUIDE: &str = r#"Shape of a file:

    language 0.1
    package my_project

    /// A doc comment describing the agent.
    agent Name(first: string, second: text) -> result<markdown> {
      model requires {
        structured_output
      }

      budget {
        steps <= 4
        tokens <= 20000
      }

      policy {
        network deny
      }

      flow {
        emit result = ask<markdown>("Instructions using ${first} and ${second}.")
      }
    }

Rules:

* `language 0.1` is the first line. `package` is optional and must be a valid
  identifier.
* An agent declares typed inputs and one or more typed outputs. Every declared
  output must be assigned by an `emit` in the flow.
* Types are `string`, `int`, `float`, `bool`, `text`, `markdown`, `json`,
  `file`, `bytes`, `T[]`, and records declared with `type`.
* `ask<T>("...")` is a model call. Interpolate an input with `${name}`.
  `ask<markdown>` and `ask<text>` return prose; other types are constrained to a
  schema.
* The language is default-deny: an effect is available only if `policy` grants
  it. Prefer `network deny`. Never write an `allow` rule that the workflow does
  not plainly require.
* A tool must be declared before use, with its effects, and listed in the
  agent's `tools` block:

      tool web.search(query: string) -> string[] !network

      agent Research(topic: string) -> report<markdown> {
        tools { mcp web.search }
        policy { network allow ["example.com"] }
        flow {
          hits = call web.search(topic)
          emit report = ask<markdown>("Summarise", context: hits)
        }
      }

* Never write a credential, API key, token or password anywhere. Configuration
  lives outside the source."#;

fn initial_prompt(request: &AuthoringRequest, tools: &ToolContext) -> String {
    let mut prompt = String::new();
    prompt.push_str("Write an Ingot agent for this workflow:\n\n");
    prompt.push_str(request.workflow.trim());
    prompt.push_str("\n\n");
    prompt.push_str(LANGUAGE_GUIDE);
    prompt.push_str("\n\n");
    prompt.push_str(&tools.describe());

    if let Some(package) = &request.package {
        prompt.push_str(&format!("\nUse `package {package}`.\n"));
    }

    if request.previous_source.trim().is_empty() {
        prompt.push_str(
            "\nThis is a new project, so write the whole file. Keep it to one agent \
             unless the workflow plainly needs more.\n",
        );
    } else {
        prompt.push_str(
            "\nThis project already exists. Change the source below as little as the \
             workflow requires, keep everything else exactly as it is, and reply with \
             the complete file:\n\n```ingot\n",
        );
        prompt.push_str(&request.previous_source);
        prompt.push_str("```\n");
    }
    prompt
}

fn repair_prompt(request: &AuthoringRequest, feedback: Feedback<'_>) -> String {
    let mut prompt = String::from(
        "The source you proposed does not compile. Fix it and reply with the complete \
         corrected file.\n\nThe compiler reported:\n\n",
    );
    for diagnostic in feedback.diagnostics {
        if diagnostic.severity != Severity::Error {
            continue;
        }
        prompt.push_str(&format!("* {}: {}\n", diagnostic.code, diagnostic.message));
    }
    prompt.push_str("\nThe source that failed:\n\n```ingot\n");
    prompt.push_str(feedback.source);
    if !feedback.source.ends_with('\n') {
        prompt.push('\n');
    }
    prompt.push_str("```\n\nThe workflow is still:\n\n");
    prompt.push_str(request.workflow.trim());
    prompt.push_str(
        "\n\nDo not add a policy `allow` rule to work around a diagnostic. \
         Do not add a tool the project does not route.\n",
    );
    prompt
}

/// Take the first fenced block, or the whole reply when there is none.
///
/// A model that follows the instruction sends one block; one that adds a
/// sentence of preamble should not fail the build for it, and one that sends
/// bare source should still work.
fn extract_source(reply: &str) -> String {
    let mut source = match fenced_block(reply) {
        Some(block) => block,
        None => reply.trim().to_string(),
    };
    if !source.ends_with('\n') {
        source.push('\n');
    }
    source
}

fn fenced_block(reply: &str) -> Option<String> {
    let mut lines = reply.lines();
    let mut block: Option<Vec<&str>> = None;
    for line in lines.by_ref() {
        let trimmed = line.trim_start();
        match &mut block {
            None => {
                if trimmed.starts_with("```") {
                    block = Some(Vec::new());
                }
            }
            Some(collected) => {
                if trimmed.starts_with("```") {
                    return Some(collected.join("\n"));
                }
                collected.push(line);
            }
        }
    }
    // An unterminated fence still carries source; the compiler judges it.
    block.map(|collected| collected.join("\n"))
}

// --- routed tools -----------------------------------------------------------

/// The tools a project actually routes, as MCP discovery reported them.
///
/// Authoring uses real schemas or none. A model that invents `web.search`
/// produces a file that compiles and cannot run, which is the worst of both:
/// the failure arrives at run time, in front of whoever trusted the generator.
#[derive(Debug, Clone)]
pub(crate) enum ToolContext {
    /// Nothing was discovered and nothing is claimed.
    ///
    /// The review path compares one source file with another; neither is a
    /// project, so there is no routing table to hold a declaration against, and
    /// inventing an objection would be worse than staying quiet.
    Unchecked,
    /// A project that configures no MCP server. A declared tool cannot be
    /// served by anything, so authoring must not write one.
    NoServers,
    /// What discovery reported, keyed by the name source declares a tool under.
    Routed(BTreeMap<String, ToolDescriptor>),
}

impl ToolContext {
    /// The tool section of an authoring prompt.
    fn describe(&self) -> String {
        const NONE: &str = "This project routes no MCP tool server, so declare no `tool` and \
                            write no `call`. Everything must be done with `ask`.\n";

        let ToolContext::Routed(routed) = self else {
            return NONE.to_string();
        };
        if routed.is_empty() {
            return NONE.to_string();
        }

        let mut out = String::from(
            "These are the only tools this project routes. Declare and call these or \
             none; do not invent another.\n\n",
        );
        for (name, descriptor) in routed {
            out.push_str(&format!("* `{name}`"));
            if let Some(description) = &descriptor.description {
                out.push_str(&format!(" — {description}"));
            }
            out.push('\n');
            out.push_str(&format!(
                "  input schema: {}\n",
                compact_json(&descriptor.input_schema)
            ));
        }
        out
    }

    /// Diagnostics for every tool the candidate declared and the project cannot
    /// route.
    fn unrouted(&self, program: &Program) -> Vec<AuthoringDiagnostic> {
        let declared = program
            .tools
            .iter()
            .map(|tool| tool.name.text().to_string());
        match self {
            ToolContext::Unchecked => Vec::new(),
            ToolContext::NoServers => declared
                .map(|name| AuthoringDiagnostic {
                    code: UNROUTED_TOOL,
                    severity: Severity::Error,
                    message: format!(
                        "`{name}` cannot be routed: this project configures no MCP server, \
                         so nothing can serve it"
                    ),
                })
                .collect(),
            ToolContext::Routed(routed) => declared
                .filter(|name| !routed.contains_key(name))
                .map(|name| AuthoringDiagnostic {
                    code: UNROUTED_TOOL,
                    severity: Severity::Error,
                    message: format!(
                        "`{name}` is not one of the tools this project routes; \
                         run `ingot tools` to see them"
                    ),
                })
                .collect(),
        }
    }
}

/// A one-line rendering, so a schema in a prompt stays a schema rather than
/// twenty lines of indentation.
fn compact_json(value: &serde_json::Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "{}".to_string())
}

// --- policy comparison ------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct GrantKey {
    agent: String,
    subject: String,
    qualifier: Option<String>,
}

/// What one `allow` rule hands over.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Grant {
    /// `allow` with no value list: every value of this subject.
    unbounded: bool,
    values: BTreeSet<String>,
}

fn granted(program: &Program) -> BTreeMap<GrantKey, Grant> {
    let mut grants: BTreeMap<GrantKey, Grant> = BTreeMap::new();
    for agent in &program.agents {
        let Some(policy) = &agent.policy else {
            continue;
        };
        for rule in &policy.rules {
            let PolicyAction::Allow {
                qualifier, values, ..
            } = &rule.action
            else {
                continue;
            };
            let key = GrantKey {
                agent: agent.name.text.clone(),
                subject: rule.subject.text.clone(),
                qualifier: qualifier.as_ref().map(|ident| ident.text.clone()),
            };
            let grant = grants.entry(key).or_default();
            if values.is_empty() {
                grant.unbounded = true;
            } else {
                grant
                    .values
                    .extend(values.iter().map(|value| value.plain_text()));
            }
        }
    }
    grants
}

/// Everything the candidate grants that the previous source did not.
fn new_grants(
    previous: &BTreeMap<GrantKey, Grant>,
    candidate: &BTreeMap<GrantKey, Grant>,
) -> Vec<PolicyProposal> {
    let mut proposals = Vec::new();
    for (key, grant) in candidate {
        let before = previous.get(key);
        // An unbounded grant already covers everything a narrower one could add.
        if before.map(|before| before.unbounded).unwrap_or(false) {
            continue;
        }

        let added = if grant.unbounded {
            // Widening a listed grant to an unlisted one is a new grant even
            // though it adds no value to the list.
            Grant {
                unbounded: true,
                values: BTreeSet::new(),
            }
        } else {
            let existing = before.map(|before| &before.values);
            let values: BTreeSet<String> = grant
                .values
                .iter()
                .filter(|value| !existing.map(|set| set.contains(*value)).unwrap_or(false))
                .cloned()
                .collect();
            if values.is_empty() {
                continue;
            }
            Grant {
                unbounded: false,
                values,
            }
        };

        proposals.push(PolicyProposal {
            agent: key.agent.clone(),
            subject: key.subject.clone(),
            action: render_grant(key, &added),
        });
    }
    proposals
}

fn render_grant(key: &GrantKey, grant: &Grant) -> String {
    let mut rendered = "allow".to_string();
    if let Some(qualifier) = &key.qualifier {
        rendered.push(' ');
        rendered.push_str(qualifier);
    }
    if !grant.values.is_empty() {
        rendered.push_str(" [");
        rendered.push_str(
            &grant
                .values
                .iter()
                .map(|value| format!("\"{value}\""))
                .collect::<Vec<_>>()
                .join(", "),
        );
        rendered.push(']');
    }
    rendered
}

#[cfg(test)]
mod tests {
    use super::*;

    const PREVIOUS: &str = r#"
language 0.1
package demo

tool web.search(query: string) -> string[] !network

agent Research(topic: string) -> report<markdown> {
  tools { mcp web.search }
  flow {
    emit report = ask<markdown>("draft ${topic}")
  }
}
"#;

    fn repair(previous: &str, initial: &str, repairs: &[String], max_repairs: usize) -> RepairLoop {
        let mut candidates = FixedCandidates::new(initial, repairs);
        author(
            previous,
            &mut candidates,
            &ToolContext::Unchecked,
            Limits {
                max_repairs,
                accept_policy: false,
            },
        )
        .expect("a file-backed loop cannot fail")
    }

    /// One candidate, no repairs, nothing accepted in advance.
    fn review(previous: &str, candidate: &str) -> RepairLoop {
        repair(previous, candidate, &[], 0)
    }

    fn proposals(previous: &str, candidate: &str) -> Vec<PolicyProposal> {
        match review(previous, candidate).outcome() {
            RepairOutcome::PolicyProposals { proposals } => proposals.clone(),
            _ => Vec::new(),
        }
    }

    #[test]
    fn an_authoring_repair_cannot_accept_its_own_policy_widening() {
        let candidate = r##"
language 0.1
package demo

tool web.search(query: string) -> string[] !network

agent Research(topic: string) -> report<markdown> {
  tools { mcp web.search }
  policy {
    network allow ["example.com"]
  }
  flow {
    hits = call web.search(topic)
    emit report = ask<markdown>("draft", context: hits)
  }
}
"##;

        let loop_ = review(PREVIOUS, candidate);

        assert_eq!(
            loop_.outcome(),
            &RepairOutcome::PolicyProposals {
                proposals: vec![PolicyProposal {
                    agent: "Research".to_string(),
                    subject: "network".to_string(),
                    action: "allow [\"example.com\"]".to_string(),
                }]
            }
        );
        assert!(
            loop_.accepted_source().is_none(),
            "policy widening must stop for explicit operator acceptance"
        );
        assert!(
            loop_.attempts().is_empty(),
            "the loop must stop before the candidate becomes a repair attempt"
        );
    }

    #[test]
    fn an_authoring_repair_without_policy_changes_can_continue() {
        let candidate = PREVIOUS.replace("draft ${topic}", "short draft ${topic}");
        let loop_ = review(PREVIOUS, &candidate);

        assert!(proposals(PREVIOUS, &candidate).is_empty());
        assert_eq!(loop_.accepted_source(), Some(candidate.as_str()));
    }

    #[test]
    fn narrowing_a_policy_is_not_a_proposal() {
        // A candidate that removes a host, or replaces a grant with a denial,
        // asks for nothing. Listing those for acceptance would train an operator
        // to approve the list without reading it.
        let previous = PREVIOUS.replace(
            "  tools { mcp web.search }",
            "  tools { mcp web.search }\n  policy { network allow [\"a.example\", \"b.example\"] }",
        );
        let narrowed = previous.replace(
            "network allow [\"a.example\", \"b.example\"]",
            "network allow [\"a.example\"]",
        );
        let denied = previous.replace(
            "network allow [\"a.example\", \"b.example\"]",
            "network deny",
        );

        assert!(proposals(&previous, &narrowed).is_empty());
        assert!(proposals(&previous, &denied).is_empty());
    }

    #[test]
    fn dropping_the_value_list_is_a_proposal() {
        // `network allow` reaches every host; `network allow ["a.example"]`
        // reaches one. Adding no new string is not the same as adding nothing.
        let previous = PREVIOUS.replace(
            "  tools { mcp web.search }",
            "  tools { mcp web.search }\n  policy { network allow [\"a.example\"] }",
        );
        let widened = previous.replace("network allow [\"a.example\"]", "network allow");

        let widening = proposals(&previous, &widened);
        assert_eq!(widening.len(), 1, "{widening:?}");
        assert_eq!(widening[0].action, "allow");
    }

    #[test]
    fn bounded_repair_stops_at_the_first_compiling_candidate() {
        let broken = PREVIOUS.replace(
            "emit report = ask<markdown>(\"draft ${topic}\")",
            "emit report = missing",
        );
        let fixed = PREVIOUS.replace("draft ${topic}", "repaired draft ${topic}");

        let loop_ = repair(PREVIOUS, &broken, std::slice::from_ref(&fixed), 1);

        assert_eq!(loop_.accepted_source(), Some(fixed.as_str()));
        assert_eq!(loop_.attempts().len(), 2);
        assert!(loop_.attempts()[0].has_errors());
        assert!(!loop_.attempts()[1].has_errors());
    }

    #[test]
    fn bounded_repair_keeps_source_and_structured_diagnostics_at_the_ceiling() {
        let broken = PREVIOUS.replace(
            "emit report = ask<markdown>(\"draft ${topic}\")",
            "emit report = missing",
        );

        let loop_ = repair(PREVIOUS, &broken, &[], 0);

        assert!(loop_.accepted_source().is_none());
        assert_eq!(loop_.outcome(), &RepairOutcome::RetryCeilingReached);
        assert_eq!(loop_.attempts().len(), 1);
        assert_eq!(loop_.attempts()[0].source, broken);
        assert!(loop_.attempts()[0]
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == ingot_diagnostics::codes::UNRESOLVED_NAME));
    }

    #[test]
    fn an_accepted_policy_grant_is_carried_and_still_recorded() {
        let candidate = PREVIOUS.replace(
            "  tools { mcp web.search }",
            "  tools { mcp web.search }\n  policy { network allow [\"example.com\"] }",
        );
        let mut candidates = FixedCandidates::new(&candidate, &[]);
        let loop_ = author(
            PREVIOUS,
            &mut candidates,
            &ToolContext::Unchecked,
            Limits {
                max_repairs: 0,
                accept_policy: true,
            },
        )
        .expect("a file-backed loop cannot fail");

        assert!(loop_.accepted_source().is_some());
        assert_eq!(
            loop_.accepted_proposals(),
            &[PolicyProposal {
                agent: "Research".to_string(),
                subject: "network".to_string(),
                action: "allow [\"example.com\"]".to_string(),
            }],
            "an accepted grant must still be reported, or the record of what was \
             accepted disappears"
        );
    }

    #[test]
    fn an_invented_tool_is_a_repairable_diagnostic() {
        let mut candidates = FixedCandidates::new(PREVIOUS, &[]);
        let loop_ = author(
            "",
            &mut candidates,
            &ToolContext::NoServers,
            Limits {
                max_repairs: 0,
                accept_policy: false,
            },
        )
        .expect("a file-backed loop cannot fail");

        assert_eq!(loop_.outcome(), &RepairOutcome::RetryCeilingReached);
        assert!(
            loop_.attempts()[0]
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == UNROUTED_TOOL),
            "{:?}",
            loop_.attempts()[0].diagnostics
        );
    }

    #[test]
    fn a_routed_tool_is_accepted() {
        let descriptor = ToolDescriptor {
            name: "search".to_string(),
            description: None,
            input_schema: serde_json::json!({"type": "object"}),
            output_schema: None,
        };
        let context = ToolContext::Routed(
            [("web.search".to_string(), descriptor)]
                .into_iter()
                .collect(),
        );
        let mut candidates = FixedCandidates::new(PREVIOUS, &[]);
        let loop_ = author(
            "",
            &mut candidates,
            &context,
            Limits {
                max_repairs: 0,
                accept_policy: false,
            },
        )
        .expect("a file-backed loop cannot fail");

        assert!(loop_.accepted_source().is_some(), "{loop_:?}");
    }

    #[test]
    fn a_credential_shaped_value_ends_the_loop_without_quoting_it() {
        let candidate = PREVIOUS.replace(
            "draft ${topic}",
            "draft ${topic} with api_key=\"sk-live-4f9ac1d3b7e25a86\"",
        );
        let mut candidates = FixedCandidates::new(&candidate, &[]);
        let loop_ = author(
            PREVIOUS,
            &mut candidates,
            &ToolContext::Unchecked,
            Limits {
                max_repairs: 2,
                accept_policy: false,
            },
        )
        .expect("a file-backed loop cannot fail");

        let RepairOutcome::CredentialRefused { finding } = loop_.outcome() else {
            panic!("expected a refusal, got {:?}", loop_.outcome());
        };
        assert_eq!(finding.shape, "a vendor-prefixed API key");
        assert!(
            loop_.attempts().is_empty(),
            "the offending source must not be kept for a repair prompt"
        );
    }

    #[test]
    fn a_fenced_reply_yields_only_the_source() {
        let reply = "Here you go:\n\n```ingot\nlanguage 0.1\n\nagent A() -> b<markdown> {}\n```\n\nHope that helps.";
        assert_eq!(
            extract_source(reply),
            "language 0.1\n\nagent A() -> b<markdown> {}\n"
        );
    }

    #[test]
    fn an_unfenced_reply_is_taken_whole() {
        assert_eq!(extract_source("language 0.1\n"), "language 0.1\n");
    }
}
