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
    let args = invocation(&plan(root, network), IMAGE, &command, root, None);
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

// --- the egress boundary --------------------------------------------------

/// The image the proxy runs from, overridable so CI can tag it as it likes.
fn egress_image() -> String {
    std::env::var("INGOT_EGRESS_IMAGE")
        .unwrap_or_else(|_| ingot_sandbox::DEFAULT_EGRESS_IMAGE.to_string())
}

/// Whether the proxy image is here. Absent, these skip the way the rest do.
fn egress_available(runtime: &ingot_sandbox::Runtime) -> bool {
    match ingot_sandbox::image_exists(runtime, &egress_image()) {
        Ok(true) => true,
        _ => {
            if std::env::var_os("INGOT_REQUIRE_CONTAINER").is_some() {
                panic!(
                    "INGOT_REQUIRE_CONTAINER is set but `{}` is not built; \
                     docker build -f tools/egress.Dockerfile -t {} .",
                    egress_image(),
                    egress_image()
                );
            }
            eprintln!("skipping: `{}` is not built", egress_image());
            false
        }
    }
}

/// Run `script` inside a boundary routed through a proxy allowing `hosts`.
fn through_egress(hosts: &[&str], script: &str) -> Option<Output> {
    let runtime = ingot_sandbox::detect().ok()?;
    if runtime.program.is_empty() || !egress_available(&runtime) {
        return None;
    }

    let hosts: Vec<String> = hosts.iter().map(|host| (*host).to_string()).collect();
    let name = format!("test-{}", std::process::id());
    let boundary =
        ingot_sandbox::EgressBoundary::start(&runtime, &name, &hosts, &egress_image()).ok()?;

    let root = workspace("egress");
    let command = vec!["sh".to_string(), "-c".to_string(), script.to_string()];
    let args = invocation(
        &plan(&root, Network::Hosts { hosts }),
        IMAGE,
        &command,
        &root,
        Some(boundary.route()),
    );
    Command::new(&runtime.program).args(&args).output().ok()
}

#[test]
fn a_granted_host_is_reachable_from_inside_the_boundary() {
    // busybox `wget` reads the lower-case variables, so both spellings are
    // sent. A boundary that only set one would look like it worked with curl
    // and fail with the tool somebody actually shipped.
    let Some(output) = through_egress(
        &["example.com"],
        "wget -T 10 -q -O- http://example.com/ | head -c 40",
    ) else {
        return;
    };
    let body = String::from_utf8_lossy(&output.stdout);
    assert!(
        body.contains("Example Domain") || body.contains("<!doctype"),
        "a granted host should be reachable: {body} {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn a_host_the_policy_does_not_name_is_refused_from_inside_the_boundary() {
    // The claim GAP-001 existed for. Not "the plan says so" — the request is
    // made, from inside the box, and it does not arrive.
    let Some(output) = through_egress(
        &["example.com"],
        "wget -T 10 -q -O- http://neverssl.com/ && echo REACHED || echo BLOCKED",
    ) else {
        return;
    };
    let body = String::from_utf8_lossy(&output.stdout);
    assert!(
        body.contains("BLOCKED"),
        "an ungranted host must not be reachable: {body} {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn ignoring_the_proxy_reaches_nothing_at_all() {
    // The property that separates this from setting a variable and hoping. The
    // network is the bound; the variables only say where the door is. A server
    // that never heard of them gets nowhere.
    let Some(output) = through_egress(
        &["example.com"],
        "unset http_proxy https_proxy HTTP_PROXY HTTPS_PROXY; \
         wget -T 6 -q -O- http://example.com/ && echo REACHED || echo BLOCKED",
    ) else {
        return;
    };
    let body = String::from_utf8_lossy(&output.stdout);
    assert!(
        body.contains("BLOCKED"),
        "the boundary must not depend on the contained process cooperating: {body}"
    );
}
