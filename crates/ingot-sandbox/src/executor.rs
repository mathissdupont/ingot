//! Turning a plan into a container invocation.
//!
//! The plan says what the boundary is; this says how to ask a container runtime
//! for it. The interesting half is [`invocation`], which is a pure function
//! from a plan to an argument vector — so what we ask for is assertable in a
//! test without a daemon, on a machine that has none.
//!
//! An MCP server speaks on stdin and stdout, and so does `docker run -i`, which
//! is why containing one needs no new transport: the command changes, the
//! protocol does not.

use std::fmt;
use std::path::Path;
use std::process::Command;

use crate::plan::{Network, SandboxPlan};

/// A container runtime, in the order they are looked for.
///
/// Both are driven through the same arguments for everything used here.
pub const RUNTIMES: &[&str] = &["docker", "podman"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutorError {
    /// No container runtime on `PATH`.
    NoRuntime,
    /// A runtime is installed but not answering.
    RuntimeUnavailable { runtime: String, reason: String },
    /// The manifest does not say what image to run this server in.
    NoImage { server: String },
}

impl fmt::Display for ExecutorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExecutorError::NoRuntime => write!(
                f,
                "no container runtime found; looked for {}",
                RUNTIMES.join(" and ")
            ),
            ExecutorError::RuntimeUnavailable { runtime, reason } => write!(
                f,
                "`{runtime}` is installed but not usable: {reason}\n  \
                 start it, or run without --sandbox and accept that the policy is checked \
                 rather than enforced"
            ),
            ExecutorError::NoImage { server } => write!(
                f,
                "MCP server `{server}` has no `image`, so there is nothing to contain it in\n  \
                 add `image = \"...\"` to its [[mcp.server]] entry; the image is the operator's \
                 choice because the server is the operator's program"
            ),
        }
    }
}

impl std::error::Error for ExecutorError {}

/// A container runtime that answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Runtime {
    pub program: String,
    /// What it said its version was, for the run log.
    pub version: String,
}

/// The only kind of container the boundary can be expressed in.
///
/// A read-only root filesystem, `--cap-drop`, `--network none` and a POSIX
/// working directory are all Linux-container features. A Windows-container
/// daemon accepts some of them and rejects others, which would mean a boundary
/// that is partly applied — the one outcome worth refusing over.
const REQUIRED_OS: &str = "linux";

/// Find a usable container runtime.
///
/// Asks each candidate for its version rather than merely looking on `PATH`:
/// Docker Desktop leaves a working `docker` command behind an engine that is
/// frequently not running, and "command not found" and "daemon not running"
/// need different advice.
pub fn detect() -> Result<Runtime, ExecutorError> {
    let mut last: Option<ExecutorError> = None;

    for program in RUNTIMES {
        let output = Command::new(program)
            .args(["version", "--format", "{{.Server.Version}}"])
            .output();

        match output {
            Ok(output) if output.status.success() => {
                let version = String::from_utf8_lossy(&output.stdout).trim().to_string();

                // Docker Desktop can be pointed at either a Linux or a Windows
                // daemon, and the same `docker` command answers for both.
                if let Some(os) = server_os(program) {
                    if os != REQUIRED_OS {
                        last = Some(ExecutorError::RuntimeUnavailable {
                            runtime: (*program).to_string(),
                            reason: format!(
                                "it is serving {os} containers, and the boundary needs {REQUIRED_OS} \
                                 ones — a read-only root filesystem and `--network none` are not \
                                 available otherwise, so the boundary would be partly applied"
                            ),
                        });
                        continue;
                    }
                }

                return Ok(Runtime {
                    program: (*program).to_string(),
                    version: if version.is_empty() {
                        "unknown".to_string()
                    } else {
                        version
                    },
                });
            }
            Ok(output) => {
                let reason = String::from_utf8_lossy(&output.stderr)
                    .lines()
                    .next()
                    .unwrap_or("it reported a failure")
                    .trim()
                    .to_string();
                last = Some(ExecutorError::RuntimeUnavailable {
                    runtime: (*program).to_string(),
                    reason,
                });
            }
            // Not installed. Keep looking.
            Err(_) => {}
        }
    }

    Err(last.unwrap_or(ExecutorError::NoRuntime))
}

/// Whether an image is present in a runtime's local store, without starting it.
///
/// The daemon has already answered [`detect`]; an unexpected inspection error
/// therefore means it stopped being usable rather than that the image is
/// absent. Keeping those cases separate gives `ingot doctor` an actionable
/// answer without turning a read-only preflight into a pull or a build.
pub fn image_exists(runtime: &Runtime, image: &str) -> Result<bool, ExecutorError> {
    let output = Command::new(&runtime.program)
        .args(["image", "inspect", image, "--format", "{{.Id}}"])
        .output()
        .map_err(|error| ExecutorError::RuntimeUnavailable {
            runtime: runtime.program.clone(),
            reason: error.to_string(),
        })?;

    if output.status.success() {
        return Ok(true);
    }

    let reason = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let normalised = reason.to_ascii_lowercase();
    if normalised.contains("no such image")
        || normalised.contains("not found")
        || normalised.contains("no such object")
        || normalised.contains("image not known")
    {
        return Ok(false);
    }

    Err(ExecutorError::RuntimeUnavailable {
        runtime: runtime.program.clone(),
        reason: if reason.is_empty() {
            format!("could not inspect image `{image}`")
        } else {
            reason
        },
    })
}

/// The arguments that ask a runtime for this plan.
///
/// Returns everything after the runtime's own name, so the caller spawns
/// `runtime.program` with these.
pub fn invocation(
    plan: &SandboxPlan,
    image: &str,
    command: &[String],
    workspace: &Path,
) -> Vec<String> {
    let mut args = vec![
        "run".to_string(),
        // Removed when it exits, and stdin attached: an MCP server is a
        // conversation on the standard streams.
        "--rm".to_string(),
        "-i".to_string(),
        // Nothing in this box needs to gain a privilege it did not start with.
        "--security-opt".to_string(),
        "no-new-privileges".to_string(),
        "--cap-drop".to_string(),
        "ALL".to_string(),
        // The mounts are the writable surface. Everything else, including the
        // image's own filesystem, is not.
        "--read-only".to_string(),
        "--tmpfs".to_string(),
        "/tmp".to_string(),
    ];

    match &plan.network {
        Network::None => {
            args.push("--network".to_string());
            args.push("none".to_string());
        }
        // The plan already recorded that an allowlist is not enforced. Giving
        // the container a network is the honest half of that.
        Network::Hosts { .. } | Network::Unrestricted => {}
    }

    if let Some(user) = host_user(workspace) {
        // So that files a write mount receives belong to whoever owns the
        // workspace, rather than to root.
        args.push("--user".to_string());
        args.push(user);
    }

    for mount in &plan.mounts {
        args.push("--volume".to_string());
        let host = mount_source(&mount.host);
        args.push(if mount.writable {
            format!("{host}:{}", mount.guest)
        } else {
            format!("{host}:{}:ro", mount.guest)
        });
    }

    // By name. The value is read by the runtime from our own environment, so it
    // never appears in an argument vector, a process listing or a log.
    for name in &plan.env {
        args.push("--env".to_string());
        args.push(name.clone());
    }

    args.push("--workdir".to_string());
    args.push(plan.workdir.clone());

    args.push(image.to_string());
    args.extend(command.iter().cloned());
    args
}

/// What kind of container this daemon serves, when it will say.
///
/// `None` means it did not answer the question — Podman has no such field —
/// and an unanswered question is not evidence of a problem.
fn server_os(program: &str) -> Option<String> {
    let output = Command::new(program)
        .args(["version", "--format", "{{.Server.Os}}"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let os = String::from_utf8_lossy(&output.stdout)
        .trim()
        .to_ascii_lowercase();
    if os.is_empty() || os.contains("<no value>") {
        None
    } else {
        Some(os)
    }
}

/// A host path in the form a container runtime will accept.
///
/// Canonicalising on Windows yields an extended-length path — `\\?\C:\…` — and
/// a runtime parses a volume specification by splitting on colons, so `\\?\C:`
/// is one colon too many and the whole spec is rejected. Stripping the prefix
/// is not cosmetic: without it every mount on Windows fails.
fn mount_source(path: &Path) -> String {
    let text = path.display().to_string();
    if let Some(share) = text.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{share}");
    }
    text.strip_prefix(r"\\?\").unwrap_or(&text).to_string()
}

/// `uid:gid` of the workspace owner, where that is a thing.
#[cfg(unix)]
fn host_user(workspace: &Path) -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::metadata(workspace).ok()?;
    Some(format!("{}:{}", metadata.uid(), metadata.gid()))
}

/// Windows containers have no numeric owner to inherit, and Docker Desktop
/// already maps the write back to the invoking user.
#[cfg(not(unix))]
fn host_user(_workspace: &Path) -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{Mount, Unenforceable};
    use std::path::PathBuf;

    fn plan(network: Network) -> SandboxPlan {
        SandboxPlan {
            agent: "review.CodeReviewTeam".into(),
            server: "repo".into(),
            mounts: vec![
                Mount {
                    path: "crates".into(),
                    host: PathBuf::from("/srv/checkout/crates"),
                    guest: "/workspace/crates".into(),
                    writable: false,
                    from: "filesystem_read allow [\"crates\"]".into(),
                },
                Mount {
                    path: "target/review".into(),
                    host: PathBuf::from("/srv/checkout/target/review"),
                    guest: "/workspace/target/review".into(),
                    writable: true,
                    from: "filesystem_write allow [\"target/review\"]".into(),
                },
            ],
            network,
            env: vec!["GITHUB_TOKEN".into()],
            workdir: "/workspace".into(),
            unenforceable: Vec::new(),
        }
    }

    fn args_of(network: Network) -> Vec<String> {
        invocation(
            &plan(network),
            "ingot/mcp-fs:0.2",
            &[
                "ingot-mcp-fs".to_string(),
                "--root".to_string(),
                ".".to_string(),
            ],
            Path::new("/srv/checkout"),
        )
    }

    /// The value that follows `flag`, wherever it appears.
    fn value_after(args: &[String], flag: &str) -> Option<String> {
        args.iter()
            .position(|arg| arg == flag)
            .and_then(|index| args.get(index + 1))
            .cloned()
    }

    fn values_after(args: &[String], flag: &str) -> Vec<String> {
        args.iter()
            .enumerate()
            .filter(|(_, arg)| arg.as_str() == flag)
            .filter_map(|(index, _)| args.get(index + 1).cloned())
            .collect()
    }

    #[test]
    fn a_read_mount_is_asked_for_read_only() {
        let args = args_of(Network::None);
        let volumes = values_after(&args, "--volume");
        assert!(
            volumes.contains(&"/srv/checkout/crates:/workspace/crates:ro".to_string()),
            "{volumes:?}"
        );
    }

    #[test]
    fn a_write_mount_is_asked_for_writable() {
        let volumes = values_after(&args_of(Network::None), "--volume");
        assert!(
            volumes.contains(&"/srv/checkout/target/review:/workspace/target/review".to_string()),
            "{volumes:?}"
        );
        assert!(
            !volumes
                .iter()
                .any(|volume| volume.ends_with("target/review:ro")),
            "{volumes:?}"
        );
    }

    #[test]
    fn network_deny_becomes_no_network_at_all() {
        assert_eq!(
            value_after(&args_of(Network::None), "--network"),
            Some("none".to_string())
        );
    }

    #[test]
    fn a_network_the_plan_could_not_bound_is_given_rather_than_faked() {
        // The plan already recorded that the allowlist is unenforced, and
        // `ingot run --sandbox` refuses on that. Asking for `--network none`
        // here would break a tool that legitimately needs the network while
        // still not enforcing the list.
        for network in [
            Network::Unrestricted,
            Network::Hosts {
                hosts: vec!["arxiv.org".into()],
            },
        ] {
            let args = args_of(network);
            assert_eq!(value_after(&args, "--network"), None, "{args:?}");
        }
    }

    #[test]
    fn an_environment_variable_crosses_by_name_and_never_by_value() {
        let args = args_of(Network::None);
        assert_eq!(
            values_after(&args, "--env"),
            vec!["GITHUB_TOKEN".to_string()]
        );
        assert!(
            !args.iter().any(|arg| arg.contains('=')),
            "a value would appear in a process listing: {args:?}"
        );
    }

    #[test]
    fn the_container_is_hardened_by_default() {
        let args = args_of(Network::None);
        assert_eq!(
            value_after(&args, "--security-opt"),
            Some("no-new-privileges".to_string())
        );
        assert_eq!(value_after(&args, "--cap-drop"), Some("ALL".to_string()));
        assert!(args.contains(&"--read-only".to_string()));
        assert_eq!(value_after(&args, "--tmpfs"), Some("/tmp".to_string()));
        assert!(args.contains(&"--rm".to_string()));
        assert!(args.contains(&"-i".to_string()), "stdio is the protocol");
    }

    #[test]
    fn the_image_and_its_command_come_last_and_in_order() {
        let args = args_of(Network::None);
        let image = args
            .iter()
            .position(|arg| arg == "ingot/mcp-fs:0.2")
            .expect("the image must be there");
        assert_eq!(
            &args[image..],
            &[
                "ingot/mcp-fs:0.2".to_string(),
                "ingot-mcp-fs".to_string(),
                "--root".to_string(),
                ".".to_string(),
            ]
        );
    }

    #[test]
    fn the_working_directory_is_the_workspace() {
        assert_eq!(
            value_after(&args_of(Network::None), "--workdir"),
            Some("/workspace".to_string())
        );
    }

    #[test]
    fn a_plan_with_nothing_mounted_asks_for_no_volumes() {
        let mut bare = plan(Network::None);
        bare.mounts.clear();
        bare.env.clear();
        let args = invocation(&bare, "img", &["srv".to_string()], Path::new("/srv"));
        assert!(values_after(&args, "--volume").is_empty(), "{args:?}");
        assert!(values_after(&args, "--env").is_empty(), "{args:?}");
    }

    #[test]
    fn an_unenforceable_note_does_not_change_what_is_asked_for() {
        // The refusal belongs to the caller. By the time an invocation is
        // built, the operator has either fixed the policy or accepted it.
        let mut noted = plan(Network::Unrestricted);
        noted.unenforceable = vec![Unenforceable {
            policy: "network allow []".into(),
            reason: "unbounded".into(),
        }];
        let with = invocation(&noted, "img", &[], Path::new("/srv"));
        noted.unenforceable.clear();
        let without = invocation(&noted, "img", &[], Path::new("/srv"));
        assert_eq!(with, without);
    }

    #[test]
    fn an_extended_length_windows_path_is_made_acceptable() {
        // `docker: invalid spec: … too many colons` is what happens without
        // this, and it happens for every mount, on every Windows machine,
        // because `Path::canonicalize` produces the prefix.
        assert_eq!(
            mount_source(Path::new(r"\\?\C:\Users\me\repo")),
            r"C:\Users\me\repo"
        );
        assert_eq!(
            mount_source(Path::new(r"\\?\UNC\server\share\repo")),
            r"\\server\share\repo"
        );
        assert_eq!(mount_source(Path::new("/srv/checkout")), "/srv/checkout");
    }

    #[test]
    fn detection_says_which_problem_it_is() {
        // Whatever this machine has, the error must distinguish "not installed"
        // from "installed but not answering" — they need different advice, and
        // "serving the wrong kind of container" is a third case that looks like
        // success until a flag is rejected.
        match detect() {
            Ok(runtime) => assert!(RUNTIMES.contains(&runtime.program.as_str())),
            Err(ExecutorError::NoRuntime) => {}
            Err(error @ ExecutorError::RuntimeUnavailable { .. }) => {
                let text = error.to_string();
                assert!(text.contains("not usable"), "{text}");
                assert!(text.contains("--sandbox"), "{text}");
            }
            Err(other) => panic!("detection cannot produce {other}"),
        }
    }

    #[test]
    fn a_daemon_serving_the_wrong_kind_of_container_is_refused_not_partly_used() {
        // A Windows-container daemon accepts `--volume` and rejects
        // `--read-only`, so proceeding would give a boundary with the hardening
        // silently missing. GitHub's windows-latest runner is exactly this.
        let error = ExecutorError::RuntimeUnavailable {
            runtime: "docker".into(),
            reason: format!(
                "it is serving windows containers, and the boundary needs {REQUIRED_OS} ones"
            ),
        };
        let text = error.to_string();
        assert!(text.contains("windows containers"), "{text}");
        assert!(text.contains("without --sandbox"), "{text}");
    }
}
