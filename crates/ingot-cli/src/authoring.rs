//! Guardrails for model-assisted `.ing` authoring.
//!
//! The authoring model is allowed to propose ordinary source. It is not allowed
//! to approve new capabilities for itself. This module keeps that separation
//! before a future provider-backed `ingot new` loop can apply compiler repairs.

use std::collections::BTreeSet;

use ingot_compiler::compile_source;
use ingot_syntax::{PolicyAction, Program};

/// A reviewed candidate source revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AuthoringReview {
    candidate_source: String,
    policy_proposals: Vec<PolicyProposal>,
}

impl AuthoringReview {
    /// The source text an automatic repair loop may continue with.
    ///
    /// Policy proposals force an operator-visible stop. The authoring model may
    /// explain them, but it cannot silently fold them into its own repair.
    pub(crate) fn automatic_repair_source(&self) -> Option<&str> {
        if self.policy_proposals.is_empty() {
            Some(&self.candidate_source)
        } else {
            None
        }
    }

    pub(crate) fn policy_proposals(&self) -> &[PolicyProposal] {
        &self.policy_proposals
    }
}

/// A policy rule newly requested by candidate source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PolicyProposal {
    pub(crate) agent: String,
    pub(crate) subject: String,
    pub(crate) action: String,
}

/// Compare a model-proposed source revision against the previous source.
pub(crate) fn review_candidate(previous_source: &str, candidate_source: &str) -> AuthoringReview {
    let previous = compile_source("previous.ing", previous_source);
    let candidate = compile_source("candidate.ing", candidate_source);
    let previous_policy = policy_rules(&previous.program);
    let candidate_policy = policy_rules(&candidate.program);

    let policy_proposals = candidate_policy
        .difference(&previous_policy)
        .map(|rule| PolicyProposal {
            agent: rule.agent.clone(),
            subject: rule.subject.clone(),
            action: rule.action.clone(),
        })
        .collect();

    AuthoringReview {
        candidate_source: candidate_source.to_string(),
        policy_proposals,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct PolicyRuleKey {
    agent: String,
    subject: String,
    action: String,
}

fn policy_rules(program: &Program) -> BTreeSet<PolicyRuleKey> {
    let mut rules = BTreeSet::new();
    for agent in &program.agents {
        let Some(policy) = &agent.policy else {
            continue;
        };
        for rule in &policy.rules {
            rules.insert(PolicyRuleKey {
                agent: agent.name.text.clone(),
                subject: rule.subject.text.clone(),
                action: render_action(&rule.action),
            });
        }
    }
    rules
}

fn render_action(action: &PolicyAction) -> String {
    match action {
        PolicyAction::Allow {
            qualifier, values, ..
        } => {
            let mut rendered = "allow".to_string();
            if let Some(qualifier) = qualifier {
                rendered.push(' ');
                rendered.push_str(&qualifier.text);
            }
            if !values.is_empty() {
                let mut values: Vec<String> =
                    values.iter().map(|value| value.plain_text()).collect();
                values.sort();
                rendered.push_str(" [");
                rendered.push_str(
                    &values
                        .iter()
                        .map(|value| format!("\"{value}\""))
                        .collect::<Vec<_>>()
                        .join(", "),
                );
                rendered.push(']');
            }
            rendered
        }
        PolicyAction::Deny { qualifier, .. } => qualifier
            .as_ref()
            .map(|qualifier| format!("deny {}", qualifier.text))
            .unwrap_or_else(|| "deny".to_string()),
        PolicyAction::RequireApproval { .. } => "require approval".to_string(),
    }
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

        let review = review_candidate(PREVIOUS, candidate);

        assert_eq!(
            review.policy_proposals(),
            &[PolicyProposal {
                agent: "Research".to_string(),
                subject: "network".to_string(),
                action: "allow [\"example.com\"]".to_string(),
            }]
        );
        assert!(
            review.automatic_repair_source().is_none(),
            "policy widening must stop for explicit operator acceptance"
        );
    }

    #[test]
    fn an_authoring_repair_without_policy_changes_can_continue() {
        let candidate = PREVIOUS.replace("draft ${topic}", "short draft ${topic}");
        let review = review_candidate(PREVIOUS, &candidate);

        assert!(review.policy_proposals().is_empty());
        assert_eq!(review.automatic_repair_source(), Some(candidate.as_str()));
    }
}
