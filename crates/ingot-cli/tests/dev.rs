//! End-to-end tests for the long-lived `ingot dev` loop.

mod support;

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use support::{binary, stub_provider, text_reply, TempDir};

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn wait_for(receiver: &Receiver<String>, expected: &str) -> Vec<String> {
    let mut seen = Vec::new();
    loop {
        let line = receiver
            .recv_timeout(Duration::from_secs(10))
            .unwrap_or_else(|error| {
                panic!(
                    "timed out waiting for `{expected}` ({error}); saw:\n{}",
                    seen.join("\n")
                )
            });
        let found = line.contains(expected);
        seen.push(line);
        if found {
            return seen;
        }
    }
}

#[test]
fn dev_never_runs_a_source_revision_that_failed_to_compile() {
    let project = TempDir::new("dev-no-failed-run");
    let source = project.path().join("main.ing");
    let valid = r#"language 0.1

agent Research(topic: string) -> brief<markdown> {
  model exact "anthropic/claude-test"

  budget {
    steps <= 2
    tokens <= 20000
  }

  policy {
    network deny
  }

  flow {
    emit brief = ask<markdown>("Write about ${topic}.")
  }
}
"#;
    std::fs::write(&source, valid).expect("writing valid revision");
    std::fs::write(
        project.path().join("ingot.toml"),
        "[project]\nname = \"dev-test\"\n",
    )
    .expect("writing manifest");

    // The second reply serves a valid edit. If the later failed revision ever
    // reaches the provider, the counter will exceed the two accepted runs.
    let provider = stub_provider(vec![text_reply("# First"), text_reply("# Forbidden")]);
    let mut child = Command::new(binary())
        .args([
            "dev",
            &project.path().display().to_string(),
            "--run",
            "--input",
            "topic=compiler design",
            "--color",
            "never",
        ])
        .env("ANTHROPIC_API_KEY", "stub-key")
        .env("INGOT_ANTHROPIC_BASE_URL", &provider.url)
        .env_remove("OPENAI_API_KEY")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("starting dev loop");
    let stderr = child.stderr.take().expect("piped stderr");
    let mut child = ChildGuard(child);
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if sender.send(line).is_err() {
                break;
            }
        }
    });

    wait_for(&receiver, "[dev 1] run complete");
    assert_eq!(provider.served.load(Ordering::SeqCst), 1);
    let artifact = project.path().join("target/ingot/Research.ir.json");
    let first = std::fs::read(&artifact).expect("first revision IR");

    let valid_edit = valid.replace("Write about", "Explain");
    std::fs::write(&source, &valid_edit).expect("writing second valid revision");
    wait_for(&receiver, "[dev 2] run complete");
    assert_eq!(provider.served.load(Ordering::SeqCst), 2);
    let last_good = std::fs::read(&artifact).expect("second revision IR");
    assert_ne!(last_good, first, "a valid edit did not rebuild the IR");

    std::fs::write(&source, format!("{valid_edit}\nthis is not valid ingot\n"))
        .expect("writing failed revision");
    let failed = wait_for(
        &receiver,
        "[dev 3] failed; this revision was not built or run",
    );
    let kept = wait_for(&receiver, "keeping revision 2 artifacts");

    assert_eq!(
        provider.served.load(Ordering::SeqCst),
        2,
        "a failed revision reached the provider"
    );
    assert_eq!(
        std::fs::read(&artifact).expect("preserved IR"),
        last_good,
        "a failed revision replaced the last successful artifact"
    );
    assert!(
        failed.iter().any(|line| line.contains("ING")),
        "compiler diagnostics were not shown:\n{}",
        failed.join("\n")
    );
    assert!(
        kept.iter().any(|line| line.contains("keeping revision 2")),
        "last-good status was not shown"
    );

    child.0.kill().expect("stopping dev loop");
}
