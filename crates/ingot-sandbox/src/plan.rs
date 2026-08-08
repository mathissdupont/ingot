//! Turning a `policy` block into a boundary.
//!
//! This is a pure function of the artifact, the workspace and the server's
//! name. It starts nothing, so it can be inspected before a run and tested
//! anywhere — including on a machine with no container runtime, which is most
//! of them.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Component, Path, PathBuf};

use ingot_ir::{AgentIr, Decision, PolicyRule};
use serde::{Deserialize, Serialize};

/// Where the workspace appears inside the boundary.
///
/// Fixed, so that a tool sees the same paths wherever the artifact runs. An
/// agent that says `src` gets `/workspace/src`, on every machine.
pub const GUEST_WORKSPACE: &str = "/workspace";

/// The `server` a plan names when the boundary is for the run itself.
///
/// Stage 1 plans a boundary per (tool server, agent) pair. Stage 2 plans one for
/// the run, which is not a server and has no name in the manifest, so it gets a
/// reserved one — a manifest cannot collide with it because `[[mcp.server]]`
/// names are identifiers and this is not.
pub const RUN_SUBJECT: &str = "(the run)";

/// Policy subjects that name filesystem paths.
const READ: &str = "filesystem_read";
const WRITE: &str = "filesystem_write";
const NETWORK: &str = "network";
const EXTERNAL_WRITE: &str = "external_write";

/// One directory made visible inside the boundary.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mount {
    /// The path relative to the workspace, as the policy wrote it.
    pub path: String,
    /// Where it is on this machine.
    pub host: PathBuf,
    /// Where it appears inside.
    pub guest: String,
    pub writable: bool,
    /// The policy line this came from, for the report.
    pub from: String,
}

/// What the boundary does about the network.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "camelCase")]
pub enum Network {
    /// No network at all. There is nothing to connect to.
    None,
    /// Network on. The artifact named hosts; nothing enforces the list.
    Hosts { hosts: Vec<String> },
    /// Network on, with the artifact naming no hosts.
    Unrestricted,
}

/// Something the artifact asks for that a boundary cannot deliver.
///
/// Recorded rather than dropped. A restriction that quietly does not apply is
/// worse than one that never existed, because a sandbox was switched on and
/// somebody believed it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Unenforceable {
    /// The policy line, as the source wrote it.
    pub policy: String,
    pub reason: String,
}

/// Why a plan could not be produced at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanError {
    /// A policy path is absolute, or climbs out of the workspace.
    PathEscapesWorkspace { subject: String, path: String },
    /// A read mount names something that is not there.
    ReadPathMissing {
        subject: String,
        path: String,
        host: PathBuf,
    },
    /// A policy path is empty.
    EmptyPath { subject: String },
}

impl fmt::Display for PlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PlanError::PathEscapesWorkspace { subject, path } => write!(
                f,
                "`{subject} allow [\"{path}\"]` leaves the workspace; \
                 a policy path is relative to the workspace and may not be absolute or contain `..`"
            ),
            PlanError::ReadPathMissing {
                subject,
                path,
                host,
            } => write!(
                f,
                "`{subject} allow [\"{path}\"]` names {} , which does not exist\n  \
                 mounting an empty directory would make a missing checkout look like an empty one",
                host.display()
            ),
            PlanError::EmptyPath { subject } => {
                write!(f, "`{subject}` has an empty path in its allow list")
            }
        }
    }
}

impl std::error::Error for PlanError {}

/// The boundary one tool server would run inside.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SandboxPlan {
    /// The agent whose policy this came from.
    pub agent: String,
    /// The configured server this applies to.
    pub server: String,
    /// Sorted, so two runs of the same artifact produce the same plan.
    pub mounts: Vec<Mount>,
    pub network: Network,
    /// Environment variable **names** that cross the boundary. Never values.
    pub env: Vec<String>,
    /// The working directory inside.
    pub workdir: String,
    pub unenforceable: Vec<Unenforceable>,
}

impl SandboxPlan {
    /// Whether everything the artifact asks for is actually enforced.
    pub fn is_fully_enforced(&self) -> bool {
        self.unenforceable.is_empty()
    }

    /// The host paths that must exist before the boundary can be created.
    /// Write mounts are created; read mounts were required to exist already.
    pub fn directories_to_create(&self) -> Vec<&Path> {
        self.mounts
            .iter()
            .filter(|mount| mount.writable && !mount.host.exists())
            .map(|mount| mount.host.as_path())
            .collect()
    }
}

/// Derive the boundary for `server`, from `agent`'s policy, over `workspace`.
pub fn plan(
    agent: &AgentIr,
    server: &str,
    workspace: &Path,
    pass_env: &[String],
) -> Result<SandboxPlan, PlanError> {
    let mut mounts: BTreeMap<String, Mount> = BTreeMap::new();

    // Read first, then write, so that a path granted both ends up writable:
    // read-write is the superset, and refusing the overlap would mean an
    // artifact could not read what it writes.
    for (subject, writable) in [(READ, false), (WRITE, true)] {
        let Some(rule) = agent.policy.get(subject) else {
            continue;
        };
        if rule.decision != Decision::Allow {
            continue;
        }
        for path in &rule.values {
            let host = resolve(workspace, subject, path)?;
            if !writable && !host.exists() {
                return Err(PlanError::ReadPathMissing {
                    subject: subject.to_string(),
                    path: path.clone(),
                    host,
                });
            }
            let normalised = normalise(path);
            mounts.insert(
                normalised.clone(),
                Mount {
                    guest: guest_path(&normalised),
                    path: normalised,
                    host,
                    writable,
                    from: describe(subject, rule),
                },
            );
        }
    }

    let mut unenforceable = Vec::new();
    let network = match agent.policy.get(NETWORK) {
        Some(rule) if rule.decision == Decision::Allow && rule.values.is_empty() => {
            unenforceable.push(Unenforceable {
                policy: describe(NETWORK, rule),
                reason: "the artifact names no hosts, so the boundary can only be \
                         all-or-nothing and it is all"
                    .to_string(),
            });
            Network::Unrestricted
        }
        Some(rule) if rule.decision == Decision::Allow => {
            unenforceable.push(Unenforceable {
                policy: describe(NETWORK, rule),
                reason: "a host allowlist needs an egress proxy; the boundary can give the \
                         server a network or withhold one, and it is giving it one"
                    .to_string(),
            });
            Network::Hosts {
                hosts: rule.values.clone(),
            }
        }
        // Deny, require-approval, or no rule at all. Default-deny means the
        // absence of a rule is the absence of a network.
        _ => Network::None,
    };

    if let Some(rule) = agent.policy.get(EXTERNAL_WRITE) {
        if rule.decision != Decision::Deny {
            unenforceable.push(Unenforceable {
                policy: describe(EXTERNAL_WRITE, rule),
                reason: "a boundary cannot tell an intended external write from any other; \
                         the effect check, and any approval gate, still apply"
                    .to_string(),
            });
        }
    }

    unenforceable.sort();

    let mut env: Vec<String> = pass_env.to_vec();
    env.sort();
    env.dedup();

    Ok(SandboxPlan {
        agent: agent.agent.clone(),
        server: server.to_string(),
        mounts: mounts.into_values().collect(),
        network,
        env,
        workdir: GUEST_WORKSPACE.to_string(),
        unenforceable,
    })
}

/// `filesystem_read allow ["src", "crates"]`, as the source wrote it.
fn describe(subject: &str, rule: &PolicyRule) -> String {
    let decision = match rule.decision {
        Decision::Allow => "allow",
        Decision::Deny => "deny",
        Decision::RequireApproval => "require approval",
    };
    let mut text = format!("{subject} {decision}");
    if let Some(qualifier) = &rule.qualifier {
        text.push(' ');
        text.push_str(qualifier);
    }
    if !rule.values.is_empty() {
        let quoted: Vec<String> = rule.values.iter().map(|v| format!("\"{v}\"")).collect();
        text.push_str(&format!(" [{}]", quoted.join(", ")));
    }
    text
}

/// A policy path, as a workspace-relative path with no `.` segments.
fn normalise(path: &str) -> String {
    let trimmed = path.trim().trim_end_matches(['/', '\\']);
    let parts: Vec<&str> = trimmed
        .split(['/', '\\'])
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();
    if parts.is_empty() {
        ".".to_string()
    } else {
        parts.join("/")
    }
}

fn guest_path(normalised: &str) -> String {
    if normalised == "." {
        GUEST_WORKSPACE.to_string()
    } else {
        format!("{GUEST_WORKSPACE}/{normalised}")
    }
}

/// Where a policy path is on this machine, or why it is not allowed to be.
fn resolve(workspace: &Path, subject: &str, path: &str) -> Result<PathBuf, PlanError> {
    if path.trim().is_empty() {
        return Err(PlanError::EmptyPath {
            subject: subject.to_string(),
        });
    }

    let candidate = Path::new(path);
    if candidate.is_absolute() || candidate.has_root() {
        return Err(PlanError::PathEscapesWorkspace {
            subject: subject.to_string(),
            path: path.to_string(),
        });
    }
    for component in candidate.components() {
        match component {
            Component::Normal(_) | Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(PlanError::PathEscapesWorkspace {
                    subject: subject.to_string(),
                    path: path.to_string(),
                })
            }
        }
    }

    let normalised = normalise(path);
    Ok(if normalised == "." {
        workspace.to_path_buf()
    } else {
        workspace.join(&normalised)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ingot_ir::{Budget, ModelRequirement, Requirements, IR_VERSION};

    fn agent_with(policy: &[(&str, Decision, &[&str])]) -> AgentIr {
        let mut ir = AgentIr {
            ir_version: IR_VERSION.to_string(),
            language: "0.1".to_string(),
            agent: "test.Agent".to_string(),
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
            effects: Vec::new(),
            entry: None,
            nodes: Vec::new(),
        };
        for (subject, decision, values) in policy {
            ir.policy.insert(
                (*subject).to_string(),
                PolicyRule {
                    decision: *decision,
                    values: values.iter().map(|v| (*v).to_string()).collect(),
                    qualifier: None,
                },
            );
        }
        ir
    }

    fn workspace(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("ingot-plan-{label}"));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("creating a workspace");
        root.canonicalize().expect("canonicalising it")
    }

    #[test]
    fn a_read_grant_becomes_a_read_only_mount() {
        let root = workspace("read");
        std::fs::create_dir_all(root.join("src")).unwrap();
        let agent = agent_with(&[(READ, Decision::Allow, &["src"])]);

        let plan = plan(&agent, "files", &root, &[]).unwrap();
        assert_eq!(plan.mounts.len(), 1);
        assert_eq!(plan.mounts[0].guest, "/workspace/src");
        assert!(!plan.mounts[0].writable);
        assert_eq!(plan.mounts[0].from, "filesystem_read allow [\"src\"]");
    }

    #[test]
    fn no_rule_means_no_mount_and_no_network() {
        let root = workspace("empty");
        let plan = plan(&agent_with(&[]), "files", &root, &[]).unwrap();
        assert!(plan.mounts.is_empty(), "{:?}", plan.mounts);
        assert_eq!(plan.network, Network::None);
        assert!(plan.is_fully_enforced());
    }

    #[test]
    fn a_denied_subject_grants_nothing() {
        let root = workspace("denied");
        std::fs::create_dir_all(root.join("src")).unwrap();
        let agent = agent_with(&[(READ, Decision::Deny, &["src"])]);
        assert!(plan(&agent, "files", &root, &[]).unwrap().mounts.is_empty());
    }

    #[test]
    fn a_path_granted_both_ways_is_writable() {
        // Read-write is the superset. Refusing the overlap would mean an agent
        // could not read what it writes.
        let root = workspace("both");
        std::fs::create_dir_all(root.join("out")).unwrap();
        let agent = agent_with(&[
            (READ, Decision::Allow, &["out"]),
            (WRITE, Decision::Allow, &["out"]),
        ]);

        let plan = plan(&agent, "files", &root, &[]).unwrap();
        assert_eq!(plan.mounts.len(), 1, "{:?}", plan.mounts);
        assert!(plan.mounts[0].writable);
    }

    #[test]
    fn a_write_path_that_does_not_exist_yet_is_planned_for_creation() {
        let root = workspace("newdir");
        let agent = agent_with(&[(WRITE, Decision::Allow, &["target/review"])]);

        let plan = plan(&agent, "files", &root, &[]).unwrap();
        assert_eq!(plan.mounts[0].guest, "/workspace/target/review");
        assert_eq!(plan.directories_to_create().len(), 1);
    }

    #[test]
    fn a_read_path_that_does_not_exist_is_refused() {
        let root = workspace("missing");
        let agent = agent_with(&[(READ, Decision::Allow, &["src"])]);

        let error = plan(&agent, "files", &root, &[]).unwrap_err();
        assert!(
            matches!(error, PlanError::ReadPathMissing { .. }),
            "{error}"
        );
        assert!(error.to_string().contains("empty"), "{error}");
    }

    #[test]
    fn a_path_that_leaves_the_workspace_is_refused() {
        let root = workspace("escape");
        for path in ["../secrets", "/etc", "src/../../elsewhere"] {
            let agent = agent_with(&[(READ, Decision::Allow, &[path])]);
            let error = plan(&agent, "files", &root, &[]).unwrap_err();
            assert!(
                matches!(error, PlanError::PathEscapesWorkspace { .. }),
                "{path}: {error}"
            );
        }
    }

    #[test]
    fn network_deny_and_an_absent_rule_both_mean_no_network() {
        let root = workspace("nonet");
        for policy in [vec![(NETWORK, Decision::Deny, &[] as &[&str])], vec![]] {
            let plan = plan(&agent_with(&policy), "files", &root, &[]).unwrap();
            assert_eq!(plan.network, Network::None);
            assert!(plan.is_fully_enforced());
        }
    }

    #[test]
    fn a_host_allowlist_is_reported_as_unenforceable_rather_than_pretended() {
        let root = workspace("hosts");
        let agent = agent_with(&[(NETWORK, Decision::Allow, &["arxiv.org", "github.com"])]);

        let plan = plan(&agent, "files", &root, &[]).unwrap();
        assert_eq!(
            plan.network,
            Network::Hosts {
                hosts: vec!["arxiv.org".into(), "github.com".into()]
            }
        );
        assert!(!plan.is_fully_enforced());
        let note = &plan.unenforceable[0];
        assert!(note.policy.contains("arxiv.org"), "{note:?}");
        assert!(note.reason.contains("egress proxy"), "{note:?}");
    }

    #[test]
    fn external_write_is_named_as_something_a_boundary_cannot_judge() {
        let root = workspace("external");
        let agent = agent_with(&[(EXTERNAL_WRITE, Decision::RequireApproval, &[])]);

        let gated = plan(&agent, "files", &root, &[]).unwrap();
        assert!(!gated.is_fully_enforced());
        assert!(gated.unenforceable[0]
            .policy
            .contains("external_write require approval"));

        // Denied is enforceable: the effect never runs.
        let denied = agent_with(&[(EXTERNAL_WRITE, Decision::Deny, &[])]);
        assert!(plan(&denied, "files", &root, &[])
            .unwrap()
            .is_fully_enforced());
    }

    #[test]
    fn only_named_environment_variables_cross_and_only_by_name() {
        let root = workspace("env");
        let plan = plan(
            &agent_with(&[]),
            "files",
            &root,
            &["BRAVE_API_KEY".to_string(), "BRAVE_API_KEY".to_string()],
        )
        .unwrap();
        assert_eq!(plan.env, vec!["BRAVE_API_KEY".to_string()]);
        let rendered = serde_json::to_string(&plan).unwrap();
        assert!(!rendered.contains("ANTHROPIC"), "{rendered}");
    }

    #[test]
    fn the_plan_is_the_same_however_the_policy_spelled_its_paths() {
        let root = workspace("normalise");
        std::fs::create_dir_all(root.join("src")).unwrap();
        let plain = plan(
            &agent_with(&[(READ, Decision::Allow, &["src"])]),
            "files",
            &root,
            &[],
        )
        .unwrap();
        let decorated = plan(
            &agent_with(&[(READ, Decision::Allow, &["./src/"])]),
            "files",
            &root,
            &[],
        )
        .unwrap();
        // The boundary is identical. `from` deliberately is not: it echoes the
        // policy line as the source wrote it, so a report can be checked
        // against the file it came from.
        let boundary = |plan: &SandboxPlan| -> Vec<(PathBuf, String, bool)> {
            plan.mounts
                .iter()
                .map(|mount| (mount.host.clone(), mount.guest.clone(), mount.writable))
                .collect()
        };
        assert_eq!(boundary(&plain), boundary(&decorated));
        assert_eq!(
            decorated.mounts[0].from,
            "filesystem_read allow [\"./src/\"]"
        );
    }

    #[test]
    fn the_workspace_itself_can_be_the_mount() {
        let root = workspace("dot");
        let agent = agent_with(&[(READ, Decision::Allow, &["."])]);
        let plan = plan(&agent, "files", &root, &[]).unwrap();
        assert_eq!(plan.mounts[0].guest, GUEST_WORKSPACE);
        assert_eq!(plan.mounts[0].host, root);
    }

    #[test]
    fn a_plan_round_trips_through_json() {
        let root = workspace("json");
        std::fs::create_dir_all(root.join("src")).unwrap();
        let agent = agent_with(&[
            (READ, Decision::Allow, &["src"]),
            (NETWORK, Decision::Allow, &["arxiv.org"]),
        ]);
        let plan = plan(&agent, "files", &root, &[]).unwrap();
        let text = serde_json::to_string_pretty(&plan).unwrap();
        let parsed: SandboxPlan = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed, plan);
    }
}
