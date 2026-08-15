//! `ingot studio`, from a browser's side of the socket.
//!
//! The studio's one rule is that it computes nothing the command line cannot,
//! so most of these tests are equalities: the readiness the page shows against
//! the readiness `ingot doctor --json` prints, the diagnostics against
//! `ingot check`, the run history against what a run wrote. A surface that
//! agreed with the terminal only by coincidence would be the second source of
//! truth [RFC-0007](../../../rfcs/0007-the-ingot-product-loop.md) refused.

mod support;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};

use serde_json::Value;
use support::*;

/// A studio serving in another process, with its own bookmark file.
struct Serving {
    child: Child,
    authority: String,
    token: String,
    #[allow(dead_code)]
    config: TempDir,
}

impl Serving {
    fn start(tag: &str) -> Serving {
        Serving::start_with(tag, &[])
    }

    /// A studio with these variables set, and every provider key cleared first.
    ///
    /// The child a launch spawns inherits this environment, which is the whole
    /// reason a test can point a studio-started run at a stub.
    fn start_with(tag: &str, env: &[(&str, &str)]) -> Serving {
        let config = TempDir::new(&format!("studio-config-{tag}"));
        let mut command = Command::new(binary());
        command
            .args(["studio", "--bind", "127.0.0.1:0"])
            // The list this studio keeps must be its own: a test that wrote to
            // the developer's real project list would be a test that edited
            // their machine.
            .env("INGOT_CONFIG_DIR", config.path())
            .env_remove("ANTHROPIC_API_KEY")
            .env_remove("INGOT_ANTHROPIC_BASE_URL")
            .env_remove("OPENAI_API_KEY")
            .env_remove("GEMINI_API_KEY")
            .env_remove("GOOGLE_API_KEY");
        for (name, value) in env {
            command.env(name, value);
        }
        let mut child = command
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("the ingot binary must be runnable");

        let mut line = String::new();
        BufReader::new(child.stdout.as_mut().expect("a piped stdout"))
            .read_line(&mut line)
            .expect("the studio must print its URL");
        let url = line.trim().to_string();
        let (authority, token) = url
            .strip_prefix("http://")
            .and_then(|rest| rest.split_once("/?token="))
            .map(|(authority, token)| (authority.to_string(), token.to_string()))
            .unwrap_or_else(|| panic!("`{url}` is not a studio URL"));

        Serving {
            child,
            authority,
            token,
            config,
        }
    }

    /// One request, one reply, as a browser on this machine would make it.
    fn request(&self, method: &str, target: &str) -> (u16, String) {
        let mut stream =
            TcpStream::connect(&self.authority).expect("the studio must accept a connection");
        write!(
            stream,
            "{method} {target} HTTP/1.1\r\nHost: {}\r\nOrigin: http://{}\r\nX-Ingot-Token: {}\r\n\r\n",
            self.authority, self.authority, self.token
        )
        .expect("the request must be written");
        stream.flush().expect("the request must flush");

        let mut reply = String::new();
        stream
            .read_to_string(&mut reply)
            .expect("the reply must be readable");
        let status = reply
            .split(' ')
            .nth(1)
            .and_then(|code| code.parse().ok())
            .unwrap_or(0);
        let body = reply
            .split_once("\r\n\r\n")
            .map(|(_, body)| body.to_string())
            .unwrap_or_default();
        (status, body)
    }

    fn get(&self, target: &str) -> Value {
        let (status, body) = self.request("GET", target);
        assert_eq!(status, 200, "GET {target} answered {status}: {body}");
        serde_json::from_str(&body).unwrap_or_else(|error| panic!("{target}: {error}\n{body}"))
    }

    fn post(&self, target: &str) -> Value {
        let (status, body) = self.request("POST", target);
        assert_eq!(status, 200, "POST {target} answered {status}: {body}");
        serde_json::from_str(&body).expect("a JSON reply")
    }

    /// A request with a JSON body, as the start panel makes it.
    fn send_json(&self, method: &str, target: &str, body: &str) -> (u16, String) {
        let mut stream =
            TcpStream::connect(&self.authority).expect("the studio must accept a connection");
        write!(
            stream,
            "{method} {target} HTTP/1.1\r\nHost: {authority}\r\nOrigin: http://{authority}\r\n\
             X-Ingot-Token: {token}\r\nContent-Type: application/json\r\n\
             Content-Length: {length}\r\n\r\n{body}",
            authority = self.authority,
            token = self.token,
            length = body.len()
        )
        .expect("the request must be written");
        stream.flush().expect("the request must flush");

        let mut reply = String::new();
        stream
            .read_to_string(&mut reply)
            .expect("the reply must be readable");
        let status = reply
            .split(' ')
            .nth(1)
            .and_then(|code| code.parse().ok())
            .unwrap_or(0);
        let payload = reply
            .split_once("\r\n\r\n")
            .map(|(_, payload)| payload.to_string())
            .unwrap_or_default();
        (status, payload)
    }

    /// Re-read a project's runs until `done` is satisfied, or give up.
    ///
    /// A launched child compiles before it records anything, so there is a real
    /// gap between the request returning and there being something to see.
    fn until(&self, path: &Path, done: impl Fn(&Value) -> bool) -> Value {
        for _ in 0..200 {
            let answer = self.get(&format!("/api/runs?path={}", encoded(path)));
            if done(&answer) {
                return answer;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        panic!(
            "the studio never reached the expected state: {}",
            self.get(&format!("/api/runs?path={}", encoded(path)))
        );
    }
}

impl Drop for Serving {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Percent-encode a path so it survives a query string.
fn encoded(path: &Path) -> String {
    let mut out = String::new();
    for byte in path.display().to_string().bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// The smallest project that compiles, runs, and needs no network.
fn project(dir: &Path) {
    std::fs::write(
        dir.join("main.ing"),
        "language 0.1\n\
         agent Note(topic: string) -> note<markdown> {\n\
         \x20 model requires { structured_output }\n\
         \x20 budget { steps <= 2 }\n\
         \x20 policy { network deny }\n\
         \x20 flow {\n\
         \x20   emit note = ask<markdown>(\"One line about ${topic}.\")\n\
         \x20 }\n\
         }\n",
    )
    .expect("writing the source");
    std::fs::write(
        dir.join("ingot.toml"),
        "[project]\nname = \"note\"\nversion = \"0.1.0\"\n\n\
         [build]\nentry = \"main.ing\"\nout-dir = \"target/ingot\"\n",
    )
    .expect("writing the manifest");
}

/// Run the project once against a stubbed provider, so there is a run to show.
fn run_once(dir: &Path) -> std::process::Output {
    let stub = stub_provider(vec![text_reply("# Compilers\n\nShort.")]);
    run_env(
        &[
            "run",
            &dir.display().to_string(),
            "--provider",
            "anthropic",
            "--input",
            "topic=compilers",
        ],
        &[
            ("ANTHROPIC_API_KEY", "stub-key"),
            ("INGOT_ANTHROPIC_BASE_URL", &stub.url),
        ],
    )
}

#[test]
fn a_bookmark_is_a_path_and_removing_one_touches_nothing_on_disk() {
    let dir = TempDir::new("studio-bookmark");
    project(dir.path());
    let studio = Serving::start("bookmark");

    assert_eq!(
        studio.get("/api/projects")["projects"]
            .as_array()
            .expect("an array")
            .len(),
        0
    );

    let added = studio.post(&format!("/api/projects?path={}", encoded(dir.path())));
    let projects = added["projects"].as_array().expect("an array");
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0]["name"], "note");
    assert_eq!(projects[0]["version"], "0.1.0");

    let (status, _) = studio.request(
        "DELETE",
        &format!("/api/projects?path={}", encoded(dir.path())),
    );
    assert_eq!(status, 200);
    assert_eq!(
        studio.get("/api/projects")["projects"]
            .as_array()
            .expect("an array")
            .len(),
        0
    );
    // The project itself is untouched: a bookmark was all that was removed.
    assert!(dir.path().join("main.ing").is_file());
    assert!(dir.path().join("ingot.toml").is_file());
}

#[test]
fn a_directory_that_is_not_a_project_is_refused_with_the_reason() {
    let dir = TempDir::new("studio-not-a-project");
    let studio = Serving::start("not-a-project");

    let (status, body) = studio.request(
        "POST",
        &format!("/api/projects?path={}", encoded(dir.path())),
    );
    assert_eq!(status, 400, "{body}");
    assert!(body.contains("ingot.toml"), "{body}");
}

#[test]
fn the_readiness_the_page_shows_is_the_readiness_the_doctor_prints() {
    // The equality that matters. Two answers to "is this ready?" would be two
    // sources of truth about it, whichever one happened to be right.
    let dir = TempDir::new("studio-readiness");
    project(dir.path());
    let studio = Serving::start("readiness");
    studio.post(&format!("/api/projects?path={}", encoded(dir.path())));

    let page = studio.get(&format!("/api/project?path={}", encoded(dir.path())));

    let doctor = run_env(
        &["doctor", &dir.path().display().to_string(), "--json"],
        &[],
    );
    let printed: Value = serde_json::from_str(&stdout(&doctor)).expect("doctor must print JSON");

    assert_eq!(page["readiness"], printed, "the page and the doctor differ");
}

#[test]
fn the_diagnostics_on_the_page_are_the_ones_check_refuses_for() {
    let dir = TempDir::new("studio-diagnostics");
    project(dir.path());
    // A policy that denies the network, and a tool that needs it.
    std::fs::write(
        dir.path().join("main.ing"),
        "language 0.1\n\
         tool search(query: string) -> text @mcp(\"web.search\") effects(network)\n\
         agent Note(topic: string) -> note<markdown> {\n\
         \x20 model requires { structured_output }\n\
         \x20 budget { steps <= 2 }\n\
         \x20 policy { network deny }\n\
         \x20 tools { search }\n\
         \x20 flow {\n\
         \x20   let hits = search(topic)\n\
         \x20   emit note = ask<markdown>(\"About ${hits}.\")\n\
         \x20 }\n\
         }\n",
    )
    .expect("writing the source");

    let studio = Serving::start("diagnostics");
    studio.post(&format!("/api/projects?path={}", encoded(dir.path())));
    let page = studio.get(&format!("/api/project?path={}", encoded(dir.path())));

    assert_eq!(page["compiles"], false);
    let codes: Vec<&str> = page["diagnostics"]
        .as_array()
        .expect("an array")
        .iter()
        .filter_map(|diagnostic| diagnostic["code"].as_str())
        .collect();
    assert!(!codes.is_empty(), "{page}");

    // Every code the page shows is a code `ingot check` also reports, with the
    // same text: the page renders the compiler's diagnostics rather than its
    // own account of them.
    let checked = run_env(&["check", &dir.path().display().to_string()], &[]);
    let printed = stderr(&checked);
    for code in codes {
        assert!(
            printed.contains(code),
            "check never mentioned {code}:\n{printed}"
        );
    }
}

#[test]
fn a_run_leaves_a_record_the_studio_shows() {
    let dir = TempDir::new("studio-history");
    project(dir.path());
    let ran = run_once(dir.path());
    assert_eq!(code(&ran), 0, "{}", stderr(&ran));

    let studio = Serving::start("history");
    studio.post(&format!("/api/projects?path={}", encoded(dir.path())));

    let runs = studio.get(&format!("/api/runs?path={}", encoded(dir.path())));
    let listed = runs["runs"].as_array().expect("an array");
    assert_eq!(listed.len(), 1, "{runs}");
    assert_eq!(listed[0]["agent"], "Note");
    assert_eq!(listed[0]["state"], "finished");
    assert!(listed[0]["startedUnix"].as_u64().unwrap_or(0) > 0);

    let id = listed[0]["id"].as_str().expect("an id");
    let detail = studio.get(&format!("/api/run?path={}&id={id}", encoded(dir.path())));
    let events = detail["events"].as_array().expect("an array");
    assert!(!events.is_empty());
    assert_eq!(events[0]["event"], "runStarted");
    assert_eq!(events.last().expect("a last event")["event"], "runFinished");
    // Every recorded line is an event. The live text a model produced while it
    // was answering is not one, and a record holding it would be a record no
    // replay could reproduce.
    for event in events {
        assert!(event.get("event").is_some(), "not an event: {event}");
        assert!(
            event.get("delta").is_none(),
            "a delta was recorded: {event}"
        );
    }
}

#[test]
fn a_recorded_run_is_the_json_event_stream_the_terminal_printed() {
    // The record is not a re-encoding. A consumer of `--events json` is a
    // consumer of the file, which is what keeps the two from drifting.
    let dir = TempDir::new("studio-verbatim");
    project(dir.path());
    let stub = stub_provider(vec![text_reply("# Compilers\n\nShort.")]);
    let ran = run_env(
        &[
            "run",
            &dir.path().display().to_string(),
            "--provider",
            "anthropic",
            "--events",
            "json",
            "--input",
            "topic=compilers",
        ],
        &[
            ("ANTHROPIC_API_KEY", "stub-key"),
            ("INGOT_ANTHROPIC_BASE_URL", &stub.url),
        ],
    );
    assert_eq!(code(&ran), 0, "{}", stderr(&ran));

    let printed: Vec<Value> = stderr(&ran)
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|value| value.get("event").is_some())
        .collect();

    let record = std::fs::read_dir(dir.path().join("target/ingot/runs"))
        .expect("a runs directory")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .next()
        .expect("a record");
    let stored: Vec<Value> = std::fs::read_to_string(&record)
        .expect("the record must read")
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|value| value.get("event").is_some())
        .collect();

    assert_eq!(stored, printed);
}

#[test]
fn no_event_in_a_record_carries_a_clock() {
    // Wall-clock time belongs to the two `record` lines, which describe this
    // execution. An event describes the program, and Runtime 0.1 §9 requires a
    // replay to reproduce the sequence byte for byte.
    let dir = TempDir::new("studio-clockless");
    project(dir.path());
    assert_eq!(code(&run_once(dir.path())), 0);

    let record = std::fs::read_dir(dir.path().join("target/ingot/runs"))
        .expect("a runs directory")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .next()
        .expect("a record");
    let text = std::fs::read_to_string(&record).expect("the record must read");

    let mut records = 0;
    for line in text.lines() {
        let value: Value = serde_json::from_str(line).expect("every line is JSON");
        if value.get("record").is_some() {
            records += 1;
            continue;
        }
        let printed = value.to_string();
        for clock in ["Unix", "timestamp", "elapsed", "startedAt", "duration"] {
            assert!(
                !printed.contains(clock),
                "an event carries a clock: {printed}"
            );
        }
    }
    assert_eq!(records, 2, "a finished run opens and closes its record");
}

#[test]
fn a_run_told_to_keep_no_history_keeps_none() {
    let dir = TempDir::new("studio-no-history");
    project(dir.path());
    let stub = stub_provider(vec![text_reply("# Compilers\n\nShort.")]);
    let ran = run_env(
        &[
            "run",
            &dir.path().display().to_string(),
            "--provider",
            "anthropic",
            "--no-history",
            "--input",
            "topic=compilers",
        ],
        &[
            ("ANTHROPIC_API_KEY", "stub-key"),
            ("INGOT_ANTHROPIC_BASE_URL", &stub.url),
        ],
    );
    assert_eq!(code(&ran), 0, "{}", stderr(&ran));
    assert!(!dir.path().join("target/ingot/runs").exists());
}

#[test]
fn deleting_a_record_removes_that_record_and_leaves_the_rest() {
    let dir = TempDir::new("studio-delete");
    project(dir.path());
    assert_eq!(code(&run_once(dir.path())), 0);

    let studio = Serving::start("delete");
    studio.post(&format!("/api/projects?path={}", encoded(dir.path())));
    let runs = studio.get(&format!("/api/runs?path={}", encoded(dir.path())));
    let id = runs["runs"][0]["id"].as_str().expect("an id").to_string();

    let (status, body) = studio.request(
        "DELETE",
        &format!("/api/run?path={}&id={id}", encoded(dir.path())),
    );
    assert_eq!(status, 200, "{body}");
    let after: Value = serde_json::from_str(&body).expect("a JSON reply");
    assert_eq!(after["runs"].as_array().expect("an array").len(), 0);

    // The project's own build output is not history and must survive.
    assert!(dir.path().join("main.ing").is_file());
}

#[test]
fn an_identifier_from_a_url_cannot_reach_outside_the_run_directory() {
    let dir = TempDir::new("studio-traversal");
    project(dir.path());
    let studio = Serving::start("traversal");
    studio.post(&format!("/api/projects?path={}", encoded(dir.path())));

    for hostile in ["..%2F..%2Fingot.toml", "..", "main", "1-2%2F..%2F..%2Fx"] {
        let (status, body) = studio.request(
            "GET",
            &format!("/api/run?path={}&id={hostile}", encoded(dir.path())),
        );
        assert_eq!(status, 400, "`{hostile}` was not refused: {body}");
    }
}

#[test]
fn the_machine_page_names_variables_and_never_carries_a_value() {
    // The rule the whole toolchain keeps: a credential is read from the
    // environment at the moment of a request and is never written anywhere,
    // including into a page about whether it is set.
    let config = TempDir::new("studio-machine-config");
    let mut child = Command::new(binary())
        .args(["studio", "--bind", "127.0.0.1:0"])
        .env("INGOT_CONFIG_DIR", config.path())
        .env("ANTHROPIC_API_KEY", "sk-ant-do-not-print-this")
        .env_remove("OPENAI_API_KEY")
        .env_remove("GEMINI_API_KEY")
        .env_remove("GOOGLE_API_KEY")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("the ingot binary must be runnable");

    let mut line = String::new();
    BufReader::new(child.stdout.as_mut().expect("a piped stdout"))
        .read_line(&mut line)
        .expect("the studio must print its URL");
    let url = line.trim().to_string();
    let (authority, token) = url
        .strip_prefix("http://")
        .and_then(|rest| rest.split_once("/?token="))
        .map(|(authority, token)| (authority.to_string(), token.to_string()))
        .expect("a studio URL");

    let mut stream = TcpStream::connect(&authority).expect("a connection");
    write!(
        stream,
        "GET /api/machine HTTP/1.1\r\nHost: {authority}\r\nX-Ingot-Token: {token}\r\n\r\n"
    )
    .expect("the request must be written");
    stream.flush().expect("the request must flush");
    let mut reply = String::new();
    stream.read_to_string(&mut reply).expect("a reply");
    let _ = child.kill();
    let _ = child.wait();

    let body = reply
        .split_once("\r\n\r\n")
        .map(|(_, body)| body)
        .unwrap_or("");
    assert!(
        !body.contains("sk-ant-do-not-print-this"),
        "the machine page reproduced a credential:\n{body}"
    );
    assert!(body.contains("ANTHROPIC_API_KEY"), "{body}");

    let machine: Value = serde_json::from_str(body).expect("a JSON reply");
    let anthropic = machine["providers"]
        .as_array()
        .expect("an array")
        .iter()
        .find(|provider| provider["name"] == "anthropic")
        .expect("anthropic must be listed");
    assert_eq!(anthropic["variables"][0]["set"], true);
    let openai = machine["providers"]
        .as_array()
        .expect("an array")
        .iter()
        .find(|provider| provider["name"] == "openai")
        .expect("openai must be listed");
    assert_eq!(openai["variables"][0]["set"], false);
}

#[test]
fn a_run_started_from_the_studio_is_the_same_run_a_terminal_would_have_made() {
    let dir = TempDir::new("studio-start");
    project(dir.path());
    let stub = stub_provider(vec![text_reply("# Compilers\n\nShort.")]);
    let studio = Serving::start_with(
        "start",
        &[
            ("ANTHROPIC_API_KEY", "stub-key"),
            ("INGOT_ANTHROPIC_BASE_URL", &stub.url),
        ],
    );
    studio.post(&format!("/api/projects?path={}", encoded(dir.path())));

    let (status, body) = studio.send_json(
        "POST",
        &format!("/api/run?path={}", encoded(dir.path())),
        r#"{"agent":"Note","provider":"anthropic","inputs":{"topic":"compilers"}}"#,
    );
    assert_eq!(status, 200, "{body}");
    let started: Value = serde_json::from_str(&body).expect("a JSON reply");
    let pid = started["launches"][0]["pid"].as_u64().expect("a pid");

    // Both halves, because they arrive separately and that is the design: the
    // child closes its record when the interpreter is done, and exits a moment
    // later once its artifacts are written.
    let answer = studio.until(dir.path(), |answer| {
        let recorded = answer["runs"]
            .as_array()
            .map(|runs| runs.iter().any(|run| run["state"] == "finished"))
            .unwrap_or(false);
        let exited = answer["launches"]
            .as_array()
            .map(|launches| launches.iter().all(|launch| launch["state"] != "running"))
            .unwrap_or(false);
        recorded && exited
    });

    let run = &answer["runs"][0];
    assert_eq!(run["agent"], "Note");
    assert_eq!(run["provider"], "anthropic");
    // A launch and a record are joined by the process id — the record's
    // identifier ends in the pid of the process that wrote it — so nothing new
    // has to cross between the studio and the run it started.
    assert!(
        run["id"]
            .as_str()
            .expect("an id")
            .ends_with(&format!("-{pid}")),
        "{run}"
    );

    let launch = answer["launches"]
        .as_array()
        .expect("an array")
        .iter()
        .find(|launch| launch["pid"] == pid)
        .expect("the launch must still be listed");
    assert_eq!(launch["state"], "exited");
    assert_eq!(launch["exitCode"], 0);
}

#[test]
fn a_run_that_fails_before_recording_anything_is_still_reported() {
    // The failure this exists for: `ingot run` refusing at compile time writes
    // no record at all, and a button that appears to do nothing is worse than
    // an error. The launch is what carries the message.
    let dir = TempDir::new("studio-start-broken");
    project(dir.path());
    std::fs::write(dir.path().join("main.ing"), "language 0.1\nagent Broken(\n")
        .expect("writing a source that cannot compile");

    let studio = Serving::start("start-broken");
    studio.post(&format!("/api/projects?path={}", encoded(dir.path())));
    let (status, body) = studio.send_json(
        "POST",
        &format!("/api/run?path={}", encoded(dir.path())),
        r#"{"provider":"auto"}"#,
    );
    assert_eq!(status, 200, "{body}");

    let answer = studio.until(dir.path(), |answer| {
        answer["launches"]
            .as_array()
            .map(|launches| launches.iter().any(|launch| launch["state"] == "failed"))
            .unwrap_or(false)
    });
    assert_eq!(answer["runs"].as_array().expect("an array").len(), 0);
    let launch = &answer["launches"][0];
    assert_ne!(launch["exitCode"], 0);
    assert!(
        launch["log"].as_str().expect("a log").contains("ING"),
        "the log must carry the diagnostic: {launch}"
    );
}

#[test]
fn the_studio_will_not_start_a_run_with_a_field_the_page_invented() {
    // `--yes` would turn an effect that asks for a person into one that does
    // not, and `--no-history` would produce a run the studio could never show
    // again. Neither is a field, and an unknown one is refused rather than
    // ignored — the same rule the manifest keeps about a literal secret.
    let dir = TempDir::new("studio-start-fields");
    project(dir.path());
    let studio = Serving::start("start-fields");
    studio.post(&format!("/api/projects?path={}", encoded(dir.path())));

    for hostile in [
        r#"{"provider":"auto","yes":true}"#,
        r#"{"provider":"auto","noHistory":true}"#,
        r#"{"provider":"auto","args":["--yes"]}"#,
    ] {
        let (status, body) = studio.send_json(
            "POST",
            &format!("/api/run?path={}", encoded(dir.path())),
            hostile,
        );
        assert_eq!(status, 400, "`{hostile}` was accepted: {body}");
    }

    let (status, body) = studio.send_json(
        "POST",
        &format!("/api/run?path={}", encoded(dir.path())),
        r#"{"provider":"curl"}"#,
    );
    assert_eq!(status, 400, "{body}");
    assert!(body.contains("not a provider"), "{body}");

    // And nothing was started by any of it.
    let answer = studio.get(&format!("/api/runs?path={}", encoded(dir.path())));
    assert_eq!(answer["launches"].as_array().expect("an array").len(), 0);
}

#[test]
fn stopping_a_process_this_studio_did_not_start_is_refused() {
    let dir = TempDir::new("studio-stop");
    project(dir.path());
    let studio = Serving::start("stop");
    studio.post(&format!("/api/projects?path={}", encoded(dir.path())));

    let (status, body) = studio.request(
        "DELETE",
        &format!("/api/launch?path={}&pid=1", encoded(dir.path())),
    );
    assert_eq!(status, 400, "{body}");
    assert!(body.contains("did not start"), "{body}");
}

#[test]
fn the_boundary_the_page_shows_is_the_one_sandbox_prints() {
    let dir = TempDir::new("studio-boundary");
    project(dir.path());
    let studio = Serving::start("boundary");
    studio.post(&format!("/api/projects?path={}", encoded(dir.path())));

    let page = studio.get(&format!("/api/project?path={}", encoded(dir.path())));
    // This project configures no tool server, so there is nothing to contain —
    // and the page says so by having no plans rather than by inventing one.
    assert_eq!(
        page["boundary"]["plans"]
            .as_array()
            .expect("an array")
            .len(),
        0
    );
    assert_eq!(
        page["boundary"]["problems"]
            .as_array()
            .expect("an array")
            .len(),
        0
    );
}

// --- answering a gate -------------------------------------------------------
//
// [GAP-041](../../../docs/gaps.md#gap-041) in one sentence: an agent that needs
// a person could not be run from the one surface built for people. These drive
// the gate from the page's side.
//
// What RFC-0015 refused is still refused. `--yes` is a blanket answer given
// before the run to gates nobody has seen; this is one gate, at the moment it is
// reached, with the effect and the reason in front of whoever answers it.

/// A studio pointed at a stub, and a project whose write is gated.
fn gated(tag: &str) -> (TempDir, StubProvider, Serving) {
    let dir = TempDir::new(&format!("studio-{tag}"));
    gated_project(dir.path());
    let stub = stub_provider(vec![text_reply("A line about the harbour.\n")]);
    let studio = Serving::start_with(
        tag,
        &[
            ("ANTHROPIC_API_KEY", "stub-key"),
            ("INGOT_ANTHROPIC_BASE_URL", &stub.url),
        ],
    );
    studio.post(&format!("/api/projects?path={}", encoded(dir.path())));
    (dir, stub, studio)
}

/// Start the gated run and wait until the page is offering its gate.
fn until_waiting(studio: &Serving, dir: &Path) -> (u32, String) {
    let (status, body) = studio.send_json(
        "POST",
        &format!("/api/run?path={}", encoded(dir)),
        r#"{"provider":"anthropic","inputs":{"note":"the harbour"}}"#,
    );
    assert_eq!(status, 200, "{body}");

    let answer = studio.until(dir, |answer| answer["launches"][0]["pending"].is_object());
    let launch = &answer["launches"][0];
    assert_eq!(
        launch["state"], "running",
        "a run waiting at a gate is still running: {launch}"
    );
    (
        launch["pid"].as_u64().expect("a pid") as u32,
        launch["pending"]["node"]
            .as_str()
            .expect("the gate names its node")
            .to_string(),
    )
}

#[test]
fn a_gate_reaches_the_page_and_answering_it_lets_the_run_finish() {
    let (dir, _stub, studio) = gated("gate-allow");
    let (pid, node) = until_waiting(&studio, dir.path());

    // What the person is shown before answering: the effect, and the reason the
    // compiler attached. Approving something unnamed is the thing RFC-0015
    // refused, and it is refused by showing this rather than by a rule.
    let answer = studio.get(&format!("/api/runs?path={}", encoded(dir.path())));
    let gate = &answer["launches"][0]["pending"];
    assert_eq!(gate["effects"][0], "filesystem_write", "{gate}");
    assert!(
        gate["reason"]
            .as_str()
            .map(|r| !r.is_empty())
            .unwrap_or(false),
        "the gate has to say what is about to happen: {gate}"
    );

    let (status, body) = studio.send_json(
        "POST",
        &format!("/api/approval?path={}&pid={}", encoded(dir.path()), pid),
        &format!(r#"{{"node":"{node}","allowed":true}}"#),
    );
    assert_eq!(status, 200, "{body}");

    studio.until(dir.path(), |answer| {
        answer["launches"][0]["state"] == "exited"
    });
    assert!(
        dir.path().join("data/out/note.md").is_file(),
        "the gate was opened, so the write must have happened"
    );
}

#[test]
fn a_gate_refused_from_the_page_stops_the_run_before_the_effect() {
    let (dir, _stub, studio) = gated("gate-refuse");
    let (pid, node) = until_waiting(&studio, dir.path());

    let (status, body) = studio.send_json(
        "POST",
        &format!("/api/approval?path={}&pid={}", encoded(dir.path()), pid),
        &format!(r#"{{"node":"{node}","allowed":false}}"#),
    );
    assert_eq!(status, 200, "{body}");

    let answer = studio.until(dir.path(), |answer| {
        answer["launches"][0]["state"] == "failed"
    });
    assert!(
        !dir.path().join("data/out/note.md").exists(),
        "the gate was refused, so nothing may have been written: {answer}"
    );
}

#[test]
fn an_answer_naming_a_gate_the_run_is_not_at_is_refused() {
    // A tab left open showing an older gate. Applying its answer would decide
    // the gate the run is actually at with the intent of one already settled.
    let (dir, _stub, studio) = gated("gate-stale");
    let (pid, node) = until_waiting(&studio, dir.path());

    let (status, body) = studio.send_json(
        "POST",
        &format!("/api/approval?path={}&pid={}", encoded(dir.path()), pid),
        r#"{"node":"a-gate-from-another-run","allowed":true}"#,
    );
    assert_eq!(status, 400, "{body}");
    assert!(
        body.contains(&node),
        "the refusal names the real gate: {body}"
    );

    // And the run is still waiting, rather than having been decided either way.
    let answer = studio.get(&format!("/api/runs?path={}", encoded(dir.path())));
    assert_eq!(answer["launches"][0]["pending"]["node"], node.as_str());
}

#[test]
fn an_answer_the_page_invented_a_field_for_is_refused() {
    let (dir, _stub, studio) = gated("gate-field");
    let (pid, node) = until_waiting(&studio, dir.path());

    let (status, body) = studio.send_json(
        "POST",
        &format!("/api/approval?path={}&pid={}", encoded(dir.path()), pid),
        &format!(r#"{{"node":"{node}","allowed":true,"forever":true}}"#),
    );
    assert_eq!(status, 400, "{body}");
    assert_eq!(
        studio.get(&format!("/api/runs?path={}", encoded(dir.path())))["launches"][0]["pending"]
            ["node"],
        node.as_str()
    );
}

#[test]
fn answering_a_process_this_studio_did_not_start_is_refused() {
    let dir = TempDir::new("studio-gate-stranger");
    project(dir.path());
    let studio = Serving::start("gate-stranger");
    studio.post(&format!("/api/projects?path={}", encoded(dir.path())));

    let (status, body) = studio.send_json(
        "POST",
        &format!("/api/approval?path={}&pid=999999", encoded(dir.path())),
        r#"{"node":"n0","allowed":true}"#,
    );
    assert_eq!(status, 400, "{body}");
    assert!(body.contains("did not start"), "{body}");
}
