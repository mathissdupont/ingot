//! Run history: the event stream, on disk, and the only thing the studio keeps.
//!
//! Everything else [`crate::studio`] shows is re-derived from a directory every
//! time it is asked. A run is different — it happens once and is then gone —
//! so a run that nobody recorded cannot be shown afterwards at all.
//!
//! # The format is the event stream and nothing more
//!
//! A record is JSON Lines. The middle of the file is exactly what
//! `ingot run --events json` writes to its standard error: one event object per
//! line, byte for byte, with no field added and none removed. Around it are two
//! lines carrying a `record` key instead of an `event` key.
//!
//! That distinction is the whole design. [Runtime 0.1 §9] requires a replayed
//! run to reproduce its event sequence exactly, so an event may not carry a
//! clock: two runs of the same artifact against the same cassette have to
//! produce the same bytes. Wall-clock time, the process id and the outcome are
//! facts about *this* execution rather than about the program, so they live in
//! the `record` lines — where a replay is not expected to reproduce them, and
//! a reader can tell at a glance which is which.
//!
//! # An unfinished file means what it says
//!
//! The trailing record line is written when the run ends. A file without one
//! is a run that started and did not report a result: it may be going right
//! now, or the process may have been killed. Nothing here guesses which. The
//! studio shows that state under that name, because a history that quietly
//! reports interrupted runs as running would be worse than no history.
//!
//! [Runtime 0.1 §9]: ../../../specs/runtime/v0.1.md

use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use ingot_runtime::{RunEvent, Usage};
use serde::Serialize;
use serde_json::Value;

/// Where records live inside a project's build directory.
///
/// Under `target/ingot` rather than beside the source, because a run record is
/// output: it is disposable, it is already ignored by version control, and
/// deleting the build directory is expected to lose it.
pub const RUNS_DIR: &str = "runs";

const RECORD_SCHEMA_VERSION: u32 = 1;

/// Do not read a directory forever because somebody left a run in a loop.
///
/// The studio lists newest first, so the cut falls on the oldest records and
/// the page says when it happened rather than pretending the list is complete.
const MAX_LISTED: usize = 500;

// --- writing ---------------------------------------------------------------

/// An open record, appending as the run goes.
pub struct RunRecorder {
    path: PathBuf,
    file: File,
    id: String,
    started: u64,
    closed: bool,
}

impl RunRecorder {
    /// Open a record for a run that has just started.
    ///
    /// Returns `None` when the directory cannot be created or the file cannot
    /// be opened. A history is a convenience; a run must not fail because one
    /// could not be kept, and the run's own output already went to the terminal.
    pub fn begin(out_dir: &Path, agent: &str, provider: &str, contained: bool) -> Option<Self> {
        let directory = out_dir.join(RUNS_DIR);
        std::fs::create_dir_all(&directory).ok()?;

        let started = now();
        // Zero-padded so a lexical sort is a chronological one, and suffixed
        // with the process id so two runs starting in the same second are two
        // records rather than one interleaved mess.
        let id = format!("{started:010}-{}", std::process::id());
        let path = directory.join(format!("{id}.jsonl"));
        let file = File::create(&path).ok()?;

        let mut recorder = RunRecorder {
            path,
            file,
            id,
            started,
            closed: false,
        };
        let header = serde_json::json!({
            "record": "started",
            "schemaVersion": RECORD_SCHEMA_VERSION,
            "id": recorder.id,
            "agent": agent,
            "provider": provider,
            "contained": contained,
            "startedUnix": started,
        });
        recorder.write(&header.to_string());
        Some(recorder)
    }

    /// Append one event, exactly as the JSON event stream would print it.
    pub fn event(&mut self, event: &RunEvent) {
        let line = event.to_json_line();
        self.write(&line);
    }

    /// Close the record with what the run reported.
    pub fn finish(&mut self, outcome: Outcome<'_>) {
        if self.closed {
            return;
        }
        self.closed = true;
        let mut line = serde_json::json!({
            "record": "finished",
            "finishedUnix": now(),
            "startedUnix": self.started,
        });
        let object = line.as_object_mut().expect("a literal object");
        match outcome {
            Outcome::Finished { steps, usage, cost } => {
                object.insert("ok".into(), Value::Bool(true));
                object.insert("steps".into(), Value::from(steps));
                object.insert(
                    "usage".into(),
                    serde_json::to_value(usage).unwrap_or(Value::Null),
                );
                if let Some(cost) = cost {
                    object.insert("cost".into(), Value::from(cost));
                }
            }
            Outcome::Failed { reason } => {
                object.insert("ok".into(), Value::Bool(false));
                object.insert("reason".into(), Value::from(reason));
            }
        }
        self.write(&line.to_string());
    }

    /// Where this record is being written, for a message after the run.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Flushed after every line so a run that is still going can be read while
    /// it goes, and so a run that is killed keeps everything up to that point.
    fn write(&mut self, line: &str) {
        let _ = writeln!(self.file, "{line}");
        let _ = self.file.flush();
    }
}

/// How a run ended, as the command that ran it saw it.
pub enum Outcome<'a> {
    Finished {
        steps: u32,
        usage: Usage,
        /// The rendered cost, when the artifact declared prices to charge it
        /// against. Rendered rather than numeric so the record says what the
        /// terminal said.
        cost: Option<String>,
    },
    Failed {
        reason: &'a str,
    },
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or_default()
}

// --- reading ---------------------------------------------------------------

/// What a run was, without its events.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunSummary {
    pub id: String,
    pub agent: String,
    pub provider: String,
    pub contained: bool,
    pub started_unix: u64,
    pub finished_unix: Option<u64>,
    /// `finished`, `failed`, or `unfinished`.
    pub state: &'static str,
    pub steps: Option<u32>,
    pub usage: Option<Usage>,
    pub cost: Option<String>,
    pub reason: Option<String>,
    /// How many events the record holds.
    ///
    /// Named apart from [`RunDetail::events`] on purpose: the two are flattened
    /// into one object, and two fields called `events` would emit the key
    /// twice — leaving which one a reader sees up to its parser.
    pub event_count: usize,
}

/// A run and every event it recorded.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunDetail {
    #[serde(flatten)]
    pub summary: RunSummary,
    /// The event objects verbatim, in the order they were emitted.
    pub events: Vec<Value>,
}

/// Every record in a project's build directory, newest first.
pub fn list(out_dir: &Path) -> Vec<RunSummary> {
    let directory = out_dir.join(RUNS_DIR);
    let Ok(entries) = std::fs::read_dir(&directory) else {
        return Vec::new();
    };

    let mut ids: Vec<String> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            name.strip_suffix(".jsonl")
                .filter(|id| is_record_id(id))
                .map(str::to_string)
        })
        .collect();
    ids.sort();
    ids.reverse();
    ids.truncate(MAX_LISTED);

    ids.iter()
        .filter_map(|id| read(out_dir, id).ok().map(|detail| detail.summary))
        .collect()
}

/// How many records a project has, without parsing any of them.
pub fn count(out_dir: &Path) -> usize {
    std::fs::read_dir(out_dir.join(RUNS_DIR))
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|entry| {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    name.strip_suffix(".jsonl")
                        .map(is_record_id)
                        .unwrap_or(false)
                })
                .count()
        })
        .unwrap_or(0)
}

/// One record, with its events.
pub fn read(out_dir: &Path, id: &str) -> Result<RunDetail> {
    let path = record_path(out_dir, id)?;
    let file = File::open(&path).with_context(|| format!("reading {}", path.display()))?;

    let mut header: Option<Value> = None;
    let mut trailer: Option<Value> = None;
    let mut events = Vec::new();
    for line in BufReader::new(file).lines() {
        let Ok(line) = line else { break };
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            // A half-written last line is what a killed process leaves. Stop
            // there rather than refusing to show the run at all.
            break;
        };
        match value.get("record").and_then(Value::as_str) {
            Some("started") => header = Some(value),
            Some(_) => trailer = Some(value),
            None if value.get("event").is_some() => events.push(value),
            None => {}
        }
    }

    let Some(header) = header else {
        bail!("{} has no opening record line", path.display());
    };

    let string = |value: &Value, key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_default()
    };
    let ok = trailer
        .as_ref()
        .and_then(|value| value.get("ok"))
        .and_then(Value::as_bool);

    let summary = RunSummary {
        id: id.to_string(),
        agent: string(&header, "agent"),
        provider: string(&header, "provider"),
        contained: header
            .get("contained")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        started_unix: header
            .get("startedUnix")
            .and_then(Value::as_u64)
            .unwrap_or_default(),
        finished_unix: trailer
            .as_ref()
            .and_then(|value| value.get("finishedUnix"))
            .and_then(Value::as_u64),
        state: match ok {
            Some(true) => "finished",
            Some(false) => "failed",
            None => "unfinished",
        },
        steps: trailer
            .as_ref()
            .and_then(|value| value.get("steps"))
            .and_then(Value::as_u64)
            .map(|steps| steps as u32),
        usage: trailer
            .as_ref()
            .and_then(|value| value.get("usage"))
            .and_then(|usage| serde_json::from_value(usage.clone()).ok()),
        cost: trailer
            .as_ref()
            .and_then(|value| value.get("cost"))
            .and_then(Value::as_str)
            .map(str::to_string),
        reason: trailer
            .as_ref()
            .and_then(|value| value.get("reason"))
            .and_then(Value::as_str)
            .map(str::to_string),
        event_count: events.len(),
    };
    Ok(RunDetail { summary, events })
}

/// The record a running process is writing, if it has opened one.
///
/// A launch and a record are the same run seen from two ends: this process
/// knows a child by its process id, and the child names its own record. They
/// meet because a record id ends with the process id that opened it — see
/// [`RunRecorder::begin`] — so the join needs no new bookkeeping and no
/// message from the child.
///
/// Two things keep the join honest. The suffix is matched rather than the id
/// parsed, because the timestamp half is not this caller's business. And a
/// record older than the launch is not a candidate: an operating system reuses
/// process ids, so a finished run from a previous process with the same id is
/// exactly what a bare suffix match would find for a launch that has not
/// written its first line yet.
///
/// `records` is expected newest first, as [`list`] returns them.
pub fn of_process(records: &[RunSummary], pid: u32, not_before: u64) -> Option<String> {
    let suffix = format!("-{pid}");
    records
        .iter()
        .find(|record| record.id.ends_with(&suffix) && record.started_unix >= not_before)
        .map(|record| record.id.clone())
}

/// Remove one record.
pub fn delete(out_dir: &Path, id: &str) -> Result<()> {
    let path = record_path(out_dir, id)?;
    std::fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))
}

/// The file an id names, once the id has been shown to be an id.
///
/// The studio takes this from a URL, so it is checked against the shape a
/// record id has rather than sanitised: `..`, a separator or a drive letter is
/// simply not one, and there is no encoding of those that is.
fn record_path(out_dir: &Path, id: &str) -> Result<PathBuf> {
    if !is_record_id(id) {
        bail!("`{id}` is not a run identifier");
    }
    Ok(out_dir.join(RUNS_DIR).join(format!("{id}.jsonl")))
}

/// `<seconds>-<pid>`, digits only on both sides.
fn is_record_id(id: &str) -> bool {
    match id.split_once('-') {
        Some((seconds, pid)) => {
            !seconds.is_empty()
                && !pid.is_empty()
                && seconds.bytes().all(|byte| byte.is_ascii_digit())
                && pid.bytes().all(|byte| byte.is_ascii_digit())
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dir(PathBuf);

    impl Dir {
        fn new(name: &str) -> Dir {
            let path =
                std::env::temp_dir().join(format!("ingot-runs-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("a temporary directory");
            Dir(path)
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_record_holds_the_event_stream_verbatim() {
        let dir = Dir::new("verbatim");
        let mut recorder =
            RunRecorder::begin(&dir.0, "Research", "anthropic", false).expect("a recorder");
        let event = RunEvent::NodeStarted {
            node: "n1".into(),
            kind: "model.call".into(),
        };
        recorder.event(&event);
        recorder.finish(Outcome::Finished {
            steps: 1,
            usage: Usage::default(),
            cost: None,
        });
        let id = recorder.id.clone();
        let path = recorder.path().to_path_buf();
        drop(recorder);

        let detail = read(&dir.0, &id).expect("the record must read back");
        assert_eq!(detail.summary.state, "finished");
        assert_eq!(detail.summary.agent, "Research");
        assert_eq!(detail.events.len(), 1);

        // The summary is flattened into the detail, so no field of one may
        // share a name with a field of the other: a duplicated key leaves which
        // value a reader sees up to its parser.
        let encoded = serde_json::to_string(&detail).expect("a detail serializes");
        assert_eq!(encoded.matches("\"events\":").count(), 1, "{encoded}");

        // Asserted on the bytes rather than on a parsed value, because verbatim
        // is the claim: the middle of the file is what `--events json` printed,
        // so a consumer of one is a consumer of the other. Comparing parsed
        // values would pass even if the record re-encoded every line.
        let stored = std::fs::read_to_string(&path).expect("the record must read");
        let lines: Vec<&str> = stored.lines().collect();
        assert_eq!(lines[1], event.to_json_line());
        // The two lines around it are the ones a replay is not expected to
        // reproduce, and they say which they are by carrying `record` where an
        // event carries `event`.
        assert!(lines[0].contains("\"record\":\"started\""), "{}", lines[0]);
        assert!(lines[2].contains("\"record\":\"finished\""), "{}", lines[2]);
        assert!(!lines[1].contains("\"record\""));
    }

    #[test]
    fn a_run_that_never_reported_a_result_is_unfinished_rather_than_guessed_at() {
        let dir = Dir::new("unfinished");
        let mut recorder =
            RunRecorder::begin(&dir.0, "Research", "replay", false).expect("a recorder");
        recorder.event(&RunEvent::RunStarted {
            agent: "Research".into(),
            provider: "replay".into(),
        });
        let id = recorder.id.clone();
        drop(recorder);

        let summary = read(&dir.0, &id)
            .expect("the record must read back")
            .summary;
        assert_eq!(summary.state, "unfinished");
        assert_eq!(summary.finished_unix, None);
    }

    #[test]
    fn a_half_written_line_does_not_hide_the_run() {
        // What a killed process leaves behind. The run is still worth showing.
        let dir = Dir::new("truncated");
        let directory = dir.0.join(RUNS_DIR);
        std::fs::create_dir_all(&directory).expect("a runs directory");
        std::fs::write(
            directory.join("0000000001-7.jsonl"),
            "{\"record\":\"started\",\"id\":\"0000000001-7\",\"agent\":\"A\",\"startedUnix\":1}\n\
             {\"event\":\"nodeStarted\",\"node\":\"n1\",\"kind\":\"model.ca",
        )
        .expect("a truncated record");

        let summary = read(&dir.0, "0000000001-7")
            .expect("a truncated record must still read")
            .summary;
        assert_eq!(summary.agent, "A");
        assert_eq!(summary.event_count, 0);
        assert_eq!(summary.state, "unfinished");
    }

    #[test]
    fn an_identifier_from_a_url_cannot_name_a_file_outside_the_directory() {
        let dir = Dir::new("traversal");
        for hostile in [
            "../../etc/passwd",
            "..",
            "1-2/../../x",
            r"..\..\windows",
            "C:1-2",
            "1-2.jsonl",
            "",
        ] {
            assert!(
                record_path(&dir.0, hostile).is_err(),
                "`{hostile}` must not name a record"
            );
        }
        assert!(record_path(&dir.0, "0000000001-7").is_ok());
    }

    #[test]
    fn records_are_listed_newest_first() {
        let dir = Dir::new("order");
        let directory = dir.0.join(RUNS_DIR);
        std::fs::create_dir_all(&directory).expect("a runs directory");
        for (id, started) in [
            ("0000000001-1", 1),
            ("0000000009-1", 9),
            ("0000000005-1", 5),
        ] {
            std::fs::write(
                directory.join(format!("{id}.jsonl")),
                format!("{{\"record\":\"started\",\"agent\":\"A\",\"startedUnix\":{started}}}\n"),
            )
            .expect("a record");
        }
        let ids: Vec<String> = list(&dir.0).into_iter().map(|run| run.id).collect();
        assert_eq!(ids, ["0000000009-1", "0000000005-1", "0000000001-1"]);
        assert_eq!(count(&dir.0), 3);
    }

    #[test]
    fn a_launch_is_joined_to_its_own_record_and_not_to_a_reused_process_id() {
        let dir = Dir::new("join");
        let directory = dir.0.join(RUNS_DIR);
        std::fs::create_dir_all(&directory).expect("a runs directory");
        // Two records written by process 4242: one long finished, one this
        // launch's own. An operating system reuses process ids, so both exist.
        for (id, started) in [("0000000100-4242", 100), ("0000000900-4242", 900)] {
            std::fs::write(
                directory.join(format!("{id}.jsonl")),
                format!(
                    "{{\"record\":\"started\",\"agent\":\"A\",\"startedUnix\":{started}}}
"
                ),
            )
            .expect("a record");
        }
        let records = list(&dir.0);

        assert_eq!(
            of_process(&records, 4242, 900).as_deref(),
            Some("0000000900-4242"),
            "a launch must find the record opened after it started"
        );
        assert_eq!(
            of_process(&records, 4242, 1500),
            None,
            "a launch that has not written its first line has no record yet, and              the old one belonging to a reused id is not it"
        );
        assert_eq!(of_process(&records, 7, 0), None, "no record, no join");
    }
}
