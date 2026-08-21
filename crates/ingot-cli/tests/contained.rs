//! `ingot run --contained` and its supervisor channel, end to end.
//!
//! The channel is exercised through `--supervised`, which runs the guest as an
//! ordinary child process. That is deliberate: the interesting half — config,
//! model proxying, approval routing, events, outputs — is identical either way,
//! and testing it without a container runtime means it is tested on every
//! platform rather than only where Docker happens to work.
//!
//! One test does use a real container. It skips where a runtime or the image is
//! absent, and `INGOT_REQUIRE_CONTAINER=1` turns that skip into a failure.

mod support;

use std::path::Path;
use std::sync::atomic::Ordering;

use support::*;

/// A shipped example, by absolute path: a test binary's working directory is its
/// own crate, not the repository.
fn example(name: &str) -> String {
    repo_root()
        .join("examples")
        .join(name)
        .display()
        .to_string()
}

fn summarizer_cassette() -> String {
    repo_root()
        .join("examples/document-summarizer/tests/cassettes/brief.json")
        .display()
        .to_string()
}

/// The document the shipped cassette was recorded against, byte for byte.
fn recorded_document() -> String {
    let text = std::fs::read_to_string(summarizer_cassette())
        .expect("the shipped cassette must be readable");
    let cassette: serde_json::Value = serde_json::from_str(&text).expect("it must parse");
    cassette["inputs"]["document"]
        .as_str()
        .expect("the cassette records the document")
        .to_string()
}

/// A file holding the recorded document, so it crosses the command line intact.
fn document_file(dir: &TempDir) -> String {
    let path = dir.path().join("doc.txt");
    std::fs::write(&path, recorded_document()).expect("writing the document");
    format!("document=@{}", path.display())
}

/// A project with one agent, written fresh so a test can choose its policy.
fn project(dir: &Path, source: &str, manifest_extra: &str) {
    std::fs::write(dir.join("main.ing"), source).expect("writing the source");
    std::fs::write(
        dir.join("ingot.toml"),
        format!(
            "[project]\nname = \"probe\"\nversion = \"0.1.0\"\n\n\
             [build]\nentry = \"main.ing\"\nout-dir = \"target/ingot\"\n{manifest_extra}"
        ),
    )
    .expect("writing the manifest");
}

// --- the channel ------------------------------------------------------------

#[test]
fn a_supervised_run_produces_the_same_artifacts_as_a_local_one() {
    let out = TempDir::new("supervised-out");
    let document = TempDir::new("supervised-doc");

    let output = run_env(
        &[
            "run",
            &example("document-summarizer"),
            "--supervised",
            "--provider",
            "replay",
            "--cassette",
            &summarizer_cassette(),
            "--input",
            "audience=engineering leads",
            "--input",
            &document_file(&document),
            "--out-dir",
            &out.path().display().to_string(),
        ],
        &[],
    );

    assert_eq!(code(&output), EXIT_OK, "{}", stderr(&output));

    // The agent ran in another process and its output landed here, written by
    // this one from what came back down the channel.
    let summary = std::fs::read_to_string(out.path().join("summary.md"))
        .expect("the host writes the artifacts");
    assert!(summary.contains("Compiler Design"), "{summary}");

    // Every event crossed, in order, and the `runStarted` line names the
    // provider that actually answered rather than naming the channel.
    let log = stderr(&output);
    assert!(log.contains("(provider: replay)"), "{log}");
    assert!(log.contains("n0  llm.call"), "{log}");
    assert!(log.contains("emit summary"), "{log}");
    assert!(log.contains("done: 1 step(s)"), "{log}");
}

#[test]
fn a_supervised_run_says_plainly_that_it_enforces_nothing() {
    // A flag that looks like containment and is not is worse than no flag.
    let document = TempDir::new("supervised-warn");
    let output = run_env(
        &[
            "run",
            &example("document-summarizer"),
            "--supervised",
            "--provider",
            "replay",
            "--cassette",
            &summarizer_cassette(),
            "--input",
            "audience=engineering leads",
            "--input",
            &document_file(&document),
        ],
        &[],
    );
    let log = stderr(&output);
    assert!(log.contains("nothing is enforced"), "{log}");
    assert!(log.contains("not a boundary"), "{log}");
}

#[test]
fn a_failure_inside_comes_back_with_its_own_message_and_a_diagnostic_exit() {
    // A missing input is the operator's to fix, and that verdict is reached
    // inside and carried out — the hint must not be lost at the boundary.
    let output = run_env(
        &[
            "run",
            &example("document-summarizer"),
            "--supervised",
            "--provider",
            "replay",
            "--cassette",
            &summarizer_cassette(),
            "--input",
            "audience=engineering leads",
        ],
        &[],
    );
    assert_eq!(code(&output), EXIT_DIAGNOSTICS, "{}", stderr(&output));
    let log = stderr(&output);
    assert!(log.contains("missing input `document`"), "{log}");
    assert!(log.contains("not with the agent itself"), "{log}");
}

#[test]
fn the_completion_is_fetched_by_the_host_and_not_from_inside() {
    let dir = TempDir::new("supervised-provider");
    project(
        dir.path(),
        "language 0.1\n\
         agent Note(topic: string) -> note<markdown> {\n\
         \x20 model requires { structured_output }\n\
         \x20 budget { steps <= 2 tokens <= 1000 }\n\
         \x20 policy { network deny }\n\
         \x20 flow {\n\
         \x20   emit note = ask<markdown>(\"Write one line about ${topic}.\")\n\
         \x20 }\n\
         }\n",
        "",
    );

    let stub = stub_provider(vec![text_reply("# Note\n\nA line.")]);
    let output = run_env(
        &[
            "run",
            &dir.path().display().to_string(),
            "--supervised",
            "--provider",
            "anthropic",
            "--input",
            "topic=compilers",
        ],
        &[
            ("ANTHROPIC_API_KEY", "stub-key"),
            ("INGOT_ANTHROPIC_BASE_URL", &stub.url),
        ],
    );

    assert_eq!(code(&output), EXIT_OK, "{}", stderr(&output));
    // The stub was reached exactly once, from out here. The guest was given
    // neither the URL nor the key, and could not have made this call.
    assert_eq!(stub.served.load(Ordering::SeqCst), 1);
    assert!(stdout(&output).contains("A line."), "{}", stdout(&output));
}

#[test]
fn tools_run_inside_and_their_results_reach_the_flow() {
    // `--supervised` starts the guest as a child, and the guest starts the tool
    // server as *its* child. That is the arrangement a contained run has, minus
    // the boundary, so it is where the wiring is worth checking.
    let dir = TempDir::new("supervised-tools");
    std::fs::create_dir_all(dir.path().join("data")).unwrap();
    std::fs::write(
        dir.path().join("data").join("note.txt"),
        "hello from disk\n",
    )
    .unwrap();

    project(
        dir.path(),
        "language 0.1\n\
         tool fs.read_file(path: string) -> text !filesystem_read\n\
         agent Reader() -> echo<markdown> {\n\
         \x20 model requires { structured_output }\n\
         \x20 tools { mcp fs.read_file }\n\
         \x20 budget { steps <= 4 tokens <= 1000 }\n\
         \x20 policy { network deny\n    filesystem_read allow [\"data\"] }\n\
         \x20 flow {\n\
         \x20   note = call fs.read_file(\"note.txt\")\n\
         \x20   emit echo = ask<markdown>(\"Repeat this exactly: ${note}\")\n\
         \x20 }\n\
         }\n",
        &format!(
            "\n[[mcp.server]]\nname = \"files\"\ncommand = {}\nargs = [\"--root\", \"data\"]\n",
            toml_string(&fs_server().display().to_string())
        ),
    );

    let stub = stub_provider(vec![text_reply("hello from disk")]);
    let output = run_env(
        &[
            "run",
            &dir.path().display().to_string(),
            "--supervised",
            "--provider",
            "anthropic",
        ],
        &[
            ("ANTHROPIC_API_KEY", "stub-key"),
            ("INGOT_ANTHROPIC_BASE_URL", &stub.url),
        ],
    );

    assert_eq!(code(&output), EXIT_OK, "{}", stderr(&output));
    let log = stderr(&output);
    assert!(log.contains("tool fs.read_file"), "{log}");
    assert!(
        log.contains("[contained]"),
        "the guest's own diagnostics must be relayed:\n{log}"
    );
}

// --- what a run costs -------------------------------------------------------

/// A project with one model call and a `cost` ceiling, priced or not.
///
/// The stub answers with 120 input and 40 output tokens, which at $3 and $15 the
/// million is 0.00096 USD — so `0.0005 usd` is a ceiling one call passes and
/// `1 usd` is one it does not reach.
fn priced_project(dir: &Path, ceiling: &str, prices: bool) {
    let source = format!(
        r#"language 0.1
agent Priced(topic: string) -> note<markdown> {{
  model requires {{ structured_output }}
  budget {{ steps <= 2 tokens <= 1000 cost <= {ceiling} }}
  policy {{ network deny }}
  flow {{
    emit note = ask<markdown>("One line about ${{topic}}.")
  }}
}}
"#
    );
    let manifest = if prices {
        r#"
[[model.price]]
model = "claude-opus-5"
input = "3"
output = "15"
currency = "usd"
"#
    } else {
        ""
    };
    project(dir, &source, manifest);
}

/// Run `priced_project` once, with or without the supervisor channel.
fn priced_run(path: &str, supervised: bool) -> std::process::Output {
    let stub = stub_provider(vec![text_reply("# Note")]);
    let mut args = vec![
        "run",
        path,
        "--provider",
        "anthropic",
        "--input",
        "topic=compilers",
    ];
    if supervised {
        args.push("--supervised");
    }
    run_env(
        &args,
        &[
            ("ANTHROPIC_API_KEY", "stub-key"),
            ("INGOT_ANTHROPIC_BASE_URL", &stub.url),
        ],
    )
}

/// The `error:` line, which is the one sentence an operator reads.
fn refusal(output: &std::process::Output) -> String {
    stderr(output)
        .lines()
        .find(|line| line.starts_with("error:"))
        .unwrap_or("<no refusal>")
        .to_string()
}

#[test]
fn a_cost_ceiling_stops_a_supervised_run_where_it_stops_a_host_one() {
    // GAP-048 was this comparison coming out different: the same artifact, the
    // same manifest and the same prices refused out here and ran on in there,
    // because the prices did not cross. What is asserted is the agreement —
    // either half passing alone would say nothing.
    let dir = TempDir::new("cost-ceiling");
    priced_project(dir.path(), "0.0005 usd", true);
    let path = dir.path().display().to_string();

    let host = priced_run(&path, false);
    let supervised = priced_run(&path, true);

    assert_eq!(code(&host), EXIT_DIAGNOSTICS, "{}", stderr(&host));
    assert_eq!(
        code(&supervised),
        EXIT_DIAGNOSTICS,
        "{}",
        stderr(&supervised)
    );
    assert!(
        refusal(&host).contains("`cost` budget of 0.0005 USD"),
        "{}",
        stderr(&host)
    );
    assert_eq!(
        refusal(&host),
        refusal(&supervised),
        "the boundary must not change what a ceiling means"
    );
}

#[test]
fn a_supervised_run_reports_what_it_spent() {
    // The ledger is kept where the charging happens, which is inside. This only
    // holds if it came back out.
    let dir = TempDir::new("cost-reported");
    priced_project(dir.path(), "1 usd", true);
    let path = dir.path().display().to_string();

    let output = priced_run(&path, true);
    assert_eq!(code(&output), EXIT_OK, "{}", stderr(&output));
    assert!(
        stderr(&output).contains("cost      0.00096 USD"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn a_supervised_run_says_when_a_ceiling_was_not_charged() {
    // The half of GAP-048 that was not arithmetic. A `cost` budget nothing
    // charged has to be named as such or it reads as enforced, and inside a box
    // it was not mentioned at all. The model is named too, so configuring it is
    // a copy and a paste.
    let dir = TempDir::new("cost-unpriced");
    priced_project(dir.path(), "1 usd", false);
    let path = dir.path().display().to_string();

    let output = priced_run(&path, true);
    assert_eq!(code(&output), EXIT_OK, "{}", stderr(&output));
    let log = stderr(&output);
    assert!(log.contains("not charged for `claude-opus-5`"), "{log}");
    assert!(log.contains("the budget was not enforced"), "{log}");
}

// --- refusals ---------------------------------------------------------------

#[test]
fn a_program_whose_agents_want_different_boundaries_is_refused() {
    // The two-agent example is exactly this case: the coordinator may write and
    // the reviewer may not. One box for both would hand the reviewer a grant its
    // own policy denies, and nothing downstream could detect it.
    let output = run_env(
        &[
            "run",
            &example("code-review-team"),
            "--contained",
            "--image",
            "ingot/run:test",
        ],
        &[],
    );
    assert_ne!(code(&output), EXIT_OK);
    let log = stderr(&output);
    assert!(log.contains("do not share one boundary"), "{log}");
    assert!(log.contains("widen a policy"), "{log}");
    assert!(log.contains("CodeReviewTeam"), "{log}");
    assert!(log.contains("--sandbox"), "{log}");
}

#[test]
fn a_missing_boundary_never_falls_back_to_a_host_run() {
    let document = TempDir::new("contained-no-image");
    let out = TempDir::new("contained-no-boundary-out");
    let output = std::process::Command::new(binary())
        .args([
            "run",
            &example("document-summarizer"),
            "--contained",
            "--provider",
            "replay",
            "--cassette",
            &summarizer_cassette(),
            "--input",
            "audience=engineering leads",
            "--input",
            &document_file(&document),
            "--out-dir",
            &out.path().display().to_string(),
            "--color",
            "never",
        ])
        // Resolve the already-open binary first, then make runtime detection
        // deterministic: neither Docker nor Podman can be found.
        .env("PATH", "")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .output()
        .expect("the ingot binary must be runnable");

    assert_ne!(code(&output), EXIT_OK);
    let log = stderr(&output);
    assert!(
        log.contains("no container runtime found") || log.contains("installed but not usable"),
        "the command must refuse because no usable boundary exists:\n{log}"
    );
    assert!(!log.contains("nothing is enforced"), "{log}");
    assert!(
        !out.path().join("summary.md").exists(),
        "a missing boundary must stop before the agent can produce an artifact"
    );
}

#[test]
fn recording_a_supervised_run_is_refused_rather_than_half_done() {
    let cassette = TempDir::new("contained-record");
    let document = TempDir::new("contained-record-doc");
    let output = run_env(
        &[
            "run",
            &example("document-summarizer"),
            "--supervised",
            "--record",
            &cassette.path().join("out.json").display().to_string(),
            "--input",
            "audience=engineering leads",
            "--input",
            &document_file(&document),
        ],
        &[],
    );
    assert_ne!(code(&output), EXIT_OK);
    let log = stderr(&output);
    assert!(log.contains("cannot be combined"), "{log}");
    assert!(log.contains("omit the tool results"), "{log}");
}

#[test]
fn allowing_unenforced_rules_without_a_boundary_is_refused() {
    let output = run_env(
        &[
            "run",
            &example("document-summarizer"),
            "--sandbox-allow-unenforced",
        ],
        &[],
    );
    assert_ne!(code(&output), EXIT_OK);
    assert!(
        stderr(&output).contains("nothing to leave unenforced"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn an_image_without_containment_is_refused() {
    let output = run_env(
        &["run", &example("document-summarizer"), "--image", "x:1"],
        &[],
    );
    assert_ne!(code(&output), EXIT_OK);
    assert!(
        stderr(&output).contains("only applies to --contained"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn sandbox_and_contained_are_not_combinable() {
    // They contain different things. Accepting both would leave it unclear which
    // boundary is in force, which is the one thing this feature must never be.
    let output = run_env(
        &["run", &example("repo-digest"), "--sandbox", "--contained"],
        &[],
    );
    assert_ne!(code(&output), EXIT_OK);
    assert!(
        stderr(&output)
            .to_lowercase()
            .contains("cannot be used with"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn exec_without_a_supervisor_refuses_instead_of_waiting() {
    // `ingot exec` reads its whole configuration from a channel. Run by hand it
    // must say so rather than blocking on an empty stdin forever.
    let output = std::process::Command::new(binary())
        .arg("exec")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("the binary must be runnable");
    assert_ne!(output.status.code(), Some(EXIT_OK));
    let log = String::from_utf8_lossy(&output.stderr);
    assert!(log.contains("not a way to run an agent"), "{log}");
}

#[test]
fn exec_is_not_offered_in_the_command_list() {
    let output = run_env(&["--help"], &[]);
    let help = stdout(&output);
    assert!(help.contains("run "), "{help}");
    assert!(
        !help
            .lines()
            .any(|line| line.trim_start().starts_with("exec")),
        "`exec` is not for operators to invoke:\n{help}"
    );
}

// --- with a real boundary ---------------------------------------------------

/// The image the contained test needs.
///
/// Tagged with the crate version, which is what `ingot image build` prepares.
fn image() -> String {
    format!("ingot/run:{}", env!("CARGO_PKG_VERSION"))
}

fn image_available() -> Option<String> {
    let runtime = match ingot_sandbox::detect() {
        Ok(runtime) => runtime.program,
        Err(error) => {
            if std::env::var_os("INGOT_REQUIRE_CONTAINER").is_some() {
                panic!("INGOT_REQUIRE_CONTAINER is set but no runtime is usable: {error}");
            }
            eprintln!("skipping: {error}");
            return None;
        }
    };

    let image = image();
    let present = std::process::Command::new(&runtime)
        .args(["image", "inspect", &image])
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false);

    if !present {
        let hint =
            format!("the image {image} is not built; run `ingot image build` from the repository");
        if std::env::var_os("INGOT_REQUIRE_CONTAINER").is_some() {
            panic!("INGOT_REQUIRE_CONTAINER is set but {hint}");
        }
        eprintln!("skipping: {hint}");
        return None;
    }
    Some(runtime)
}

#[test]
fn a_reference_contained_run_needs_no_repository_specific_build_command() {
    // The claim: an agent whose policy grants nothing at all — no mount, no
    // network — completes a model call. Nothing inside could have reached a
    // provider, so the answer came through the supervisor.
    let Some(_runtime) = image_available() else {
        return;
    };

    let out = TempDir::new("contained-out");
    let document = TempDir::new("contained-doc");

    let output = run_env(
        &[
            "run",
            &example("document-summarizer"),
            "--contained",
            "--provider",
            "replay",
            "--cassette",
            &summarizer_cassette(),
            "--input",
            "audience=engineering leads",
            "--input",
            &document_file(&document),
            "--out-dir",
            &out.path().display().to_string(),
        ],
        &[],
    );

    assert_eq!(code(&output), EXIT_OK, "{}", stderr(&output));

    let log = stderr(&output);
    assert!(log.contains("the run itself"), "{log}");
    assert!(log.contains("network  none"), "{log}");
    assert!(
        log.contains("the workspace is not visible"),
        "this agent's policy grants no path, so the box has no mounts:\n{log}"
    );

    let summary = std::fs::read_to_string(out.path().join("summary.md"))
        .expect("the host writes the artifacts, from outside the boundary");
    assert!(summary.contains("Compiler Design"), "{summary}");
}

#[test]
fn a_contained_agent_reads_and_writes_only_through_its_policys_mounts() {
    // The whole feature in one test. The agent runs in a box with `--network
    // none`; its tool server runs inside that box; the mounts come from its own
    // `policy` block; the model answer comes from a stub on the host that nothing
    // inside could have reached. What lands on the host afterwards arrived through
    // the one write mount the policy named.
    let Some(_runtime) = image_available() else {
        return;
    };

    let dir = TempDir::new("contained-tools");
    std::fs::create_dir_all(dir.path().join("data")).unwrap();
    std::fs::write(
        dir.path().join("data").join("note.txt"),
        "boxed and filed\n",
    )
    .unwrap();
    // Named by no policy rule, so it must not exist inside at all.
    std::fs::write(dir.path().join("secret.txt"), "not mounted\n").unwrap();

    project(
        dir.path(),
        "language 0.1\n\
         tool fs.read_file(path: string) -> text !filesystem_read\n\
         tool fs.write_file(path: string, content: text) -> file !filesystem_write\n\
         agent Boxed() -> digest<markdown> {\n\
         \x20 model requires { structured_output }\n\
         \x20 tools { mcp fs.read_file\n    mcp fs.write_file }\n\
         \x20 budget { steps <= 6 tokens <= 2000 }\n\
         \x20 policy { network deny\n    \
                       filesystem_read allow [\"data\"]\n    \
                       filesystem_write allow [\"out\"]\n    \
                       secrets deny export }\n\
         \x20 flow {\n\
         \x20   note = call fs.read_file(\"data/note.txt\")\n\
         \x20   summary = ask<markdown>(\"Repeat this exactly: ${note}\")\n\
         \x20   _filed = call fs.write_file(\"out/digest.md\", summary)\n\
         \x20   emit digest = summary\n\
         \x20 }\n\
         }\n",
        // `ingot-mcp-fs`, not a host path: inside the boundary the server comes
        // from the image, which is what `tools/ingot.Dockerfile` puts there.
        "\n[[mcp.server]]\nname = \"files\"\ncommand = \"ingot-mcp-fs\"\n\
         args = [\"--root\", \".\", \"--allow-write\"]\n",
    );

    let stub = stub_provider(vec![text_reply("# Digest\n\nboxed and filed")]);
    let output = run_env(
        &[
            "run",
            &dir.path().display().to_string(),
            "--contained",
            "--provider",
            "anthropic",
        ],
        &[
            ("ANTHROPIC_API_KEY", "stub-key"),
            ("INGOT_ANTHROPIC_BASE_URL", &stub.url),
        ],
    );

    assert_eq!(code(&output), EXIT_OK, "{}", stderr(&output));

    let log = stderr(&output);
    assert!(log.contains("/workspace/data"), "{log}");
    assert!(log.contains("/workspace/out"), "{log}");
    assert!(log.contains("network  none"), "{log}");
    assert!(
        !log.contains("secret.txt"),
        "an unnamed path is not part of the boundary:\n{log}"
    );

    // The model call was served here, from a socket the box has no route to.
    assert_eq!(stub.served.load(Ordering::SeqCst), 1);

    // And the tool server, inside the box, wrote through the write mount onto
    // this machine.
    let filed = std::fs::read_to_string(dir.path().join("out").join("digest.md"))
        .expect("the write mount reaches the host");
    assert!(filed.contains("boxed and filed"), "{filed}");
}
