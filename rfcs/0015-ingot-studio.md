# RFC-0015: Ingot Studio, the shell

- Status: **Accepted**
- Created: 2026-08-11
- Affects: CLI, tooling
- Closes: [GAP-025](../docs/gaps.md#gap-025)
- Introduces: `ingot-studio`, `ingot studio`, the run record

## Problem

Nine commands, each correct, each printing to a terminal, and no place that
shows a project's state at once. Answering "is this agent alright?" today means
running `check`, `doctor`, `tools` and `test`, reading three kinds of output,
and holding the result in your head. Every fact is available; none of them are
in the same place.

Two of those facts are worse than scattered. **What a run did** is not anywhere
at all after the terminal scrolls: a run writes its event stream to standard
error and then it is gone, so "what happened last time?" has no answer. And
**what this machine can reach** — which providers this build includes, which
credentials are exported, whether a container runtime is there — is spread
across `ingot doctor` output for one project at a time.

[RFC-0007](0007-the-ingot-product-loop.md) named a surface as a non-goal in a
very specific form: *"a hosted no-code workflow editor as the source of truth."*
The reason was that a surface built then would have had to invent the semantics
the language did not yet have, and those inventions would have become the real
definition of what an agent is. That reason has expired for reading and has not
expired for writing, which is what shapes this RFC.

## Goals and non-goals

**Goals.**

1. One surface showing a project's diagnostics, readiness, boundary, agents and
   run history, plus what this machine can reach.
2. Every fact in it produced by the same function the corresponding command
   calls, so a page and a terminal cannot disagree.
3. Run history that is the event stream and not a summary of it.
4. No new persistent state that could have been re-derived.

**Non-goals.**

- Editing a program. The canvas is separate work with a harder constraint, and
  nothing here prejudges it.
- Editing a manifest. See *Connections are read-only*, below.
- Starting a run. The studio shows runs; `ingot run` performs them.
- Remote access, accounts, or multi-user anything.

## Why this is not the surface RFC-0007 refused

Three differences, and the first two are structural rather than promised.

**It cannot compute.** `ingot-studio` is a crate with no dependencies and no
compiler. It holds a socket, an HTTP parser, a guard and one HTML page.
Everything it shows arrives through a single trait:

```rust
pub trait Answers: Send + Sync {
    fn answer(&self, request: &Head, body: &[u8]) -> Reply;
}
```

The implementation lives in `ingot-cli` and calls `doctor::report`,
`sandbox::plan_all`, `compile_path` and `LanguageService::check_file` — the same
functions `ingot doctor`, `ingot sandbox`, `ingot build` and the editor call. A
surface becomes a second source of truth by knowing something the tool does not;
this one has no way to know anything.

**It cannot write a program.** There is no route that modifies a `.ing` file or
an `ingot.toml`. The only bytes it writes are a bookmark file of paths and the
deletion of a run record a person asked to delete.

**It is read-mostly by construction, not by discipline.** Of eight routes, five
are `GET`, two add or remove a path from a list, and one deletes a run record.

## The honest-state problem

Docker Desktop has a daemon holding the list of containers, so its window can
show one. Ingot has no daemon and deliberately no registry: a project is a
directory, a provider is an environment variable, a run is a process that
exited. A surface over that has to decide, for each thing it shows, whether it
is remembering or re-reading — and remembering is how it becomes authoritative.

**Projects: a bookmark file, and nothing more.** `projects.json` under the
platform configuration directory (`INGOT_CONFIG_DIR` overrides it) holds an
array of paths. Every fact shown about a project — its name, version, whether it
compiles, what it may reach, how many runs it has — is read from the directory
at the moment it is asked for. Deleting the file loses bookmarks and no
information. Removing a bookmark touches nothing on disk.

**Connections: read-only.** The page shows which providers this build includes,
which environment variables each answers to, and whether each is set — by name,
never by value. It shows the `[[model.provider]]` block to write and where it
goes. It does not write it.

That is a deliberate limit rather than an unfinished feature. Writing a
`[[model.provider]]` block through a form means re-serializing a manifest a
person hand-wrote, which loses their comments and their ordering. That is
exactly the failure mode the canvas has to avoid when it edits source, and
solving it once — as a span-targeted edit rather than a regeneration — is worth
more than solving it twice, badly, first here. Until then the studio shows the
text and the person writes it.

**Runs: the one piece of genuinely new persistent state.** It has to be new,
because a run is the one thing that cannot be re-derived from a directory. It
is specified in the next section, and the shape of it is the interesting part.

## The run record

`ingot run` writes `<out-dir>/runs/<id>.jsonl` as it goes. `--no-history` opts
out. The file is JSON Lines:

```
{"agent":"Research","contained":false,"id":"1786000000-4821","provider":"anthropic","record":"started","schemaVersion":1,"startedUnix":1786000000}
{"event":"runStarted","agent":"Research","provider":"anthropic"}
{"event":"nodeStarted","node":"n1","kind":"model.call"}
...
{"event":"runFinished","steps":7,"usage":{"inputTokens":4210,"outputTokens":880}}
{"record":"finished","finishedUnix":1786000042,"ok":true,"steps":7,"usage":{...},"cost":"$0.041"}
```

**The middle of the file is the event stream verbatim.** Each line is exactly
what `ingot run --events json` prints to standard error, byte for byte, produced
by the same `RunEvent::to_json_line`. A consumer of one is a consumer of the
other, so the two cannot drift into different shapes.

**Wall-clock time lives in the `record` lines and nowhere else.**
[Runtime 0.1 §9](../specs/runtime/v0.1.md) requires a replayed run to reproduce
its event sequence byte for byte, and cassettes match by position, so an event
may not carry a clock — two runs of one artifact against one cassette have to
produce identical bytes. Time, the process id and the outcome are facts about
*this execution* rather than about the program. Putting them on lines that
carry `record` where an event carries `event` makes the distinction mechanical:
a reader selecting on `event` sees exactly what a replay would reproduce.

**Deltas are not recorded.** The live text a model produces while it is still
answering is not an event ([RFC-0013](0013-streaming.md)), and a record holding
it would be a record no replay could reproduce.

**An unfinished file means what it says.** The trailing `record` line is written
when the run ends. A file without one is a run that started and reported no
result — it may be going right now, or the process may have been killed. Nothing
guesses which. The studio shows that state under the name *no result recorded*,
and re-reads while the page is open so a run in progress fills in.

That is the honest reading of what the file can prove, and the alternative is
worse: a pid to check would be one more thing to be wrong about, and a history
that quietly reported interrupted runs as running would mislead in exactly the
situation where somebody is trying to work out what went wrong.

**Where records live.** Under the project's build directory, because a run
record is output: disposable, already ignored by version control, and expected
to disappear when the build directory does. Not under the studio's own
configuration, because then a project's history would be a fact about a machine
rather than about a project.

`ingot dev --run` keeps no record. A watch loop runs on every save, and burying
the runs somebody meant to keep under the ones they did not is not a history.

## Who can reach the studio

A loopback port is reachable by every process on the machine and, through a page
open in the same browser, by every site the person is visiting. The studio shows
project paths, environment-variable names, diagnostics and run history — one
person's working life — so the guard is three checks and a request must pass all
three.

**A session token.** Fresh per process, printed once in the URL `ingot studio`
emits, stored nowhere. The page strips it from the address bar on load so it
does not land in browser history.

**The `Host` header must be a loopback authority naming this exact port.** This
is the DNS-rebinding defence and it is the one that is easy to omit. An attacker
who points `studio.attacker.example` at `127.0.0.1` gets a page that is
same-origin with itself and can therefore read whatever it fetches from this
socket; the request arrives carrying that name, and is refused on it. The port
must match too: another local process on another port is as much a stranger as a
remote one.

**The `Origin` header, when present, must be this studio's own.** A browser
sends it when it considers a request cross-site, so a page elsewhere that
somehow learned the token still cannot read a report.

Two more things follow from the same reasoning. `Studio::start` **refuses** a
non-loopback bind rather than warning about it — a person who typed the wrong
flag would not find out otherwise. And every response carries
`Content-Security-Policy: default-src 'none'; connect-src 'self'`, so nothing
the page renders can reach off the machine even if a diagnostic somehow carried
a URL into it.

## Why a local server rather than a desktop framework

It ships inside the binary that already ships. No Node, no npm, no bundler, no
second toolchain to keep current, and no dependency tree to audit for a
component that listens on a socket — the same reasoning as `ingot-egress`. The
page is one document with its style and script inline, so nothing has to be
fetched afterwards with a token attached and the page cannot half-load.

A webview wrapper can come later without changing any of this.

## Routes

| Method | Route | Answers with |
|---|---|---|
| GET | `/api/projects` | the bookmark list, each entry read from its directory |
| POST | `/api/projects?path=` | adds a bookmark; refuses a directory with no `ingot.toml` |
| DELETE | `/api/projects?path=` | removes a bookmark |
| GET | `/api/project?path=` | diagnostics, readiness, agents, boundary |
| GET | `/api/runs?path=` | run summaries, newest first |
| GET | `/api/run?path=&id=` | one run and its events |
| DELETE | `/api/run?path=&id=` | removes one record |
| GET | `/api/machine` | providers, credentials by name, runtime, images |

A run identifier from a URL is checked against the shape an identifier has —
`<digits>-<digits>` — rather than sanitised. `..`, a separator or a drive letter
is simply not one, and there is no encoding of those that is.

## Compatibility

Additive. One new subcommand, one new flag on `ingot run`, one new crate. The
only behaviour change to an existing command is that `ingot run` now writes a
record under its output directory; `--no-history` restores the old behaviour,
and nothing reads the directory except the studio.

`doctor::inspect` was split into `report` (a value) and `inspect` (which renders
it), so the studio and the terminal show one report rather than two. The
built-in provider table moved to `run::BUILT_IN` for the same reason.

## Alternatives

**Shell out to `ingot` for each panel.** Correct by construction, and slow
enough to be unpleasant: a page load would be four process launches per project.
Calling the same functions in-process gets the same guarantee.

**Write the run record from the studio by starting runs itself.** Then only runs
started from the studio would have history, and the terminal — where people
actually work — would produce none. The record belongs to `ingot run`.

**Keep run history in the studio's configuration directory.** It would survive
`rm -rf target/`, and it would also make a project's history a fact about one
machine's UI configuration. A record is output; output goes with output.

**Poll versus server-sent events for a run in progress.** Polling a file every
two seconds while the page is visible needs no held-open connection, no second
protocol and no thread per viewer. The file is append-only and flushed per line,
so a poll is a `read_to_string`.

## Conformance tests

- `crates/ingot-studio/tests/server.rs` — the guard from the other end of a
  socket: a missing token, a guessed token, a rebinding `Host`, a cross-site
  `Origin`, and in each case that **no route was reached at all**. Plus the
  refusal to bind anywhere but loopback, and that two studios do not share a
  token.
- `crates/ingot-cli/tests/studio.rs` — the equalities. The readiness on the page
  against `ingot doctor --json`; the diagnostics against `ingot check`; the
  recorded event stream against what `--events json` printed; that no event in a
  record carries a clock; that `--no-history` writes nothing; that a hostile run
  identifier is refused; and that the machine page names `ANTHROPIC_API_KEY`
  while never reproducing its value.
- `crates/ingot-cli/src/runs.rs` — the record is verbatim at the byte level, an
  unfinished run reads as unfinished, a half-written last line does not hide the
  run, and an identifier cannot name a file outside the directory.
