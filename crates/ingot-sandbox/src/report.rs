//! Printing a plan.
//!
//! The report exists so that a boundary can be read before it is trusted. It
//! shows every mount with the policy line it came from, so the answer to "why
//! can this agent see that?" is on the same line as the fact.

use std::fmt::Write;

use crate::plan::{Network, SandboxPlan};

/// A human-readable plan, without a trailing newline.
pub fn render(plan: &SandboxPlan) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "server `{}`  (for agent {})", plan.server, plan.agent);

    if plan.mounts.is_empty() {
        let _ = writeln!(
            out,
            "  mount    (none)                              the workspace is not visible"
        );
    }
    let width = plan
        .mounts
        .iter()
        .map(|mount| mount.guest.len())
        .max()
        .unwrap_or(0)
        .max(12);
    for mount in &plan.mounts {
        let _ = writeln!(
            out,
            "  mount    {:<width$}  {}   {}",
            mount.guest,
            if mount.writable { "rw" } else { "ro" },
            mount.from,
            width = width
        );
    }

    match &plan.network {
        Network::None => {
            let _ = writeln!(out, "  network  none");
        }
        Network::Unrestricted => {
            let _ = writeln!(out, "  network  unrestricted");
        }
        Network::Hosts { hosts } => {
            let _ = writeln!(
                out,
                "  network  on, allowlist not enforced ({})",
                hosts.join(", ")
            );
        }
    }

    let _ = writeln!(
        out,
        "  env      {}",
        if plan.env.is_empty() {
            "(none)".to_string()
        } else {
            plan.env.join(", ")
        }
    );
    let _ = writeln!(out, "  workdir  {}", plan.workdir);

    if !plan.unenforceable.is_empty() {
        let _ = writeln!(out);
        let _ = writeln!(out, "  cannot enforce:");
        for note in &plan.unenforceable {
            let _ = writeln!(out, "    {}", note.policy);
            for line in wrap(&note.reason, 68) {
                let _ = writeln!(out, "      {line}");
            }
        }
    }

    out.trim_end().to_string()
}

/// Wrap at a word boundary. Reasons are prose and a terminal is not infinite.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        if !current.is_empty() && current.len() + 1 + word.len() > width {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{Mount, Unenforceable};
    use std::path::PathBuf;

    fn plan() -> SandboxPlan {
        SandboxPlan {
            agent: "review.CodeReviewTeam".into(),
            server: "repo".into(),
            mounts: vec![
                Mount {
                    path: "src".into(),
                    host: PathBuf::from("/srv/checkout/src"),
                    guest: "/workspace/src".into(),
                    writable: false,
                    from: "filesystem_read allow [\"src\", \"crates\"]".into(),
                },
                Mount {
                    path: "target/review".into(),
                    host: PathBuf::from("/srv/checkout/target/review"),
                    guest: "/workspace/target/review".into(),
                    writable: true,
                    from: "filesystem_write allow [\"target/review\"]".into(),
                },
            ],
            network: Network::None,
            env: Vec::new(),
            workdir: "/workspace".into(),
            unenforceable: Vec::new(),
        }
    }

    #[test]
    fn every_mount_carries_the_policy_line_it_came_from() {
        let text = render(&plan());
        assert!(text.contains("/workspace/src"), "{text}");
        assert!(text.contains("filesystem_read allow"), "{text}");
        assert!(text.contains("rw"), "{text}");
        assert!(text.contains("network  none"), "{text}");
    }

    #[test]
    fn an_empty_boundary_says_so_rather_than_printing_nothing() {
        let mut plan = plan();
        plan.mounts.clear();
        let text = render(&plan);
        assert!(text.contains("not visible"), "{text}");
    }

    #[test]
    fn what_cannot_be_enforced_is_printed_last_and_in_full() {
        let mut plan = plan();
        plan.network = Network::Hosts {
            hosts: vec!["arxiv.org".into()],
        };
        plan.unenforceable = vec![Unenforceable {
            policy: "network allow [\"arxiv.org\"]".into(),
            reason: "a host allowlist needs an egress proxy; the boundary can give the server \
                     a network or withhold one, and it is giving it one"
                .into(),
        }];

        let text = render(&plan);
        assert!(text.contains("allowlist not enforced"), "{text}");
        assert!(text.contains("cannot enforce:"), "{text}");
        assert!(text.contains("egress proxy"), "{text}");
        assert!(
            text.lines().all(|line| line.len() <= 100),
            "reasons must wrap:\n{text}"
        );
    }

    #[test]
    fn the_report_has_no_trailing_blank_line() {
        let text = render(&plan());
        assert_eq!(text, text.trim_end());
    }
}
