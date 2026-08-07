//! What the boundary actually does, against a real container runtime.
//!
//! The unit tests assert the argument vector — what we *ask* for. These assert
//! what is *granted*: that a read mount refuses a write, that a write mount
//! accepts one, that `network deny` leaves nothing to connect to, and that a
//! path the policy did not name is not there at all.
//!
//! They use `alpine` rather than an Ingot image, because the claim under test
//! is about the boundary and not about any particular server. On a machine with
//! no runtime they report that and return; set `INGOT_REQUIRE_CONTAINER=1` —
//! as CI does — to make an absent runtime a failure instead.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use ingot_sandbox::{invocation, Mount, Network, SandboxPlan};

/// The image is pulled if absent, so it must be small and universally present.
const IMAGE: &str = "alpine:3";

fn runtime() -> Option<String> {
    match ingot_sandbox::detect() {
        Ok(runtime) => Some(runtime.program),
        Err(error) => {
            if std::env::var_os("INGOT_REQUIRE_CONTAINER").is_some() {
                panic!("INGOT_REQUIRE_CONTAINER is set but no runtime is usable: {error}");
            }
            eprintln!("skipping: {error}");
            None
        }
    }
}

fn workspace(label: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("ingot-container-{label}"));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("readable")).expect("creating the workspace");
    std::fs::create_dir_all(root.join("writable")).expect("creating the workspace");
    std::fs::write(root.join("readable").join("note.txt"), "hello\n").unwrap();
    std::fs::write(root.join("secret.txt"), "not mounted\n").unwrap();
    root.canonicalize().expect("canonicalising it")
}

fn plan(root: &Path, network: Network) -> SandboxPlan {
    SandboxPlan {
        agent: "test.Agent".into(),
        server: "files".into(),
        mounts: vec![
            Mount {
                path: "readable".into(),
                host: root.join("readable"),
                guest: "/workspace/readable".into(),
                writable: false,
                from: "filesystem_read allow [\"readable\"]".into(),
            },
            Mount {
                path: "writable".into(),
                host: root.join("writable"),
                guest: "/workspace/writable".into(),
                writable: true,
                from: "filesystem_write allow [\"writable\"]".into(),
            },
        ],
        network,
        env: Vec::new(),
        workdir: "/workspace".into(),
        unenforceable: Vec::new(),
    }
}

/// Run `script` inside the boundary and return what happened.
fn inside(program: &str, root: &Path, network: Network, script: &str) -> Output {
    let command = vec!["sh".to_string(), "-c".to_string(), script.to_string()];
    let args = invocation(&plan(root, network), IMAGE, &command, root);
    Command::new(program)
        .args(&args)
        .output()
        .expect("the runtime must be runnable")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

#[test]
fn a_read_mount_can_be_read_and_not_written() {
    let Some(program) = runtime() else { return };
    let root = workspace("read-only");

    let read = inside(
        &program,
        &root,
        Network::None,
        "cat /workspace/readable/note.txt",
    );
    assert!(
        read.status.success(),
        "{}",
        String::from_utf8_lossy(&read.stderr)
    );
    assert_eq!(stdout(&read), "hello");

    let write = inside(
        &program,
        &root,
        Network::None,
        "touch /workspace/readable/new.txt 2>/dev/null && echo WROTE || echo REFUSED",
    );
    assert_eq!(stdout(&write), "REFUSED");
    assert!(
        !root.join("readable").join("new.txt").exists(),
        "a read grant must not become a write"
    );
}

#[test]
fn a_write_mount_reaches_the_host() {
    let Some(program) = runtime() else { return };
    let root = workspace("writable");

    let output = inside(
        &program,
        &root,
        Network::None,
        "echo produced > /workspace/writable/out.txt && echo OK",
    );
    assert_eq!(
        stdout(&output),
        "OK",
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(root.join("writable").join("out.txt")).unwrap(),
        "produced\n"
    );
}

#[test]
fn a_path_the_policy_did_not_name_does_not_exist_inside() {
    let Some(program) = runtime() else { return };
    let root = workspace("unmounted");

    let output = inside(
        &program,
        &root,
        Network::None,
        "test -e /workspace/secret.txt && echo PRESENT || echo ABSENT",
    );
    assert_eq!(
        stdout(&output),
        "ABSENT",
        "the workspace is not mounted wholesale; only what the policy named is"
    );
}

#[test]
fn network_deny_leaves_nothing_to_connect_to() {
    let Some(program) = runtime() else { return };
    let root = workspace("no-network");

    // No interface but loopback, so this is a property of the box rather than
    // of whether some remote host happens to be reachable from CI.
    let output = inside(
        &program,
        &root,
        Network::None,
        "ip -o link show | grep -cv ' lo:' || true",
    );
    assert_eq!(
        stdout(&output),
        "0",
        "network deny must mean no interface, not merely no route"
    );
}

#[test]
fn a_network_the_plan_could_not_bound_is_actually_there() {
    // The counterpart: when the plan says it could not enforce the allowlist,
    // the container really does get a network. Anything else would be a
    // different lie.
    let Some(program) = runtime() else { return };
    let root = workspace("network");

    let output = inside(
        &program,
        &root,
        Network::Hosts {
            hosts: vec!["example.org".into()],
        },
        "ip -o link show | grep -cv ' lo:' || true",
    );
    assert_ne!(stdout(&output), "0");
}

#[test]
fn the_root_filesystem_is_not_writable() {
    let Some(program) = runtime() else { return };
    let root = workspace("read-only-root");

    let output = inside(
        &program,
        &root,
        Network::None,
        "touch /etc/passwd-new 2>/dev/null && echo WROTE || echo REFUSED",
    );
    assert_eq!(stdout(&output), "REFUSED");
}

#[test]
fn the_working_directory_is_the_workspace() {
    let Some(program) = runtime() else { return };
    let root = workspace("workdir");
    let output = inside(&program, &root, Network::None, "pwd");
    assert_eq!(stdout(&output), "/workspace");
}
