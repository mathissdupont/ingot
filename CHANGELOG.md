# Changelog

All notable changes are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

Three versions move independently — the language, the Agent IR and the CLI. Each
entry states which of them it affects. See [GOVERNANCE.md](GOVERNANCE.md).

## [Unreleased]

**A model call's timeout is fixed at 180 seconds** — recorded as
[GAP-040](docs/gaps.md#gap-040), not fixed. `DEFAULT_TIMEOUT` is a `const` with
no manifest field, environment variable or flag behind it. Generous for a hosted
API, short for a model on your own machine — which is the deployment 0.5.2 had
just unblocked. The entry says where it should live and names the one decision
that has to be made first: whether an artifact may state a wall-clock ceiling at
all, when the same artifact would then finish on one machine and fail on
another.

**There is nowhere to answer an agent** — [GAP-041](docs/gaps.md#gap-041) and
[GAP-042](docs/gaps.md#gap-042), recorded together because they share one
design. An approval is answered at a terminal or not at all, so an artifact with
a gate in it cannot be run from the studio — the one surface built for people.
RFC-0015 decided that deliberately and explained why; its consequence had never
been written down. Above the channel there is no vocabulary at all: an agent
cannot put a question to a person and read the answer, which rules out
everything conversational and is a second program model rather than a missing
feature. The hard part is not the socket, it is what a replay does with an
answer a person typed.

## [0.5.2] — 2026-08-14

**`model requires { … }` could not work with a provider you declared** (runtime)

`[[model.provider]]` builds through `catalogue::build`, which called
`with_base_url`, `with_model` and `with_effort` — and not `with_catalogue`. The
three built-in providers were given one; a declared one was not. So an
operator's `[[model.catalogue]]` entries were invisible to the endpoint they
were most likely written for: their own.

The second half is the name. Resolution went through the protocol's `PROVIDER`
constant, so a provider declared as `name = "local"` looked its models up under
`openai`. `model exact "local/…"` already resolves against the declared name, so
one provider answered to two names in one manifest and only the pinned half
worked — and the refusal told you to write `model exact openai/<model>` for a
provider you had called something else.

Together these made a capability requirement unusable with every self-hosted
server and every gateway. The catalogue landed in 0.5.0 to move model facts out
of this binary and into the operator's hands; it did not reach the one place the
operator owns outright.

- A declared provider is built with the manifest's catalogue, and resolves
  against the name the operator gave it.
- The refusal now names that provider rather than its protocol.

*Found by running an agent against a local Ollama server.* Reading the code
raised the suspicion; the run is what turned it into a fact. That is the third
bug in two days whose shape is *what does somebody outside this repository
actually get*, after the conformance suite that would not have survived being
installed and the crate name that was already taken. See
[GAP-038](docs/gaps.md#gap-038).

## [0.5.1] — 2026-08-14

The release that can be installed. 0.5.0 could be downloaded and built from a
checkout; it could not be *published*, and finding out why took asking one
question on behalf of somebody who does not have this repository.

A patch release rather than a re-cut of 0.5.0: that tag exists, its archives are
published, and two different trees under one version number is exactly the
confusion a version number is for preventing. Nothing here changes the language,
the Agent IR or the artifact format.

**The conformance suite would not have survived being installed** (CLI)

`ingot-cli`'s build script embedded the suite by reaching two directories
upwards to `specs/conformance`. That works from a checkout and only from a
checkout: `cargo package` carries a crate's own files and nothing above it, so
the suite was not in the package — and the build script's `read_dir` failure was
a silent one, so a published crate would have built *successfully* and installed
a binary whose `ingot conform` had no cases.

This is precisely the shape [GAP-038](docs/gaps.md#gap-038) describes. The entry
was written on 2026-08-13 and says the friction that stops somebody else is
usually not on the list because nobody has hit it yet. The first step towards
somebody else installing this found a bug that only somebody else could have
found.

- The cases now live in **`ingot-conformance`**, a crate of their own, so
  travelling with the code is a property of where they are rather than a
  convention. `specs/conformance` is gone; [`specs/README.md`](specs/README.md)
  says where it went.
- An empty suite is a **build failure**, in the build script and again as a
  `const` assertion on the table that reached the binary. An empty suite passes
  every backend it is pointed at, which is worse than no suite.
- The drift test now compares in both directions: a file in the tree but not in
  the binary, and a file in the binary that is no longer in the tree.

**Installable from crates.io** (CLI)

`cargo install ingot-cli` — no `--git`, no clone. `ingot-mcp` and `ingot-lsp`
install the reference tool server and the language server, separately, because
they are three separate permissions to grant a machine.

- Every crate now carries the metadata a registry needs; `ingot-cli` has its own
  README and `ingot-language-service` and `ingot-lsp` had no `description`.
- Verified the way it will actually be consumed: the binary built from the
  packaged sources, run from a directory with no checkout above it, executes all
  twelve cases.

**`ingot-types` is now `ingot-lang-types`** (internal; no source change)

The name on crates.io belongs to an unrelated packet-parsing library — the same
project that holds the bare `ingot`, actively maintained, four published
versions. [GAP-019](docs/gaps.md#gap-019) had already weighed the bare name and
accepted the discovery confusion, concluding it "costs nothing"; it did not know
the same owner held this one too. This is not a preference — the name is taken.

Only the **package** name changed. `[lib] name = "ingot_types"` is kept, so
`use ingot_types::…` reads the same everywhere and not one line of Rust moved: a
package name is an address in a registry, a library name is what the code says,
and only the first one collided. A publish is all-or-nothing in practice —
crates go up in dependency order and a name taken halfway through leaves the
earlier ones permanently published — so this was worth finding before the token
was ever used, not after.

## [0.5.0] — 2026-08-13

The release where a declaration outlives the run that made it, and where the
suite that checks a backend became something you can install rather than
something you can read.

Since 0.4.0: a `verify` carries the check it names and a failed one ends the
run; the Python backend streams, so both backends accept the same answer; there
is a conformance suite, built into the binary, that has already found four real
divergences between them; an agent can keep state between runs and be stopped at
a checkpoint and continued; a tool server can be reached over a network without
`network deny` ceasing to mean what it says; and what a model can do is
configuration rather than a constant in this binary.

**Every milestone is done.** M8 was the last, and the gap register's `Unproven`
class is no longer empty as a result — see
[GAP-038](docs/gaps.md#gap-038) and [GAP-039](docs/gaps.md#gap-039). The
question stopped being whether it is built.

**A catalogue the operator owns** (closes nothing; prevents a class of
staleness)

- `[[model.catalogue]]` declares what a model provides — its context window and
  its capabilities — beside the prices and for the same reason: these facts are
  provider- and time-dependent.
- They used to be `const`s in one provider module, which meant a model growing a
  larger window was a code change and a release, and that `model requires { … }`
  worked for one vendor of three. OpenAI and Google refused it outright, saying
  they had "no catalogue to match them against".
- All three providers now resolve through one function, so they cannot answer
  the same question differently. An unknown context window does not satisfy a
  requirement, and a refusal names every candidate and what each was short of.

**A conformance suite you can install** (closes M8)

- The suite is **built into the `ingot` binary**. `ingot conform` works from any
  directory against any backend command — no clone, and no version to keep in
  step, because the cases test conformance to the specifications that binary was
  built from.
- `ingot conform --export <DIR>` writes the cases out to read or edit;
  `--suite <DIR>` runs an edited copy. Inside a checkout the tree still wins, so
  editing a case changes what runs, and a test compares the two on every build.
- Four new cases: `loop-guard`, `parallel-map`, `checkpoint` and
  `budget-exhausted`. Twelve in total, and both backends pass all of them.
- The suite's README now says precisely what is *not* covered and why: a tool
  call and a sub-agent, because the Python target implements neither; a
  run-time policy denial, because exercising it needs a hand-written artifact
  the compiler will not produce; and resumption, because the request shape
  describes one run.

**Three bugs the four new cases found**

- `parallel map` collected `null` for every element, in both backends and in
  both flagship examples. An iteration's value is read from the last body node's
  binding, and the idiom — a bare expression — lowered to a node with none.
  The interpreter now refuses such a node rather than collecting `null`.
- A loop guard read its state once, before the loop, so a guard over working
  memory never changed and only `max` ever stopped the loop.
- The two backends numbered loop iterations differently.
  [Runtime 0.1 §9.1](specs/runtime/v0.1.md) now says which: an *index* counts
  from zero, an *iteration* from one.
- And a false positive: the checker warned that the last expression of a
  `parallel map` body was a discarded value. It is the iteration's value.

The golden IR moves for both examples: a map body's last node gains its binding.

**A tool server that is not a child process** (MCP binding 0.2; closes
[GAP-007](docs/gaps.md#gap-007), opens [GAP-037](docs/gaps.md#gap-037),
specified in
[RFC-0019](rfcs/0019-a-tool-server-that-is-not-a-child-process.md))

- A `[[mcp.server]]` may carry a `url` instead of a `command`, spoken to over
  Streamable HTTP. `auth-env` names the environment variable holding a bearer
  token — a name, never a value.
- **The server's host is checked against the calling agent's own `network`
  grant, before anything connects.** `network deny` permits no remote server at
  all; there is no "except through tools". This is the check
  [ADR-0005](docs/adr/0005-mcp-over-stdio-only.md) said had to exist before the
  transport could, and the ADR now carries an amendment saying what changed and
  what did not.
- No new effect and no new policy subject. The endpoint stays in the manifest,
  so the same artifact still runs against a different deployment without a
  recompile.
- The cost, stated rather than hidden: an artifact needs a wider policy to be
  served remotely than locally, because serving a tool remotely does put its
  arguments on the network.
- `args`, `cwd`, `pass-env` and `image` are **refused** beside a `url` rather
  than ignored, so nobody can believe a credential reached a server that never
  saw it.
- `tools/call` is not retried. It is not idempotent, and a server that sent mail
  and then failed to answer must not be asked twice.
- `--sandbox` and `--contained` refuse a remote server, naming it. See
  [GAP-037](docs/gaps.md#gap-037).
- Plain `http` to anything but a loopback address warns on every run.
- Behind the CLI's `remote-tools` feature (on by default) and `ingot-mcp`'s
  `http`, so a build that hosts only local servers carries no TLS stack for it.

**A checkpoint you can stop at** (Agent IR 0.2, Runtime 0.5; closes
[GAP-008](docs/gaps.md#gap-008), opens [GAP-036](docs/gaps.md#gap-036),
specified in [RFC-0018](rfcs/0018-state-that-outlives-a-run.md))

- `ingot run --stop-at "<label>"` stops at a checkpoint and writes a snapshot;
  `ingot run --resume <FILE>` continues from it.
- **Only a checkpoint at the top level of a flow is resumable.** One inside a
  branch or a loop would need a serialised continuation, which is not a file
  anybody could read. The compiler marks each checkpoint, and `--stop-at` on a
  nested one is refused with the reason rather than silently never firing.
- A new `runStopped` event ends a stopped run. It is the only thing that
  suppresses the "every declared output was emitted" check, and it is in the
  record so a reader can see the check was suppressed.
- The events of the two halves, framing removed, concatenate to exactly the
  events of one uninterrupted run — byte for byte. That is
  [Runtime 0.5 §2.5](specs/runtime/v0.5.md) and an executable test.
- The counters carry across a stop, so stopping is not a way to spend twice what
  the artifact permits. So does the cassette position, so a replayed second half
  picks up where the first stopped.
- An artifact that changed since the run stopped is refused, with no override.
  So are inputs that differ from the ones the snapshot carries.
- A top-level `checkpoint` node gains `"resumable": true`, so an artifact that
  has one **does** change bytes. It is the only such change here, and the golden
  IR for the research example records it.
- The generated Python backend does not resume, and its portability report says
  why: a straight-line program has no node walker to re-enter. See
  [GAP-036](docs/gaps.md#gap-036).

**Persistent memory** (language 0.2, Agent IR 0.2, Runtime 0.5; closes
[GAP-014](docs/gaps.md#gap-014), opens [GAP-035](docs/gaps.md#gap-035),
specified in [RFC-0018](rfcs/0018-state-that-outlives-a-run.md))

- An agent may declare state that outlives the run:
  `memory { persistent { seen: string[] = [], visits: int = 0 } }`.
- Persistent fields are addressed by `memory.`, ephemeral ones stay on `state.`.
  Two roots rather than one, so a write that outlives the run does not look like
  a write to a scratchpad.
- Every persistent field declares a **literal** initial value. That removes
  "read before written" from persistent memory rather than making every author
  guard against the first run at every read site.
- The store is `<out-dir>/memory/<agent>.json`, relocated with `--memory FILE`
  and skipped with `--no-memory`. Every run that opens one says so.
- A store records the declaration it was written under, **in full**. A changed
  declaration is refused with a per-field diff; `--migrate-memory` keeps what
  still matches, drops the rest, and reports the loss even under
  `--events quiet`.
- Both backends read and write the same format, and the `memory-initial`
  conformance case holds them to the same seeding behaviour.
- Two runs sharing one store are **not** made safe. Stated in
  [GAP-035](docs/gaps.md#gap-035) rather than implied away.
- `--no-history` no longer suppresses the store. Where an agent keeps what it
  remembers and whether this run is written down are different questions.

**A `verify` that runs** (language 0.2, Agent IR 0.2, Runtime 0.4; closes
[GAP-030](docs/gaps.md#gap-030), opens [GAP-034](docs/gaps.md#gap-034),
specified in [RFC-0017](rfcs/0017-a-verifier-that-runs.md))

- A verifier may carry its check:
  `verifier MinSources(d: draft, min: int) = len(d.sources) >= min`. The body is
  a pure `bool` expression over the parameters, inlined at each `verify` site as
  the node's `condition` — the field `branch` already carries, so the IR schema
  is unchanged and both backends already evaluate that value form.
- A failed check emits its `verified: failed` event and **then** ends the run,
  naming the verifier. Artifacts emitted earlier stay in the record; nothing
  after the failing node runs.
- `ING2020` rejects a body that produces a value instead of deciding.
  `ING6007` warns when a `verify` comes after the `emit` of what it checks,
  where the check could not have prevented anything.
- `ING6006` narrows to the only case left that nothing can carry out: a verifier
  declared without a body. Such a declaration keeps its meaning and its
  `notPerformed` outcome, so no existing program changes.
- The rule a later verifier-with-reach has to satisfy is now written down: a
  `verified` outcome must be derivable from the run record alone.

**A conformance suite somebody else can run** (new command `ingot conform`;
closes [GAP-017](docs/gaps.md#gap-017))

- `ingot conform --backend "<your command>"` runs seven cases against any
  backend. A backend under test is a command: the suite writes a request file,
  runs the command with it, and compares the event stream, the artifacts and
  the outcome against what the case requires.
- Each case names the specification clause it enforces, so a failure says what
  to read rather than only that something differs. `--list` prints them.
- The reference interpreter is **not privileged**: it reaches the suite through
  the same adapter a third party writes. Both shipped backends are held to the
  same cases by the same code.
- [A backend author's guide](docs/guide/writing-a-backend.md), and a worked
  adapter in forty lines.
- `ingot build --target python --from-ir <document>` builds from an Agent IR
  document nothing local compiled. A backend consumes Agent IR, so it had to be
  possible to hand one an artifact somebody else built.
- Fixed, and found by the suite on its first run across both backends: the
  reference interpreter wrote `response_type` where the specification and the
  Python backend both said `responseType`. On an enum, serde's `rename_all`
  renames the variants and not their fields.
- Fixed: `TempDir` in the test support derived its name from the clock alone.
  `as_nanos` reports whatever resolution the platform has, and on macOS that is
  microseconds — so two tests starting in the same microsecond shared a
  directory and overwrote each other's fixtures.

**The Python backend streams** (closes [GAP-032](docs/gaps.md#gap-032))

- Both providers in the generated program read a `text/event-stream`, so an
  answer between 16,000 and 64,000 output tokens no longer completes on one
  backend and fails on the other.
- The output ceiling is asked of the provider instead of written into the
  generated program. Runtime 0.3 §4 forbids an artifact selecting its own
  ceiling, and the emitter used to put one in every `ask`.
- **One parser, two transports**: each accumulator rebuilds the payload a
  whole-body call would have returned and hands it to the same reader, so the
  two transports produce identical values *and* identical errors by
  construction rather than by testing.
- Cassette replay reports that it does not stream. A recording produces its
  answer at once, and inventing fragments would make a replayed run
  indistinguishable from a call that never happened.
- Fixed: the Python target's build report still explained its refusal of
  `verify` with `passed: true` and GAP-002, which Runtime 0.2 had already fixed.

## [0.4.0] — 2026-08-11

The release that makes the `policy` block true and gives the whole loop one
surface. Since 0.4.0-rc.2: a model's answer streams as it is written, Gemini
joins Anthropic and OpenAI, a tool can declare how far its capability reaches
and the compiler checks it, a network allowlist is enforced by a real proxy on a
network with no other route out, and `ingot studio` shows a project's
diagnostics, readiness, boundary and run history in one page.

**One surface over the whole loop** (new crate `ingot-studio`, new command
`ingot studio`; closes [GAP-025](docs/gaps.md#gap-025), specified in
[RFC-0015](rfcs/0015-ingot-studio.md))

- `ingot studio` serves one page on the loopback interface: your projects, and
  for each one its diagnostics, its readiness, the boundary each tool server
  would get, the agents it declares and the runs it has had — plus what this
  machine can reach. Nine commands' worth of output, in one place.
- **It computes nothing.** `ingot-studio` has no dependencies and no compiler;
  it is a socket, a guard and a page. Everything reaches it through one trait
  the CLI implements by calling `doctor::report`, `sandbox::plan_all`,
  `compile_path` and the language service — the same functions the subcommands
  and the editor call. That is what makes it not the second source of truth
  [RFC-0007](rfcs/0007-the-ingot-product-loop.md) refused: a crate with no way
  to compute cannot become one. The tests are equalities against
  `ingot doctor --json` and `ingot check`, not assertions about the page.
- **The project list is a bookmark file and nothing more.** It holds paths;
  every fact about a project is read from the project when it is asked for.
  Deleting the file loses bookmarks and no information, and removing a bookmark
  touches nothing on disk. `INGOT_CONFIG_DIR` says where it lives.
- **Connections are read-only, on purpose.** The page shows which providers this
  build includes, which variables each answers to and whether each is set — by
  name, never by value — and shows the `[[model.provider]]` block to write. It
  does not write it: re-serializing a hand-written manifest loses its comments,
  which is the same mistake as regenerating source from a diagram, and solving
  it properly belongs to the editing work rather than here.
- **Who can reach it.** A session token, fresh per process and stored nowhere; a
  `Host` header that must be a loopback authority naming this exact port, which
  is what stops a name that merely resolves to `127.0.0.1` from being treated as
  local; and a same-origin `Origin` when a browser sends one. A non-loopback
  bind is **refused**, not warned about. Every reply carries
  `default-src 'none'; connect-src 'self'`, so nothing the page renders can
  reach off the machine.
- **A run can be started from the page**, and the start panel offers the agents
  the artifact declares with a field per input it takes — the artifact's own
  signature rather than a guess at one. The studio spawns the same command a
  person would type; it interprets nothing itself.
- A *launch* is not a run. A record only exists once the interpreter reaches
  `runStarted`, so a child that fails while compiling writes none — and a button
  that appears to have done nothing is worse than an error. The launch carries
  the process id, the exit status and what the child printed, and is joined to
  its record by the pid the record's identifier ends in.
- Two things the page cannot ask for: `--yes`, which would turn an effect that
  needs a person into one that does not, and `--no-history`, which would produce
  a run the studio could never show again. Neither is a field, and an unknown
  field is refused rather than ignored. The child gets no terminal on its
  standard input, so an effect needing approval is denied, not assumed.
- No Node, no npm, no bundler. It ships in the binary that already ships.

**A run writes itself down** (new; `ingot run` behaviour change)

- `ingot run` now writes `<out-dir>/runs/<id>.jsonl`. The middle of the file is
  the JSON event stream **verbatim** — the same bytes `--events json` prints,
  from the same `to_json_line` — wrapped in two lines carrying `record` where an
  event carries `event`.
- **The clock lives on those two lines and nowhere else.** Wall-clock time, the
  process id and the outcome are facts about one execution;
  [Runtime 0.1 §9](specs/runtime/v0.1.md) requires a replay to reproduce the
  event sequence byte for byte, so an event may not carry a clock. A reader
  selecting on `event` sees exactly what a replay would reproduce, and a test
  asserts no event carries one.
- Deltas are not recorded: the live text a model produces is not an event, and a
  record holding it would be a record no replay could reproduce.
- A file with no closing line is a run that started and reported no result. It
  may be going or it may have been killed; nothing guesses, and the studio shows
  it under that name rather than claiming it is running.
- `--no-history` writes nothing. `ingot dev --run` keeps no record — a watch
  loop would bury the runs you meant to keep.

**Less to type for the thing you do first**

- `--provider replay` finds the project's cassette when there is exactly one, so
  replaying your own fixture no longer means typing its path. Two is a real
  question — which recording did you mean? — and it is asked rather than
  guessed, with both named so the answer is a copy. None says how to record one.
- `ingot init` prints the commands that follow, says plainly that **none of them
  need an API key**, and ends with the run that produces something to read. A
  test executes every command it prints: printed instructions that do not work
  are worse than none.

**Fixed**

- `ingot run --sandbox` reported "no container runtime found" on a machine
  without one, hiding the refusal it should have led with. The boundary is
  settled from the artifact before the environment gets a say, so a policy no
  boundary can keep is now reported as that on any machine, and the runtime is
  required at the point a server actually has to go inside one. This is also
  what made two tests fail on the macOS and Windows CI runners.
- `cargo doc -D warnings` failed on four links from public module documentation
  to private items in the `openai` and `google` providers, and on an unresolved
  `EgressBoundary` link in the CLI. The documentation job has been red since the
  streaming release.

**Internal** (no behaviour change)

- `doctor::inspect` split into `report`, which returns the value, and `inspect`,
  which renders it. The studio shows that report rather than one of its own.
- The built-in provider table moved to `run::BUILT_IN`, so `ingot doctor` and
  `ingot studio` cannot disagree about which vendors exist.

**A network allowlist is enforced** (new crate `ingot-egress`; closes
[GAP-001](docs/gaps.md#gap-001))

- `network allow ["arxiv.org"]` now bounds a contained tool server to that host.
  A request anywhere else is refused from inside the box, and a container test
  makes the request and watches it fail.
- The arrangement: the server joins a container network created `--internal`,
  which has no route out. The proxy joins that network and an ordinary one, so
  it is the only thing on the internal side that can reach anything.
  `HTTP_PROXY` points the server at it.
- **The enforcement is the network, not the variable.** A server that reads
  `HTTP_PROXY` goes through the filter; a server that ignores it reaches
  nothing, because there is nowhere for its packets to go. A third container
  test unsets every proxy variable inside the box and the request still fails —
  the boundary does not depend on the contained process cooperating.
- `ingot run --sandbox` builds the arrangement when the proxy image is present,
  and reports the allowlist as unenforceable when it is not. The plan is made
  after asking, so what it says and what happens cannot drift.
- One proxy serves a whole run, so its list is the union of every agent's grant.
  Each agent is still bounded to its own by the compile-time check from
  [GAP-013](docs/gaps.md#gap-013) — the boundary bounds the run, the compiler
  bounds each agent inside it.
- **Nothing in the register sits in the Unenforced class now.** That is the
  class that could mislead, and it is empty.

**The egress filter** (part of the above)

- A forward proxy that decides on the host: `CONNECT` for TLS, an absolute-URI
  request line for plain HTTP, and a `403` naming the reason for anything the
  policy does not grant. No TLS is terminated and no certificate authority is
  involved. Runnable as `ingot egress --allow arxiv.org`, which prints every
  decision — allowed and refused alike, because a filter that only reported what
  it stopped would leave you unable to tell "nothing was blocked" from "nothing
  was tried".
- Each failure mode the register named is closed, and each has a test that
  connects a real socket:
  - **DNS rebinding.** The client never resolves anything. It hands over a name;
    the proxy resolves once and dials one of *those* addresses. The check and
    the connection cannot disagree, because there is one resolution.
  - **Address literals.** `CONNECT 93.184.216.34:443` is refused as its own kind
    of refusal rather than falling through to "not listed" — a policy grants
    names, and the log should say which mistake was made.
  - **TLS SNI against the Host header.** Neither is read. A `CONNECT` tunnel
    carries bytes to the address the proxy dialled, so whatever the client
    writes afterwards goes to a granted host. For plain HTTP the request target
    decides and `Host` is ignored: two sources for one fact is how a filter and
    a destination come to disagree.
  - **A granted name pointing inward.** Every resolved address is checked
    against loopback, link-local, private and carrier-grade ranges — not just
    the one that gets used. `169.254.169.254` is in that list by name.
- No dependencies, deliberately. This is the component a sandbox is trusted to
  be right about, and every crate it pulls in is a crate that has to be right
  too.

**A capability has a reach** (Language 0.2; closes
[GAP-013](docs/gaps.md#gap-013); narrows [GAP-001](docs/gaps.md#gap-001),
[GAP-007](docs/gaps.md#gap-007))

- A tool declares where it goes, and the compiler checks it against what the
  agent granted:

  ```ingot
  tool web.search(query: string) -> search_result[] !network("arxiv.org")

  policy { network allow ["arxiv.org"] }
  ```

  Reaching a host the policy does not grant is `ING4009`, which names both the
  declaration and the grant: the two halves of the mistake are in different
  places and either one may be the wrong one.
- **What a policy value became.** Before this, adding a host to
  `network allow [...]` changed nothing a compiler could see, and removing one
  changed nothing either. The list was a claim with no reader. It has one now.
- Both sides state it on purpose. A scope written only in the policy would have
  left the compiler with one statement and nothing to compare it against;
  containment needs two. A scope written at the call site was rejected for
  scattering security decisions through the flow and for making a backend
  discover mid-run that it cannot honour something.
- A reach uses the vocabulary a policy of that subject already uses — a host for
  `network`, a workspace-relative path for the filesystem pair. An effect that
  names no resource, an empty `!network()`, a wildcard, a URL, or a path that
  leaves the workspace are all refused (`ING4010`) rather than narrowed: a value
  that reads like a constraint and is not one is the failure this syntax exists
  to end.
- **A declared reach is not advisory.** A policy's value list has always been
  advisory where nothing enforces it, and Ingot says so. `!network("arxiv.org")`
  says this tool *must* be bounded to that host, so a run that cannot keep it
  refuses before starting anything — before a tool server, before a model call,
  before the cassette is even opened. `--allow-unenforced-scopes` proceeds and
  names every declaration it is proceeding without.
- Nothing bounds egress to a host yet (GAP-001), so a declared `network` reach
  refuses everywhere today. A declared filesystem reach is kept under
  `--sandbox` and `--contained`, whose mounts come from the policy the compiler
  proved contains it.
- Opt-in throughout. An effect without parentheses keeps its meaning, so source
  written before this compiles unchanged and produces byte-identical IR — no
  package digest moves and no cassette is invalidated.

**A third protocol: Gemini** (CLI; opens [GAP-033](docs/gaps.md#gap-033))

- `kind = "google"` and `--provider google` reach Google's Generative Language
  API. `GEMINI_API_KEY` or `GOOGLE_API_KEY`; `auto` routes `google/…` to it the
  way it already routes the other two.
- It is here for one reason: Gemini is the vendor that cannot be reached by
  pretending to be something else. `kind = "openai"` already reaches Ollama,
  vLLM, llama.cpp, LM Studio, Azure, Groq, Together, OpenRouter, Fireworks and
  DeepSeek with nothing but a `base-url`, and `kind` has always named a
  protocol rather than a company. A fourth protocol earns its place the same
  way this one did.
- Three differences from the other two, each of them load-bearing. The model
  and the method are **path segments**, so `base-url` for this protocol is the
  API base — a base that already names a method is refused rather than
  concatenated into a 404. The key travels in the `x-goog-api-key` **header**:
  the API also accepts `?key=`, and a URL reaches proxy logs and crash reports
  in a way a header does not. Structured output takes an **OpenAPI subset**
  rather than JSON Schema, so `ask<T>` is translated — and a type with no
  faithful translation is refused, because a schema quietly stripped of a
  constraint is one the caller believes in and does not have.
- It streams, so a Gemini call gets the 64,000-token ceiling and shows its text
  as it arrives, through the same contract the other two use.
- `--effort` is refused on this protocol rather than guessed at (GAP-033): the
  thinking control differs per model generation, and sending nothing while
  saying nothing would let an operator believe a flag took effect.
- The provider `cfg` gates are now one umbrella feature per crate rather than
  a list that grows with every protocol.

**An answer arrives as it is written** (Runtime 0.3;
closes [GAP-005](docs/gaps.md#gap-005); opens
[GAP-031](docs/gaps.md#gap-031), [GAP-032](docs/gaps.md#gap-032))

- A provider may now deliver a completion incrementally, and `ingot run` prints
  the text as it arrives instead of showing nothing until the answer is whole.
  Both the Anthropic and the OpenAI-compatible providers stream.
- A streamed call may ask for up to 64,000 output tokens instead of 16,000. The
  ceiling belongs to the transport, not to the artifact: a service that composes
  a whole body before sending it holds the connection open for the length of the
  answer, and several refuse a larger `max_tokens` unless the request streams.
  The interpreter picks the ceiling by asking the provider whether it streams.
- **A delta is not an event.** The live text travels on a second channel that is
  not the event stream, is not recorded in a cassette, and carries no
  determinism guarantee. Putting fragments into the recorded stream would have
  broken [Runtime 0.1 §9](specs/runtime/v0.1.md) and cassette position matching.
  A replay emits no deltas, which is correct — there is nothing live to watch.
- **A partial answer is not an answer.** The value a run uses is always
  assembled from the finished response and validated whole, by the same code
  path a whole-body response takes. On a truncation or a type mismatch the
  accumulated text is discarded and the run fails exactly as before. A watcher
  that was shown that text is told it was discarded, so a half-finished answer
  is not left on screen looking like a result.
- Each provider accumulates its stream into the shape its whole-body response
  has and hands that to the same parser — one parser, two transports, so the two
  paths cannot drift into producing different answers or different errors.
- A stream that fails part-way is not retried. The caller has already shown that
  text to somebody, and a second attempt would repeat it from the beginning.
- Additive throughout: both provider methods and both sink methods are
  defaulted, so a backend written against Runtime 0.2 satisfies 0.3 unchanged.
- Not done, and recorded rather than glossed: a contained run does not stream
  and keeps the 16k ceiling (GAP-031), and the Python backend does not stream,
  so the two backends accept different answer lengths (GAP-032).

**The register says what is true**
(closes [GAP-022](docs/gaps.md#gap-022); narrows
[GAP-011](docs/gaps.md#gap-011), [GAP-012](docs/gaps.md#gap-012),
[GAP-025](docs/gaps.md#gap-025))

- GAP-022 is closed: 0.4.0-rc.2 publishes `ingot`, `ingot-mcp-fs` and
  `ingot-lsp` for Linux, Windows and macOS on both architectures, with one
  `SHA256SUMS`. Trying Ingot no longer starts with a Rust toolchain.
- GAP-011 no longer claims there is no `import`; Language 0.2 landed one. What
  remains is package semantics above it — wildcards, re-exports, agent imports,
  and an identity that means something outside one directory.
- GAP-012 no longer claims optionals, unions and functions are missing; all
  three landed. Only generics remain, deferred by decision rather than omission.
- GAP-025 no longer claims tool and safe-run guidance are missing; every work
  package it named has landed and RFC-0007's conformance list is complete. What
  is left is that the loop is nine correct commands with no single place that
  shows a project's state at once.
- Added `editor_and_cli_diagnostics_are_identical`, the last unticked test in
  RFC-0007: the editor and the command line must not grow two answers to "is
  this source correct".

**A `cost` budget is charged, or says it was not** (Runtime 0.2;
closes [GAP-003](docs/gaps.md#gap-003))

- `budget { cost <= 5 usd }` is now enforced, against prices the project
  supplies per model in `[[model.price]]`. Exceeding it ends the run the way
  `steps` and `tokens` do, and `ingot run` and `ingot test` print what a run
  cost.
- Prices are deployment configuration, not part of the artifact. A price is
  provider- and time-dependent, so an artifact carrying one would be stale the
  moment it was published — and so would a table compiled into this binary.
- The model is matched **exactly** as the provider reports it. A prefix rule
  would price `claude-opus-5-mini` at `claude-opus-5`'s rate, and a wrong price
  is worse than none.
- A budget is enforced only against a total that missed nothing. A call that
  could not be priced leaves the budget unenforced and the run names the model
  and why; `ingot check` warns with `ING5007` when the project configures no
  price at all. Silence was the old behaviour and it was a way of pretending.
- No cost calculation touches a float. Accumulation is in millionths of a
  currency unit as integers, the same six-digit precision Agent IR renders a
  `Cost` with, so a total is exact and identical on every platform. An amount
  finer than that is refused rather than rounded.
- Currencies are never converted. A rate is a second time-dependent input, and
  guessing one would make a budget mean something the operator did not write.

## [0.4.0-rc.2] — 2026-08-09

**Fixed: the first candidate produced no archives.** `x86_64-apple-darwin`
asked for the `macos-13` runner image, which GitHub has retired. A job asking
for a retired image does not fail — it queues indefinitely — so rc.1 built
Linux, Windows and Apple silicon and then waited for a runner that was never
coming. The Intel binary is now cross-compiled from Apple silicon and still
executed before it ships, which is the property that job exists to hold.

- Language version: **0.2** (unchanged)
- Agent IR version: **0.2** (unchanged)
- Runtime version: **0.2** (cassettes now record tool calls)
- Ingot Package version: **0.1** (unchanged)
- CLI version: **0.4.0-rc.2**

**A tool-using agent can be tested offline** (cassette 0.2;
closes [GAP-006](docs/gaps.md#gap-006))

- A cassette now records tool invocations and their results alongside the model
  exchanges, keyed by a digest of the invocation the way model requests already
  were. `ingot run --record` captures them and `ingot test` serves them, so an
  agent that calls tools is testable with no server started, nothing reached and
  no key exported.
- A recorded **failure** is replayed as a failure. How an agent behaves when a
  tool fails is the behaviour most worth having a test for, and a format that
  could only hold successes would be a format for the happy path.
- A call whose arguments changed since recording is refused rather than answered
  from the wrong row, the same rule model requests already followed.
- A replayed tool call does **not** perform the effect. An agent whose recorded
  run wrote a file receives the handle that write produced and leaves the
  filesystem alone: `ingot test` proves what an agent does *with* an answer, not
  that the tool still gives it.
- Cassette 0.2 is a superset of 0.1, so existing recordings keep replaying and
  only re-recording moves one. A document that states `0.1` and carries
  `toolCalls` is refused.
- `ToolInvocation` gained `node`, the IR node that made the call, mirroring
  `CompletionRequest`.

## [0.4.0-rc.1] — 2026-08-09

**The first tagged release, and it published nothing.** The version headings
below it record development milestones that were never tagged; this one was
tagged and then queued forever on a retired runner image, so no archive came out
of it either. Fixed in 0.4.0-rc.2 — which is what a candidate is for.

- Language version: **0.2** (imports, optionals and unions, pure helper functions)
- Agent IR version: **0.2** (portable node source spans)
- Runtime version: **0.2** (a three-state `verify` outcome)
- Ingot Package version: **0.1** (new)
- CLI version: **0.4.0-rc.1**

**A wedged contained run is ended rather than waited on**
(closes [GAP-024](docs/gaps.md#gap-024))

- The supervisor reads the guest on its own thread and waits on a channel, so a
  wait can end. A guest that stops responding is killed and reported; the
  container is `--rm`, so nothing is left behind.
- Two deadlines, because the two silences differ: 60 seconds from spawn to the
  first `config` call, for a guest that may not exist yet, and an idle deadline
  between two lines from one that has started.
- Every line resets the idle deadline, including `event` notifications. A guest
  running a long flow is not silent — it narrates — so the bound only has to
  cover the gap between two steps rather than the length of a run.
- The idle deadline is **derived** from `[mcp] timeout-seconds`, the bound the
  guest's own tool host already honours: `max(120s, timeout × 2 + 60s)`. A
  project that raised its tool timeout does not have to remember a second number.
- `[run] timeout-seconds` and `--timeout` override it. `--timeout 0` waits
  indefinitely, which is a deliberate choice rather than a default.

**A verification that never ran no longer says it passed** (Runtime 0.2;
closes [GAP-002](docs/gaps.md#gap-002))

- The `verified` event carries `outcome` — `notPerformed`, `passed` or `failed`
  — and `passed` is removed. A boolean can describe two states and there are
  three; leaving the old field in place beside a new `performed` flag would have
  kept the misleading reading available.
- `notPerformed` is not a failure. A consumer deciding whether a run met its
  declared properties must treat it as unknown.
- `ingot check` now warns with `ING6006` when a flow names a verifier nothing in
  the toolchain can perform, so the gap is visible before the run rather than in
  an event stream after it. A warning rather than an error: the declaration is
  correct and keeps its meaning when verifiers gain an execution model.
- Added [Runtime 0.2](specs/runtime/v0.2.md). This is a breaking change to a
  documented event stream, taken at the only point where it is free — nothing has
  been released, so no consumer of Runtime 0.1's `verified` event exists outside
  this repository.
- What this does not do is make verifiers executable. That half is now
  [GAP-030](docs/gaps.md#gap-030) and needs an RFC: a verifier is a tool call, a
  model call with a rubric, or host-provided code, and those have different
  security stories.

**The name is a description, not a claim**
(closes [GAP-019](docs/gaps.md#gap-019))

- Ingot claims no rights in its name and is not seeking a trademark. The
  clearance that release was waiting on is closed by a decision rather than by a
  search: the registry question is a checkable fact with no conflict — `ingot` on
  crates.io is an unrelated library that publishes no binary — and the trademark
  question is a rename risk that is knowingly accepted.
- This unblocks [GAP-022](docs/gaps.md#gap-022): what remains before a first
  release is choosing the moment, not a decision.

**Packaging: the checked artifact, made movable**
([RFC-0012](rfcs/0012-the-ingot-package.md),
[#10](https://github.com/mathissdupont/ingot/issues/10);
closes [GAP-004](docs/gaps.md#gap-004), [GAP-015](docs/gaps.md#gap-015))

- Added `ingot package`, which writes the compiled project as a standard OCI
  image layout holding one artifact manifest. Existing clients move it — there is
  no Ingot registry and no Ingot transport — and the printed digest is `sha256`
  of the manifest.
- The packaged Agent IR layers are the bytes `ingot build` wrote, carried
  verbatim rather than re-encoded, so the artifact that is distributed is the one
  that was tested.
- The package digest is reproducible: no timestamp, no build-machine path, no
  compression, and one canonical JSON encoding for every document it generates.
  The same inputs produce the same digest on Linux, macOS and Windows.
- Added `ingot.lock`, written into the project and carried in the package. It
  records identity rather than content: source and agent digests, the compiler
  version, and declared tool servers and model services by name. No field in it
  can hold an environment value.
- Source text does not travel in a package; its digest does. Cassettes,
  credentials, absolute paths and editor state are excluded by specification and
  by test.
- Added `ingot package --verify`, which recompiles the project and names every
  blob, source, agent and metadata field that moved since the package was
  written. It repairs nothing.
- Added `--report python`, which carries a target's portability report in the
  package. Omitted, a package makes no portability claim at all.
- Added the build-time secret scan. `ingot build` and `ingot package` scan
  source, the compiled IR and every cassette for credential-shaped **values** and
  refuse rather than warn, naming the file, the line and the shape but never the
  value. It is the same scanner that guards model-assisted authoring: a generator
  must not be able to write what the packager would refuse.
- A contained-run image reference may now be digest-pinned
  (`ingot/run@sha256:…`), and a run refuses when the image present locally is not
  the one named. Acquisition stays manual: automatic pulling waits for a
  signature scheme and a trust root ([GAP-029](docs/gaps.md#gap-029)).
- Added the normative [Ingot Package 0.1](specs/image/v0.1.md) specification.
  `lockVersion`, `configVersion` and the media-type versions move independently
  of the language, the Agent IR and the CLI.

**Model-assisted authoring guardrails**
([RFC-0007](rfcs/0007-the-ingot-product-loop.md),
[#9](https://github.com/mathissdupont/ingot/issues/9))

- Added the first `ingot new` authoring review surface:
  `ingot new --previous OLD.ing --candidate PROPOSED.ing` compares a proposed
  source repair against the previous source and separates new policy rules from
  ordinary compiler repair.
- Added a bounded offline repair loop for reviewed candidates:
  `--repair-candidate PATH` supplies compiler-repair proposals, `--max-repairs`
  caps how many are consumed, and a failed ceiling leaves the last source plus
  compiler diagnostics visible for manual continuation.
- `ingot new --out-dir DIR WORKFLOW...` now creates ordinary project files from
  a workflow description using maintained offline templates, including source,
  manifest, README, example inputs and replay cassettes that continue to
  check, build and test without an authoring model.
- Automatic repair is blocked when a candidate adds policy. The candidate can
  still explain the request, but policy acceptance remains an explicit operator
  decision.
- `ingot new --provider auto|anthropic|openai|replay` hands authoring to a real
  model provider. The model writes source, the compiler verifies it, and the
  diagnostics of a failed attempt become the next prompt. `--cassette` replays a
  recorded authoring session and `--record` writes one — including a session
  that ended in a refusal. Without `--provider`, nothing reaches a model and the
  maintained templates are still what `ingot new` writes.
- `ingot new --project DIR WORKFLOW...` proposes a change to an existing project
  as a unified diff of its entry source and writes nothing. `--apply` is the
  separate step that writes it. The candidate review path prints a diff too,
  rather than a wall of accepted source.
- Policy comparison is now per grant rather than per rule: only an `allow` that
  widens what the previous source granted is a proposal. Narrowing — a removed
  host, a `deny`, a `require approval` — asks for nothing and is no longer
  listed. Replacing `allow ["host"]` with a bare `allow` is a proposal, because
  it grants more without adding a string. `--accept-policy` accepts the listed
  grants in a second, explicit run and still prints every grant it carried.
- Authored source is checked against the tools the project actually routes,
  discovered through the same MCP path `ingot tools` uses. A `tool` declaration
  nothing can serve is a repairable diagnostic rather than a file that compiles
  and fails at run time.
- A credential-shaped value in proposed source ends the authoring loop: nothing
  is written and the source is not sent back to the model. The report names the
  shape and the line, never the value. The same scan runs on the workflow
  description before it can reach a prompt, a manifest or a terminal history.
- A model-authored project is deliberately given no cassette
  ([GAP-028](docs/gaps.md#gap-028)). A recorded answer no model produced would be
  a test that proves nothing, so the generated README carries the one
  `ingot run --record` command that creates a real one, with example input files
  written for the agent's prose inputs.

**Agent IR 0.2 source provenance**
([RFC-0007](rfcs/0007-the-ingot-product-loop.md),
[#11](https://github.com/mathissdupont/ingot/issues/11);
closes [GAP-027](docs/gaps.md#gap-027))

- Agent IR now emits `"irVersion": "0.2"` and supports optional per-node
  `sourceSpan` metadata with a project-relative source id plus UTF-8 byte
  offsets.
- The compiler preserves source spans through lowering for executable nodes,
  including `state.read` nodes created from state references and approval nodes
  inserted before gated calls.
- Human text traces resolve `sourceSpan` to local file/line/column ranges when
  the project source is available, and fall back to portable byte ranges when it
  is not.
- Source provenance is descriptive only: runtime event JSON, execution
  semantics, canonical node ids, policy, budgets and cassette request digests
  are unchanged. IR without `sourceSpan` remains valid.

**Language 0.2 reuse foundation**
([RFC-0007](rfcs/0007-the-ingot-product-loop.md),
[#7](https://github.com/mathissdupont/ingot/issues/7))

- `language 0.2` now supports project-local `import` blocks for shared `type`,
  `tool` and `verifier` declarations. Imports are compile-time source structure:
  they are resolved before semantic analysis and erased before Agent IR
  lowering.
- Optional and union type expressions are available as `T?` and `A | B`, with
  conservative assignability and canonical Agent IR type text.
- Expression-only pure helper functions can be declared with
  `fn name(params) -> type = expression` and called from flow expressions.
  Helpers type-check like ordinary calls, must remain effect-free, and lower by
  inlining into pure IR values rather than adding runtime function calls.
- Generics are intentionally deferred by RFC-0011 until at least three real
  `.ing` source examples show the same reusable type or helper pattern that
  imports, optionals, unions and pure helpers cannot express cleanly.

**Editor-neutral language service foundation**
([RFC-0007](rfcs/0007-the-ingot-product-loop.md),
[#6](https://github.com/mathissdupont/ingot/issues/6))

- New crate `ingot-language-service` exposes compiler-backed diagnostics and
  canonical formatting as editor-neutral structured data. Future LSP and editor
  adapters consume this crate instead of reimplementing parser or semantic
  logic.
- Editor diagnostics preserve the compiler's stable code, message, primary and
  secondary labels, notes, help text and original byte spans, while also
  projecting ranges to zero-based UTF-16 positions for LSP-style consumers.
- Formatting returns a full-document text edit from the same canonical printer
  used by `ingot fmt`.
- The required M7 test `editor_and_cli_diagnostics_are_identical` now pins code,
  primary byte span and message equality between the language service and CLI
  compiler path. Reference examples are also checked through the language
  service surface.
- New binary crate `ingot-lsp` adds the first stdio language server. It
  advertises full document sync, UTF-16 positions and document formatting,
  publishes diagnostics on `didOpen` and `didChange`, and answers
  `textDocument/formatting` from the language service.
- LSP diagnostics carry stable Ingot codes in the standard diagnostic `code`
  field and preserve byte-span data in `Diagnostic.data`, so editor display can
  be compared back to compiler spans.
- The language service now exposes completion, hover and definition data from
  the parsed program: keywords, built-in types and functions, policy/model
  vocabulary, top-level declarations, local flow bindings and doc comments.
- `ingot-lsp` advertises and serves `textDocument/completion`,
  `textDocument/hover` and `textDocument/definition` from that shared service,
  with tests covering declared symbols and reference examples through the LSP
  surface.
- A reference VS Code extension under `editors/vscode` contributes `.ing`
  language detection, TextMate syntax highlighting, bracket/comment rules and
  an LSP launcher configurable through `ingot.lsp.path`.

**Typed MCP discovery and preflight foundation**
([RFC-0007](rfcs/0007-the-ingot-product-loop.md),
[#8](https://github.com/mathissdupont/ingot/issues/8))

- `ingot tools --json` now exposes a versioned, machine-readable inventory of
  server identity, routes, input/output schemas, source signatures and required
  environment-variable names without exposing their values.
- Live preflight compares checked `.ing` parameters with MCP input schemas.
  Missing required parameters, rejected parameters and incompatible basic
  types are blocking drift; unsupported unions, references and untyped schemas
  remain explicitly unverified.
- Human `ingot tools` output reports the same schema status and issue codes, so
  CI, editors and operators share one readiness decision.
- `ingot tools --propose` renders editable typed declarations for undeclared
  published tools and manifest aliases for uniquely matching unresolved tools.
  Proposals never write files or guess effects: source snippets retain a
  blocking `!TODO_EFFECT` until the operator reviews the tool.

**A version-matched contained-run image path**
([RFC-0007](rfcs/0007-the-ingot-product-loop.md),
[#5](https://github.com/mathissdupont/ingot/issues/5); closes
[GAP-026](docs/gaps.md#gap-026))

- `ingot image build [SOURCE]` finds the Ingot checkout, refuses a source/binary
  version mismatch, and builds the auditable reference Dockerfile as the exact
  `ingot/run:<cli-version>` tag.
- `ingot run --contained` selects that local reference image when no explicit
  custom image is configured. Missing images are never pulled automatically and
  a missing container boundary never falls back to a host run.
- `ingot doctor` distinguishes the selected default from a custom image, reports
  stale `ingot/run:<version>` overrides, and points an absent reference image to
  the product command rather than a repository-specific Docker invocation.
- CI builds the image through `ingot image build` and runs the real contained
  acceptance tests without `--image`.

**A diagnostic human run trace**
([RFC-0007](rfcs/0007-the-ingot-product-loop.md),
[#4](https://github.com/mathissdupont/ingot/issues/4))

- Default text events now form a deterministic numbered trace with qualified
  agent/node provenance, provider/model and tool/sub-agent boundaries, artifact
  origins, failure location, and observed/final step and token budgets.
- Static prompt text is visible, while every dynamic substitution and named
  context value is explicitly redacted. JSON Lines output is byte-compatible in
  shape and order; quiet output remains quiet.
- The same renderer handles local, supervised and contained runs without TTY
  control sequences and preserves existing text landmarks used by scripts.
- Source range resolution now lives in the Agent IR 0.2 source provenance entry
  above; dynamic values remain redacted without a separate secret-classification
  design.

**An integrated edit loop**
([RFC-0007](rfcs/0007-the-ingot-product-loop.md),
[#3](https://github.com/mathissdupont/ingot/issues/3))

- `ingot dev [PATH]` immediately checks and writes canonical Agent IR, then uses
  native filesystem events to repeat that cycle when the entry source or
  manifest changes. Event bursts are debounced without polling the filesystem.
- Failed revisions retain the compiler's authoritative diagnostics, never reach
  build or execution, and leave the last successful artifacts untouched.
- `--run` opts into running each good revision with ordinary `--input`, provider,
  cassette and agent selection. It is off by default and runs synchronously, so
  saving a prompt neither silently calls a model nor creates overlapping runs.
- Generated starter READMEs show both the model-free edit loop and an opt-in
  offline replay loop using their checked-in cassette and example inputs.

**One readiness report before execution**
([RFC-0007](rfcs/0007-the-ingot-product-loop.md),
[#2](https://github.com/mathissdupont/ingot/issues/2))

- `ingot doctor [PATH]` reports compilation, provider routing and credential
  presence, static MCP routing and executable availability, and contained-run
  runtime/image readiness without starting a provider, server or container.
- Every failed check names its source or manifest location and an actionable
  fix. Credential values are never read into or printed by the report.
- `ingot doctor --json` emits the documented schema v1 shape for editors and CI;
  check identifiers and statuses are stable, and exit code `1` means at least
  one blocking prerequisite is missing.
- Container image inspection is now a public read-only sandbox operation. The
  doctor checks the configured custom image or, when none is configured, the
  version-matched `ingot/run:<cli-version>` reference image without pulling it.

**The first complete product-loop path**
([RFC-0007](rfcs/0007-the-ingot-product-loop.md),
[#1](https://github.com/mathissdupont/ingot/issues/1))

- `ingot init --template brief|document-workflow` creates maintained horizontal
  examples rather than an untested skeleton. Both expose ordinary `.ing` source,
  checked-in example inputs and a reviewed cassette.
- A fresh project now checks, builds, replays its suite and runs its example
  artifact with no provider credential. The generated README prints those exact
  commands, and an end-to-end test executes them from the project directory.
- `brief` remains the default, so existing `ingot init <name>` usage is
  compatible; it now leaves the user with a deterministic first run as well as
  compilable source.

**A second backend and the portability report**
([RFC-0006](rfcs/0006-a-second-backend.md); closes
[GAP-018](docs/gaps.md#gap-018))

- `ingot build --target python` emits one self-contained, standard-library-only
  Python 3 program per agent. The generated program independently enforces input
  schemas, policy, step and token budgets, loops, approvals, state, emissions,
  checkpoints and model cassette replay.
- The Python backend depends on Agent IR, not `ingot-runtime`; its execution
  semantics were implemented from the Runtime and IR specifications so agreement
  is evidence rather than shared code.
- A portability report names degraded and unimplemented constructs before any
  program is written. Builds refuse unimplemented nodes by default;
  `--allow-unimplemented` makes the report inspectable without silently emitting
  a program with a hole in it.
- `--json` emits the report as a single machine-readable document, including an
  `unimplemented` list suitable for a deployment gate.
- Differential tests run the same document-summarizer artifact and cassette
  through the Rust interpreter and generated Python, comparing the artifact byte
  for byte and the event kinds in order. CI requires Python on all three hosted
  platforms, so those tests cannot silently skip.
- The first portability report is deliberately honest: `tool.call`,
  `agent.call`, and `verify` are not implemented; `parallel`, `approval`, and
  `checkpoint` are reported as degraded. Supported agents build and run without
  installing a Python package.

**The agent runs inside the boundary too** ([RFC-0005](rfcs/0005-the-contained-run.md),
[ADR-0007](docs/adr/0007-containing-the-run-is-not-blocked-on-a-second-backend.md);
narrows [GAP-001](docs/gaps.md#gap-001))

- `ingot run --contained` runs the interpreter and its tool servers inside a
  container derived from the agent's own `policy` block. 0.3.0 contained an
  agent's *tools*; the process holding the API key and writing the artifacts was
  still the operator's, with the operator's whole machine.
- **`network deny` now applies to the agent.** The box gets `--network none` and
  still completes a model call: the call leaves through a supervisor on the
  standard streams rather than through a socket. Those two things were previously
  incompatible.
- **The credential is outside the boundary by topology.** The provider stays on
  the host, so there is no environment inside for a key to be read from and no
  route to the process that has one — [Runtime 0.1 §11](specs/runtime/v0.1.md)
  satisfied structurally rather than by discipline.
- `--out-dir` is written by the host after the run, from the outputs the guest
  returned. An agent cannot write outside its mounts even to deliver its own
  result. Before this, `--out-dir` was written by a host process that no policy
  constrained.
- An `approval` gate crosses out and is decided by the operator. A gate that
  cannot reach anybody is **refused**, never approved by default.
- A provider failure keeps its kind across the boundary: a rate limit inside is
  the same condition as a rate limit outside, so the interpreter does not behave
  differently depending on where it is running.
- New crate `ingot-supervisor`: the protocol and both halves of the channel.
  Nothing in `ingot-runtime` changed to make a contained run possible, which is
  the test of whether the boundary is really a deployment concern.
- `tools/ingot.Dockerfile` builds the image. It is built **without** the HTTP
  providers, so there is no code inside that could use a key even if one arrived.
- **A program whose agents want different boundaries is refused** rather than run
  in the widest of them ([GAP-023](docs/gaps.md#gap-023)). The two-agent example
  is that case — the coordinator may write and the reviewer may not — and one box
  for both would hand the reviewer a grant its own policy denies. `--sandbox`
  still covers it.
- `--record` with a contained run is refused: the cassette would record the model
  exchanges, which happen outside, and omit the tool results, which happen inside.

**Fixed**

- `ingot-cli` did not build with `--no-default-features`: `catalogue::build` is
  gated on a provider feature and was called unconditionally. A build with no
  HTTP provider now refuses a `[[model.provider]]` declaration by name instead of
  failing to compile.

## [0.3.0] — 2026-08-07

More than one model service, and the policy block enforced rather than only
checked. Prepared for release with prebuilt binaries; never tagged, so the first
published archives are 0.4.0-rc.1's.

- Language version: **0.1** (unchanged; §7.1 defines what a policy path is
  relative to, which it never had)
- Agent IR version: **0.1** (unchanged)
- Runtime version: **0.1** (unchanged; §5.2, §6.1, §7 and §10 clarified)
- CLI version: **0.3.0**

**More than one model vendor** (closes [GAP-021](docs/gaps.md#gap-021))

- An OpenAI-compatible provider, speaking **Chat Completions**. That shape was
  chosen over a vendor-only one because a dozen other services speak it:
  `INGOT_OPENAI_BASE_URL` reaches Azure, a gateway, or a local vLLM or
  llama.cpp server, and the artifact does not change.
- `RoutingProvider` sends each call to the vendor the artifact pinned with
  `model exact "<vendor>/<model>"`. `--provider auto` is the new default, so a
  source that names OpenAI runs against OpenAI without the operator repeating
  it on the command line.
- **A vendor the run cannot reach is an error naming it**, never a redirection.
  Before this release the vendor half of a pinned reference was dropped and the
  call went to Anthropic regardless — a plausible answer from a model the
  artifact did not name, which is worse than a failure.
- `http.rs` is shared by both providers, so the timeout, the retry rule and the
  mapping from status code to error are decided once. Two providers that
  retried differently would make one artifact behave two ways for reasons it
  never mentions.
- The OpenAI provider **refuses to guess a model**. Names change often enough
  that a default produces a `404` reading like a bug in Ingot; the artifact
  pins one, or `--model` does, or the run stops and says so.
- Eleven wire tests against a localhost stub, covering bearer auth, the strict
  JSON schema, refusals, truncation, and a gateway that reports an error with a
  200 status.

**MCP tool host** ([spec](specs/tools/mcp-v0.1.md), [RFC-0003](rfcs/0003-mcp-tool-host.md))

- `ingot-mcp`: an MCP client and a `ToolHost` implementation. Separate from
  `ingot-runtime` on purpose — a backend that hosts tools some other way
  replaces one crate. See [ADR-0005](docs/adr/0005-mcp-over-stdio-only.md).
- `[[mcp.server]]` in `ingot.toml`: where a tool comes from is deployment
  configuration, not part of the artifact, so the same artifact runs against
  different servers unrecompiled. `[mcp.server.tools]` maps an artifact's name
  onto a server's when they differ.
- `ingot tools`: what each configured server publishes and what routes where.
  Exits non-zero when a declared tool has no server, so it works as a CI
  precondition.
- `ingot run --no-tools`: start nothing, for checking that an agent fails the
  way it should when a tool is absent.
- `ingot-mcp-fs`: a sandboxed filesystem MCP server, so a fresh checkout can run
  a tool-using agent without installing anything else. `--root` is required and
  writing needs `--allow-write`; paths that are absolute, contain `..`, or
  resolve through a symlink outside the root are refused.
- `examples/repo-digest`: the example that runs end to end with real tools.

**Runtime**

- `file` and `bytes` have defined runtime representations: a `file` is a handle
  `{"path": "…"}`, and `bytes` is base64. Previously a tool returning either
  failed with "unknown type", which was a defect rather than a design.

### Security

- A tool server starts with a **minimal environment**: `env_clear()`, a fixed
  set of platform-essential variables, then whatever `pass-env` names. Nothing
  the operator exported reaches a tool server by accident.
- `pass-env` takes **names, never values**, and unknown manifest keys are
  rejected. A manifest is committed; a secret written into one is a published
  secret.
- Three independent gates stand between an agent and a file — the compiler's
  effect check, the runtime's re-check against the artifact's own policy, and
  the server's own bound. An agent whose policy allows `filesystem_read` still
  cannot read outside the server's root, and there is a test that asserts it.

**Ingot Containers, stage 1 — the boundary runs** ([RFC-0004](rfcs/0004-ingot-containers.md))

- `ingot run --sandbox` starts each tool server inside the boundary planned for
  the calling agent: those mounts and no others, that network, no capabilities,
  a read-only root filesystem, `/tmp` on tmpfs, and only the environment
  variables `pass-env` named — forwarded by name, so a value never appears in an
  argument vector or a process listing.
- It **refuses before starting anything** when the boundary cannot honour a rule
  the policy states, naming each one. `--sandbox-allow-unenforced` proceeds and
  says which limits are advisory.
- Every run now says which regime is in force — *"the policy is enforced"* or
  *"the policy is checked, not enforced"* — rather than leaving it to be
  inferred from which flags were remembered.
- A server is started **once per agent** that holds one of its tools, so each
  gets its own policy's bound. `ToolInvocation` carries the calling agent,
  because a host that bounds reach cannot apply the right policy without it.
- `image` on each `[[mcp.server]]` says what to run the server inside. The image
  is the operator's choice because the server is the operator's program; without
  one, `--sandbox` says so instead of running it loose.
- `tools/mcp-fs.Dockerfile` builds the reference server from source.
- What the boundary *grants* — as opposed to what we ask for — is asserted
  against a real container runtime in `crates/ingot-sandbox/tests/container.rs`:
  a read mount refuses a write, a write mount reaches the host, a path the
  policy did not name does not exist inside, and `network deny` leaves no
  interface at all. They report and return where no runtime exists;
  `INGOT_REQUIRE_CONTAINER=1` makes that a failure, which is how CI runs them.

### Fixed

- **Every mount failed on Windows.** `Path::canonicalize` yields an
  extended-length path (`\\?\C:\…`), and a container runtime splits a volume
  specification on colons, so `\\?\C:` is one too many and the whole spec is
  rejected. Found by running the boundary rather than by reading it.

**Ingot Containers, stage 1 — planning** ([RFC-0004](rfcs/0004-ingot-containers.md))

- `ingot-sandbox` derives, from an agent's own `policy` block, the boundary its
  tool servers should run inside: which paths are mounted and in which
  direction, whether there is a network, which environment variable **names**
  cross. Pure — it starts nothing, so the interesting logic is testable on a
  machine with no container runtime.
- `ingot sandbox` prints it, `--json` for piping, one plan per **(server,
  agent)** pair. Not per server: in `code-review-team` the sub-agent may read
  and the coordinator may write, and a box wide enough for both would hand the
  sub-agent a grant its own policy denies.
- What a boundary **cannot** enforce is named rather than glossed over — a host
  allowlist needs an egress proxy, and `external_write` is not a thing a
  boundary can judge. `ingot run --sandbox` will refuse to start on an
  unenforced plan.
- Two refusals at plan time: a read mount whose path is missing (mounting an
  empty directory would make a missing checkout look like an empty one), and a
  policy path that is absolute or climbs out of the workspace.

**Language: what a policy path is relative to** ([Language 0.1 §7.1](specs/language/v0.1.md))

- The language never said, and until enforcement existed nothing needed it to.
  Both shipped examples turned out to write policy paths relative to the *tool
  server's root* — which the artifact cannot see, so neither was interpretable
  on its own.
- **A policy path is relative to the workspace**, a root the operator binds at
  run time with `--workspace` or `[run] workspace` in the manifest, defaulting
  to the project. The artifact says `crates`; the operator says where `crates`
  lives.
- Both examples are corrected, and their IR changes accordingly.
  `code-review-team` also turned out to claim it reads `src`, which this
  repository does not have — nobody noticed for exactly as long as nothing
  checked.

**Scope** ([docs/vision.md](docs/vision.md), [ADR-0006](docs/adr/0006-a-policy-enforcing-runner.md))

- `docs/vision.md` states what the project is for end to end, including the two
  things it is growing into: **Ingot Containers**, where an agent's `policy`
  block configures an enforced boundary rather than serving as a checklist, and
  **authoring with a model**, where the compiler is the verifier in the loop.
- [ADR-0006](docs/adr/0006-a-policy-enforcing-runner.md) amends
  [ADR-0002](docs/adr/0002-compiler-not-runtime.md), which had listed five
  conditions for owning a runtime. Two are met and three are not, so the scope
  is narrowed structurally: stage 1 contains the **tool servers** and adds no
  new consumer of the IR, and stage 2 — containing the run itself — is blocked
  on a second backend existing, because a project with its own runtime has an
  incentive to make that runtime the good target.
- Roadmap gains M9 (Ingot Containers) and M10 (`ingot new`). Milestone numbers
  are identities, not positions; the intended order is stated separately.

**Gap register** ([docs/gaps.md](docs/gaps.md))

- Every known limitation now has a stable identifier, a class describing *what
  happens to you* (unenforced, refused, degraded, absent, unproven), and an
  entry saying why it is not done and what closing it would take. The same gaps
  had been restated across six files, and restatements drift.
- Two entries were not written down anywhere before: [GAP-001], a policy
  allowlist whose values are carried into the IR and never enforced, and
  [GAP-002], a `verify` node that reports `passed: true` without a verifier
  existing to have checked anything. Both are **unenforced** — they look like
  guarantees and are not.

### Fixed

- **A sub-agent call disarmed every later approval gate.** The approval mode was
  *moved* into the callee and the caller was left set to deny, so the first
  `agent.call` silently consumed the operator's handler: every gate after it was
  refused without anyone being asked, including under `--yes`. This is exactly
  the shape of `examples/code-review-team`, where sub-agents review the files
  and only then does the external write need a human. The mode is now borrowed —
  there is one operator, and both caller and callee reach the same one.

### Known gaps

See the [gap register](docs/gaps.md). New or changed in this release:
[GAP-006](docs/gaps.md#gap-006) (cassettes carry no tool results),
[GAP-007](docs/gaps.md#gap-007) (MCP over stdio only),
[GAP-009](docs/gaps.md#gap-009) (MCP prompts, resources and sampling).

[GAP-001]: docs/gaps.md#gap-001
[GAP-002]: docs/gaps.md#gap-002

## [0.2.0] — 2026-08-06

The IR becomes executable. A reference interpreter runs an agent end to end
against a real model provider, and re-enforces every guarantee the artifact
declares rather than trusting that a compiler once checked the source.

- Language version: **0.1** (unchanged)
- Agent IR version: **0.1** (unchanged)
- Runtime version: **0.1** (new)
- CLI version: **0.2.0**

### Added

**Runtime 0.1** ([spec](specs/runtime/v0.1.md), [RFC-0002](rfcs/0002-runtime-execution-model.md))

- `ingot-runtime`: a reference interpreter for Agent IR, deliberately narrow —
  it exists to make the IR's meaning precise and testable, not to be a good
  place to host an agent. See [ADR-0002](docs/adr/0002-compiler-not-runtime.md).
- `ModelProvider` and `ToolHost` interfaces; everything vendor-specific sits
  behind them.
- Runtime enforcement of capabilities, step and token budgets, loop bounds and
  approval gates, read from the artifact's own policy object. Duplicating the
  compile-time check is the point: whoever runs an artifact is often not whoever
  built it.
- Typed responses: a declared `responseType` becomes a JSON Schema the provider
  is constrained to, and an answer that does not validate is an error. Prose
  types (`text`, `markdown`) are deliberately left unconstrained.
- A normalised event stream with no timestamps, so replaying a recording
  produces the same events byte for byte.
- `refuse rather than skip`: an unknown node kind, an unenforceable policy
  decision, or an unimplemented IR major version stops the run.

**Cassettes**

- Record a run with `--record` and replay it with `--provider replay`. Cassettes
  store the inputs alongside the exchanges, so they are self-contained.
- Replay verifies a digest of each request, so an edited prompt fails loudly
  rather than silently reusing a stale answer.

**Anthropic provider** (optional `anthropic` feature, on by default in the CLI)

- Messages API over raw HTTP; there is no official Anthropic SDK for Rust.
- Structured output for typed responses, refusal and truncation surfaced as
  errors before content is read, retry with backoff on 429 and 5xx.
- Sampling parameters are deliberately never sent — current models reject them.
- `INGOT_ANTHROPIC_BASE_URL` overrides the endpoint for a gateway or a proxy.

**Toolchain**

- `ingot run` — execute an agent, with `--input name=value` (JSON when it parses
  as JSON, `@file` to read from disk), `--out-dir`, `--events text|json|quiet`,
  and an interactive approval prompt that denies by default when unattended.
- `ingot test` — replay every cassette in a directory. No API key, no network.
- A recorded cassette for the document-summarizer example, replayed in CI.

### Fixed

- `ingot init` generated a project that would not compile when the directory
  name collided with a reserved word: `ingot init agent` emitted `package agent`,
  a syntax error. The package name is now sanitised, and omitted entirely when no
  valid identifier can be derived.

### Known limitations

- **No tool host.** MCP is not implemented, so an agent that grants a tool stops
  with a message saying no host provides it. The `research-agent` and
  `code-review-team` examples compile and check but cannot yet run.
- **`parallel` executes sequentially.** Valid, because the compiler guarantees
  map iterations cannot observe each other — but not yet fast.
- **No streaming.** Output is capped at 16k tokens per call to stay inside HTTP
  timeouts on the non-streaming path.
- **`verify` is a no-op.** IR 0.1 names a verifier but carries no implementation.
- Cost budgets are recorded and reported but not enforced.

## [0.1.0] — 2026-08-06

First milestone release. Complete compiler front end: source to Agent IR.
Backends, packaging and the language server are not part of this release.

- Language version: **0.1**
- Agent IR version: **0.1**
- CLI version: **0.1.0**

### Added

**Language 0.1** ([spec](specs/language/v0.1.md))

- Declarations: `type`, `tool`, `verifier`, `agent`; a required `language`
  declaration and an optional `package`.
- Agent sections: `model`, `tools`, `memory`, `budget`, `policy`, `flow`.
- Types: `string`, `int`, `float`, `bool`, `json`, `bytes`, `text`, `markdown`,
  `file`, lists, and user-declared records. Two lossless widenings: `int` to
  `float`, `markdown` to `text`.
- Flow: bindings, `ask`, `call`, `parallel map`, `verify`, `emit`, `checkpoint`,
  `if`/`else`, bounded `loop`, and reads and writes of working memory.
- Prompt interpolation with `${...}`, resolved and type-checked at compile time.
- Effects — `network`, `filesystem_read`, `filesystem_write`, `external_write`,
  `secret_access`, `model_access` — declared per tool and checked per call.
- Default-deny policy with `allow`, `allow [...]`, `deny` and
  `require approval`; the compiler inserts approval checkpoints from the policy.
- Budgets for `steps`, `tokens` and `cost`, with a static minimum-step check.

**Agent IR 0.1** ([spec](specs/ir/v0.1.md), [schema](specs/ir/agent-ir.schema.json))

- Flat node array with explicit `next` pointers; regions referenced by node id.
- Twelve node kinds, from `llm.call` through `approval` to `artifact.emit`.
- Canonical JSON encoding: two-space indentation, sorted keys, trailing newline.
  The same source always produces byte-identical output.
- Cost amounts encoded as decimal strings, so artifacts do not depend on
  platform float formatting.

**Toolchain**

- `ingot init`, `check`, `fmt`, `build`, `ir`, `explain`.
- Diagnostics with stable codes, source spans, secondary labels, notes, help
  text and "did you mean" suggestions; `ingot explain` prints the long form.
- A parser that recovers from errors, reporting many problems per run and never
  looping on malformed input.
- An idempotent canonical formatter with line-width-aware argument wrapping.
- Three reference examples with checked-in golden IR.

### Known limitations

- No runtime backend: `ingot build` produces IR, not something you can execute.
  That arrives in M3.
- Single-file programs only; module imports are not in 0.1.
- Emission on all paths is checked as a warning, not enforced.
- No OCI packaging, lockfile or artifact digest yet (M6).
- `Ingot` is a working name. Trademark, domain and registry clearance has not
  been carried out and requires legal review before any public release.

[Unreleased]: https://github.com/mathissdupont/ingot/compare/v0.5.2...HEAD
[0.5.2]: https://github.com/mathissdupont/ingot/compare/v0.5.1...v0.5.2
[0.5.1]: https://github.com/mathissdupont/ingot/compare/v0.5.0...v0.5.1
[0.5.0]: https://github.com/mathissdupont/ingot/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/mathissdupont/ingot/compare/v0.4.0-rc.2...v0.4.0
[0.4.0-rc.2]: https://github.com/mathissdupont/ingot/compare/v0.4.0-rc.1...v0.4.0-rc.2
[0.4.0-rc.1]: https://github.com/mathissdupont/ingot/releases/tag/v0.4.0-rc.1
