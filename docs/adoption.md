# What it takes for somebody else to use this

[`README.md`](../README.md) says what Ingot is. [`vision.md`](vision.md) says
what it is for. [`gaps.md`](gaps.md) says what it cannot do. None of them says
what has to happen before somebody who did not build it picks it up, keeps it,
and tells somebody else — so that work kept being restated in conversation and
re-derived from scratch. This file is the list.

The promotion order is **release → outside programs → announcement**, and it is
at step two. 0.9.0 is released and every crate is on crates.io, so nothing
technical blocks handing the language to somebody. What is missing is not a
feature.

## The rule that governs all of it

**Nothing here ships a claim before it is true.** No invented customers, no
partnerships, no certifications, no compliance claims, and no speed claims — a
fan-out measurably got *slower* on a VRAM-bound local model, and that number
stays sayable.

What is genuinely rare and demonstrable today, and therefore the only material
any of this may be built from:

- a policy enforced at run time from the artifact's own data, default-deny
- a run that replays from a cassette with no API key, so an agent is testable
  in CI
- an event stream byte-identical across two independent backends
- a conformance suite that ships inside the binary
- a compile-time budget: "at most $5" is a number you have *before* the run
- Apache-2.0, all of it, with no commercial tier and no open-core split

The audience is one audience with two doors: developer communities and
organisations that want to write their own agents quickly and reliably. The same
artifacts serve both, because a cassette that demos in thirty seconds without an
API key is also the thing an auditor accepts.

## Two halves, and the first one first

**Easy to write** is the half that decides whether anybody gets to a second
agent. **Worth being seen with** is the half that decides whether anybody hears
about the first. Both are real work; authoring leads, because a beautiful page
in front of a hard tool converts traffic into nothing.

---

## Track 1 — Authoring: the "easily" half

### What already works, and should stop being invisible

`ingot new --provider auto` has a model write `.ing` source and the **compiler**
decide whether it is any good — a bounded, deterministic repair loop that

- refuses automatic policy widening: a *grant* is surfaced to a person, while
  `deny` and `require approval` are not, because asking somebody to accept a
  restriction trains them to accept the list without reading it;
- refuses an invented tool the project does not route;
- ends the loop rather than repairing when a candidate carries something shaped
  like a credential, and names where it was, never what it was.

That is the answer to "make writing agents easy", and it is the honest
differentiator: the problem was never that a model cannot write agent code, it
is that nothing checks it. Today it is one row in a table of twenty commands.

Also already present: `ingot dev` watching source, `ingot tools --propose`
turning a live MCP server into declarations, `ingot doctor` answering readiness
without starting anything, seventy-two diagnostic codes with `ingot explain`,
and a language server behind editor diagnostics, formatting, completion, hover
and definition.

### Where it stops

1. **The editor integration is unreachable.** `editors/vscode` holds a built
   `.vsix` and nothing publishes it. Somebody who ran `cargo install ingot-cli`
   has to find the repository, download an archive, sideload it, and set
   `ingot.lsp.path` — after a second `cargo install ingot-lsp` they had no
   reason to know about. This is the cheapest unshipped improvement in the
   project.
2. **The blank page has two doors.** `ingot init` offers `brief` and
   `document-workflow`. Every template ships a recorded fixture, so a template
   is not decoration — it is the difference between reading the language and
   running it.
3. **The studio cannot create a project.** It can already do more than it gets
   credit for: bookmark projects, show diagnostics, readiness, boundary and
   agents, view and edit the flow through the canvas, list and delete runs,
   **start** a run with a chosen agent, provider, cassette and inputs, stop one,
   and answer an approval gate. What it has no route for is `init` and `new` — so
   a project must be created in a terminal before the studio can see it at all.
   That, and the empty bookmark list on first launch, is the whole first-run
   problem.
4. ~~**The studio cannot answer a question.**~~ **Closed 2026-08-20.** It could
   not: the launcher watched for `approvalRequested` and nothing else, and its
   answer channel carried `{ node, allowed: bool }` — a decision, not a string —
   so an agent with a `consult` in it could be started from the studio and never
   finished, with nothing refusing to start one. A launch now waits on a tagged
   `Waiting`, either an approval or a question, and a question is answered with
   one of its choices or free text. Studio schema 2; the CLI's own channel
   already carried both halves, so the language, the IR and the cassette were
   untouched. This was the studio's half of
   [RFC-0020](../rfcs/0020-a-person-in-the-loop.md).
5. **The tenth agent is hard.** [GAP-011](gaps.md#gap-011): an `import` is a
   project-local path, so sharing between projects means copying source. Market
   the first agent, not the tenth, until that closes.

### Deliverables

- Publish the extension to the VS Code Marketplace **and** Open VSX, from a
  release workflow rather than by hand, and make the LSP discoverable from it.
- Two to four more templates, each with a fixture, chosen from what outside
  programs actually turn out to want rather than from what seems tidy.
- Authoring in the studio: create a project, and run `ingot new --provider auto`
  with the refuse-and-repair loop **visible** — each attempt, the diagnostic
  that killed it, the grants awaiting a person. See Track 6: this is where
  character belongs, because the character is the mechanism.
- A first-run path that ends in a real artifact. See Track 4.
- The conversation surface, which is two things sharing one panel. See Track 8.

---

## Track 2 — The refusals

Seventy-two diagnostic codes, each generated into a browsable page: the code,
its real terminal rendering, its `ingot explain` text, and one line on why it
exists. Generated from `ingot-diagnostics`, so it cannot drift from the
compiler.

It is simultaneously the most entertaining artifact the project can produce and
the most useful document a new user will ever read, because a stranger meets
Ingot by pasting an error code into a search engine. The README already leads
with a refusal; this is that page, seventy-two times.

## Track 3 — The playground

Compile the compiler and the interpreter's replay path to WebAssembly and run
them in the browser: no API key, no server, no rate limit, no operating cost,
served as static files. Real diagnostics and a real artifact — the product
itself rather than a recording of it. A visitor breaks a `policy`, Ingot refuses,
and the entire argument lands in five seconds.

This is possible *because* of the cassette design, and it is the highest-leverage
asset available.

**The feasibility spike passed on 2026-08-20, and it needed no source changes.**
Both `ingot-compiler` and `ingot-runtime --no-default-features` build for
`wasm32-unknown-unknown` as they stand. Three properties the project already had
for other reasons are what make it work:

- **`default = []` on the runtime.** Every network provider sits behind a
  feature, so the core interpreter carries no HTTP client and no TLS to port.
- **The entry points are already string-in, string-out.** `compile_source`,
  `Compilation::render_diagnostics`, `Compilation::primary_agent`,
  `format_source` and `Cassette::from_json` are all public and none of them
  touches a path. `compile_path` is a convenience over the first, not the only
  door.
- **A replay never spawns a thread.** `fan_out_ceiling` collapses to one when
  `RunOptions` carries no `ProviderFactory`, and the threaded worker pool is
  only reached above a ceiling of one — so the one API in the interpreter that
  WebAssembly cannot honour is also the one a replay never calls.

The known limitation: `compile_source` takes one source text, so the playground
is single-file. `import` is project-local anyway, so this costs a visitor
nothing they could otherwise have.

What is left is not portability but the host layer — a `wasm-bindgen` wrapper
(MIT/Apache-2.0, so licence-clean) exposing check, format and replay-run, plus
the page around it. The fallback plan is retired.

## Track 4 — The studio's first sixty seconds

The project list is a bookmark file, so a first launch shows an empty page. That
is the worst available first impression, and it sits directly after the install
instructions. Replace it with: create from a template, run the fixture, read a
real artifact — inside a minute of installing, with nothing configured and no
key.

Everything the studio shows must be built with `textContent` and must fetch
nothing from outside the machine; both are asserted by tests in
`crates/ingot-studio/src/lib.rs`. So visual work here is inline SVG and CSS, no
libraries and no external fonts. That is a discipline, not an obstacle.

**The page is modular in the source tree and one document on the wire**
(2026-08-20). `assets/` is now a stylesheet per region and a script per view —
twenty-three files — concatenated by `concat!` at compile time. The single
response is not laziness: every request to this server carries the session
token, so a page that fetched its own stylesheet would put that token in a `src`
attribute and in any screenshot of the page; and a single document cannot
half-load, so no state has to be written to survive markup arriving without its
script. A test walks `assets/` and asserts every file appears in the assembled
page, because the hazard the split introduces is a file that is written, saved,
and never wired in.

**The look landed with it.** Warm paper, one earth accent (brick — deliberately
not the coral this whole field settled on), and the failure red moved to a cooler
crimson so a refusal never reads as the accent. Agents are drawn as characters:
one 16×26 sprite of plain rectangles, recoloured from the agent's own name, built
with `createElementNS` because an agent's name is somebody else's text. A full
figure appears in exactly one place — the panel where the agent is asking you
something — and a gate gets no face, because that is the policy stopping the run
rather than the agent asking for anything.

**The track is the readiness report.** Four rungs — compiles, model, tools,
boundary — each the worst status among the doctor's checks for that subject, and
below it one card naming the first thing that is not passing, carrying the fix
the doctor already wrote. Nothing invents a step or a percentage. Two rungs a
person might expect are deliberately absent because no check answers them yet:
whether a cassette has been recorded, and whether the project has been packaged.
They belong there when the CLI can say so.

## Track 5 — A run somebody can show

`ingot run` already writes the event stream verbatim to
`<out-dir>/runs/<id>.jsonl`. On top of it: a single self-contained HTML report of
one run — what it asked, what it could reach, what it spent against its ceiling,
which gates a person answered, and what it emitted. Plus SVG badges generated
from a checked artifact, for somebody else's repository.

People share what they made, not what they installed. This is the track that
makes the person using Ingot look good, which is the only durable kind of
promotion.

## Track 6 — Identity and tone

**Decided 2026-08-20: character everywhere, not only on the marketing surface.**

The metaphor is the one already in the name — casting, assay and the hallmark
stamp — because it is not a costume laid over the product but a restatement of
it:

| What actually happens | What it is in the metaphor |
|---|---|
| `ingot check`, default-deny, seventy-two codes | the assay: what is actually in the bar |
| `policy`, enforced by a boundary and a filtering proxy | the wall of the crucible |
| `budget { steps, tokens, cost }`, checked at compile time | weight and purity, struck into the bar |
| the OCI package's reproducible digest | the hallmark: is this the same bar |
| cassette replay, no key, byte-identical | the assay certificate, re-checkable cold |
| two backends agreeing on one event stream | two assay offices reaching one result |

The register is institutional deadpan: the joke is how relentlessly Ingot says
no, which is also the product. Ingot has no mascot standing beside the compiler,
because the compiler is the character.

**The constraint that keeps character from costing credibility.** One of the two
doors is an organisation deciding whether to trust an artifact. Character has to
come from the mechanism — an animated refuse-and-repair loop, a digest being
struck, a boundary drawing itself — and never from ornament bolted onto a
reliability claim. Where a surface has to choose between charming and legible,
it is legible.

## Track 7 — Announcement

| Where | The honest hook |
|---|---|
| Show HN | an agent compiler you can try with no API key — playground link |
| r/rust | twenty-one crates, two backends, a differential suite inside the binary |
| r/LocalLLaMA | it really runs on Ollama, including the fan-out that got slower and why |
| This Week in Rust | the release note |
| Lobsters | the language design angle: deliberately not Turing-complete |

**What must be true first:** the playground works, the refusals page is up, and
three to five non-trivial programs written by people who are not the author have
run — which is [GAP-039](gaps.md#gap-039), and is calendar time rather than a
work item. A Show HN that lands on a README loses most of its traffic.

## Track 8 — The conversation surface

Requested 2026-08-20: do this without typing commands, and give the agents a
chat panel. Those are two features sharing one panel, and one of them closes a
hole rather than adding a nicety.

**A run is already conversation-shaped.** An `ask` has a question and an answer,
a tool call has a request and a result, and `consult` is literally a question put
to a person with offered choices. Rendering a run as a transcript is not a
costume over the execution model; it is the event stream read aloud.

**The half that closes a hole is done** (2026-08-20, item 4 above): a question
reaches the page as a question, carrying its text and the answers it offers, and
is answered with one of them or with free text. What is refused is as much of it
as what is allowed — a decision sent where a string was wanted, an answer the
question never offered, and an empty one, because the flow *reads* this value.
Four end-to-end tests assert it from the browser's side of the socket, and one
of them reads the answer back out of the run record rather than off the page.

**What is left of this track** is the rest of the transcript: the `ask`s, the
tool calls and the spend, drawn as the exchange they are rather than as a tail of
captured stderr.

**Authoring is the other half.** `ingot new --provider auto` creates, and
`ingot new --project dir --provider auto "what to change"` proposes a **diff and
writes nothing without `--apply`**. That is already the honest primitive for
conversational editing: every turn that changes the program yields a diff a
person accepts. The source stays the truth, which is the rule the studio was
founded on.

Five constraints, all of them load bearing:

1. **Model output may never become markup.** Everything in the transcript is
   model-generated text, and `ask<markdown>` output is markdown. So a markdown
   renderer that builds DOM nodes, never HTML strings — the existing
   `.innerHTML` test is not in the way of this feature, it is the specification
   for it.
2. **No streaming is needed for a first version.** The page already polls every
   two seconds and that is how the gate UI works today. A transcript can ride
   the same poll; streaming is a refinement, not a prerequisite.
3. **Chat authoring needs a provider, and a first run has none.** Templates and
   the replay fixture stay the keyless door. Conversational authoring appears
   when `ingot doctor` says a provider is reachable, and is absent rather than
   broken when it is not — otherwise the flagship feature fails for exactly the
   newcomer it exists for.
4. **A policy grant must not become a button people click through.** The
   authoring loop surfaces grants deliberately, and refuses to carry them
   silently, because accepting a list without reading it is the failure the
   mechanism prevents. In a chat the accept affordance has to look and behave
   unlike "continue".
5. **An Ingot agent is not a chatbot.** A bounded flow, typed inputs, `emit`
   outputs, no conversation state and no unbounded loop. The panel must not
   imply a memory the execution model does not have. The honest shape is:
   inputs, transcript, questions answered in place, artifact.

## Track 9 — One install

Requested 2026-08-20: *"nobody downloads 21 crates one by one — a person should
install it, write something, run it."*

**First, what is actually true**, because the premise is worth correcting before
it drives a design. Nobody installs 21 crates. `cargo install ingot-cli` is one
command, and a release archive already carries **all three** binaries — `ingot`,
`ingot-mcp-fs` and `ingot-lsp` — in one file, smoke-tested before it ships. The
21 published crates are libraries the CLI depends on; the reason they scroll past
during an install is that `cargo install` **compiles them on your machine**,
which looks exactly like installing 21 things and is the actual complaint.

So the problem is not the number of downloads. It is:

1. **`cargo install` requires a Rust toolchain**, and MSRV 1.85. For anybody who
   is not already a Rust developer that is a second unrelated install, a long
   compile, and a wall — while the archive that needs none of it is one section
   further down the README.
2. **The archive route is entirely manual**: find the releases page, know which
   of four targets you are, download, verify a checksum by hand, extract, and put
   three binaries somewhere on `PATH`.
3. **No package manager knows Ingot.** No winget, no Homebrew, no apt, no
   install script. Every install is a manual one.
4. **There is no `linux-aarch64` build.** The matrix is linux x86_64, macOS on
   both architectures, and Windows x86_64 — so ARM servers, ARM CI runners and
   a Raspberry Pi have no archive at all and must use the toolchain route.
5. **Archives are checksummed and not signed.** `SHA256SUMS` proves a download
   was not corrupted; it does not prove who built it, because it is served by
   the same host as the archive.

### What shipped 2026-08-20

- **`scripts/install.sh` and `scripts/install.ps1`.** Detect the platform,
  resolve the newest version, download, **verify against the release's
  `SHA256SUMS`**, unpack the three binaries into one directory, and print what to
  try. No root, nothing written outside that directory, no `PATH` changed without
  being asked, and no flag to skip verification. Both were run end to end against
  the live 0.9.0 release, including the refusal paths: a hash mismatch exits 2
  naming both hashes and installs nothing.
- **`linux-aarch64` in the release matrix**, on an arm64 runner rather than
  cross-compiled, because the workflow's "the archived binary works" step
  actually executes what it packed and that is the property it exists to hold.
- **`cargo binstall` support** for all three binary crates. It cost only
  metadata: the archives were already named and laid out for it.

*Two findings that only surfaced by writing the installer, both now recorded in
the README as well:*

- **`releases/latest` answers 404 for this project.** Every release is marked as
  a pre-release — correctly, pre-1.0 — and that endpoint excludes pre-releases.
  It is the first thing an installer, a Homebrew livecheck or a winget manifest
  reaches for. Resolve from `releases?per_page=1` instead. `cargo binstall` is
  unaffected: it takes the version from crates.io.
- **The two archive kinds disagree about layout.** A tarball carries a
  `ingot-<version>-<target>/` directory; the Windows zip is flat, because
  `7z a … "./dist/$name/*"` packs the contents where `tar -C dist "$name"` packs
  the directory. Both are already published that way, so the installers look for
  either rather than the layout being quietly corrected at a version boundary —
  which would break anybody's existing script. Normalising it is an open
  decision, not a fix to slip in.

### What is left

- **An install script**, the one-liner: detect the platform, fetch the matching
  archive, verify it, put the three binaries on `PATH`, and print what
  `ingot doctor` says next. `curl … | sh` for Unix and an equivalent for
  PowerShell. No Rust, no choices, one line.
- **Signing before that script is promoted**, not after. Piping a remote script
  into a shell is defensible when the artifact it fetches can be verified
  independently of the host serving it, and not otherwise. This is the same
  missing piece as [GAP-029](gaps.md#gap-029) — no signature scheme and no trust
  root — so the two should be answered once, together.
- **The package managers people actually have**: a winget manifest, a Homebrew
  tap, and `cargo-binstall` — which needs no new artifact at all, only that the
  release archives keep their current naming.
- **`linux-aarch64` in the release matrix.**
- **Keep three binaries, not one.** They stay separate because they are three
  permissions to grant a machine: a compiler, a process that reads your
  filesystem on an agent's behalf, and something an editor starts. One installer
  placing all three is convenience; one *binary* doing all three would be the
  collapse the split exists to prevent.

Submitting to winget and Homebrew means accepting each project's contribution
terms and keeping a manifest current in a repository this project does not own;
both are permissively licensed and neither constrains Ingot's own licence, but
the maintenance is real and belongs to whoever cuts releases.

## Track 10 — The words on the screen

Requested 2026-08-20: *"demo.agent.provider gibi kafa karıştırıcı şeyleri
anlaşılabilir metinlere çevirelim, her yerde böyle şeyler var."*

**First, what is not the problem.** The compiler's diagnostics are good and
should be left alone. Checked by breaking a real project on purpose: a mistyped
placeholder gives `ING2009`, points at the token, offers `did you mean`, and says
why it matters; a denied capability gives `ING4001` and underlines **the policy
line that denies it** as well as the call. Nothing in that wants rewriting, and
"make the errors clearer" must not turn into churn there.

**What is the problem is the machine names leaking into surfaces meant for
people.** Every row below was read off the running studio, not imagined:

| What a person sees | Where | What they were looking for |
|---|---|---|
| `demo.framing.FramingReport` | Agents card, run records | `FramingReport`, with the package underneath |
| `capabilities` | the model chip on an agent | what this agent needs from a model — or nothing at all |
| `human`, `model_access` | the effects line | "asks a person", "calls a model" |
| `artifact<markdown>` | an agent's outputs | `markdown`; the source says `report<markdown>` |
| `node n0 · consultation 0` | the question panel | which step of the flow this is, in the flow's words |
| `provider.default` · "unpinned model calls use `local`" | `ingot doctor` | "model calls that name no model use `local`" |
| `container.configured-image` | `ingot doctor` | the summary beside it already says it; the id is for machines |

**The rule this has to follow, or it breaks things.** The identifiers are a
contract: `doctor --json`, `tools --json` and the studio's own replies are
consumed by other programs, and two of them have a written schema
([`doctor-json-v1.md`](doctor-json-v1.md),
[`tools-json-v1.md`](tools-json-v1.md)). So this is a **presentation layer, not a
rename** — the data keeps `model_access` and `source.compile`, and the page and
the human-readable CLI render the phrasing beside it. A change that renames the
underlying identifier is a schema break pretending to be a copy edit.

One thing to settle while doing it: a qualified agent name is genuinely useful
when two projects declare the same short name, so the package is not noise
everywhere — it belongs as secondary text, not as the heading.

## Track 11 — The conversation, as its own place

Requested 2026-08-20: the question panel should be *"biraz daha ayrı bir tab
gibi"* rather than sitting inside Runs.

Today the tabs are Overview, Canvas, Runs and Boundary, and a question appears
inside Runs under "Started from here" — which is where a run's process is
reported, not where a person is being addressed. Those are two different things
in one place.

The shape this wants is Track 8's transcript with a door of its own: a fifth tab
holding the run as a conversation — the `ask`s, the tool calls, the spend, and the
question answered in place — rather than a tail of captured stderr.

**Built 2026-08-20.** A fifth tab, between Canvas and Runs, holding one run as
the exchange it was. The two open questions were answered as sketched:

- **When nothing is running it shows the last run.** The tab picks the run
  somebody came for — one waiting to be answered, then one still going, then the
  newest record — so it is never empty while the project has history. It is
  deliberately *not* the newest **unfinished** record: a record with no result
  line is a run going in a terminal and a run that was killed equally, so
  preferring one would let an interrupted run from last week outrank the one that
  just finished.
- **A dot on the tab says a run is waiting**, and only this studio's own children
  count. A record with no result line could be either of those two things, and a
  dot that stays lit next to a run that ended weeks ago is worse than no dot.

Three things fell out of building it.

**The launch and the record are joined, in Rust, by the process id.** A record's
id ends with the process id that opened it, so `runs::of_process` finds the record
a launch is writing without new bookkeeping — and refuses a record older than the
launch, because operating systems reuse process ids and a bare suffix match would
happily return a finished run from a previous process with the same one.

**One transcript renderer serves a finished run and a live one.** A record is
flushed a line at a time, so the same events feed both; the only thing the live
case adds is that a question at the end is still answerable. The question panel
moved here from the Runs tab, which now says the run is stopped and offers the way
to it — the same question in two places reads as two questions.

**What a model said is not in the transcript, and cannot be.** `modelCall` carries
the model, the shape of its answer and the tokens, and deliberately not the text:
an event stream that carried prompts would write every prompt and every reply to
disk. So a model's turns are the quiet machinery between the human ones, which is
also an honest picture of where a person's attention is worth spending. An event
kind the page has never seen is shown as itself rather than described wrongly or
dropped.

## Track 12 — Guidance for a contained run

Requested 2026-08-20. Not a copy fix — four separate things are missing, and the
first two mean containment is shown but cannot be used.

1. **The studio cannot start a contained run.** A start request carries agent,
   provider, cassette and inputs. There is no field for `--contained` or
   `--sandbox`, so the boundary is displayed and never exercised.
2. **The Boundary tab spoke about one boundary as though it were both.** Its copy
   now says which is which — `--sandbox` puts each tool server in a box,
   `--contained` puts the agent itself in one and applies whether or not tool
   servers exist — but saying it is not offering it.
3. **Nothing installs the version-matched image.** `ingot image build` exists as
   a command and has no equivalent here, and `container.reference-image` cannot
   even report whether the image is present until a runtime is running.
4. **The only guidance for a missing runtime is "install Docker or Podman".** On
   Windows it does not add that Linux containers are the ones that work
   ([GAP-020](gaps.md#gap-020)).

**Built 2026-08-20, three of the four.**

`StartRequest` gained `contained` and `sandbox`, and the start panel offers them
as two switches. They are separate arrangements rather than degrees of one, so
they are two switches and not a dial: one boxes the agent, the other boxes each
declared tool server. The sandbox switch appears only when the project declares a
server, because a switch for something a project does not have is a puzzle rather
than an option.

**Neither flag is judged at the studio's end.** Whether this machine can raise a
boundary is settled by the run — from the artifact first and the environment
second — and a second opinion here is exactly how `network` came to be refused on
the grounds that no arrangement existed, two releases after one did. What the
studio owes a person is guidance *before* they ask, so a switch that cannot work
is drawn disabled with the reason and the command that fixes it, taken from the
readiness report rather than worked out again.

The command-line building moved into `arguments()`, apart from spawning, so it can
be asserted. Two tests pin it: a run started here always carries `--events json`
and `--approvals stdin` (without them the page cannot see what a run waits for or
answer it), and a boundary flag is passed on **only** when it was asked for. A
`--contained` that goes missing is a run somebody believes is in a box and is not.

`container.runtime`'s fix is now case-specific. It used to say "install and start
Docker or Podman with Linux containers" whether or not either was installed —
telling somebody to install what they have installed is how a report loses their
trust. The runtime layer already separates "no such command" from "the daemon did
not answer", so the advice does too, and on Windows both branches name Linux
containers as the requirement rather than a detail.

The Boundary tab leads with the agent's own boundary **always**, not only when
there is no tool-server plan to show. Somebody reading a plan is the person most
likely to assume it covers the agent as well.

**The fourth was deferred and then asked for, so it is built too.** The job
system it needed turned out to be small: `crates/ingot-cli/src/jobs.rs`, one slot,
no standard input, no gate, no history, reusing the launcher's bounded capture. A
build takes minutes and prints as it goes, so the page shows its log and can stop
it. One at a time deliberately — two builds of the same tag race to the same name.

**And building it surfaced the fact that matters more than the button.**
`ingot image build` needs an **Ingot source checkout**: `tools/ingot.Dockerfile`
beside a `Cargo.toml` whose workspace version matches the binary. Anybody who
installed from a release archive, `cargo install` or `cargo binstall` has none —
so for them a contained run is not one command away, it is a clone of the
repository at this tag away. The page says exactly that where it would otherwise
offer the button, and [GAP-029](gaps.md#gap-029) now records it in the form the
person paying it experiences. A button most people cannot use, with no explanation
for the rest, would have been worse than no button.

## Track 13 — The three things that still needed a terminal

Asked for 2026-08-20, after a sweep of what the studio still could not do. All
three built the same day.

**1. Creating a project.** The worst sentence the page could produce was the
first one somebody with no project would read: *go and use a terminal*. `POST
/api/create` writes a starter — the same starter `ingot new --template …` writes,
by calling the same function, because a studio with its own idea of a starter
would be a second answer to "what does a new project look like" and the two would
drift. It then bookmarks it and opens it, because a project the page created and
cannot find is worse than one it never created.

No model is involved. `ingot new` can also author from a description with a
provider; that spends money and needs a key, and neither belongs behind a button
labelled Create. The description picks the template and becomes the project's own
description, which is what it does on the command line without `--provider`.

Two refusals worth naming: a relative path (the page cannot see which directory
this process was started in, and a file appearing somewhere unexpected is the
worst outcome here) and anything that already exists (the shared writer refuses a
path that exists, one file at a time).

**2. Recording a run.** The page could replay a cassette and not make one, which
is backwards: the moment somebody wants a recording is the moment after a run went
well. `record` on the start request, a field beside the cassette field, and the
path is resolved **lexically** against the project root rather than canonicalised
— the file is not there yet, and often neither is its directory, which the
cassette writer creates. The field hides itself when the boundary switch is on,
because the run refuses that pair and the reason is worth saying before the
refusal rather than after.

**And it found a real defect, which is why the feature is worth more than it
looks.** A recorded run with a question in it *could not be replayed by `ingot
test`*: `run::test` passed `HumanChannel::Deny`, so every consultation failed with
*there is nobody to ask*, while `ingot run --provider replay` replayed the same
cassette happily. RFC-0020 had already settled which of those is right — *how does
an artifact containing a `consult` run in CI at all? It replays* — and `ingot test`
is the command that is CI. Fixed, with a test that pins it, because the previous
tests only ever went through `--provider replay`.

**3. Building the image.** See Track 12 above: built, and the reason most people
still cannot use it is now on the page rather than in a gap entry nobody reads.

What still needs a terminal after this, deliberately: `ingot package`,
`ingot conform`, and authoring with a model. The first two are release-time
operations rather than authoring ones, and the third spends money.

## A language change is queued, and deliberately not first

[GAP-047](gaps.md#gap-047): an agent cannot offer the person options it worked
out for itself. The author raised it while thinking about a coding agent, and
then pointed out that the justification in
[Language 0.3 §1.2](../specs/language/v0.3.md) is inconsistent — the *question*
may be model-authored and unreviewable, so "a reviewer sees the answer space" was
never true of the interaction. What survives is that `choices` is an enforced
constraint, verified in the interpreter, and Ingot states constraints in the
artifact so they are promises rather than hopes.

**The agreed direction:** let the agent compute the members while the source
states the bound.

```text
choice = consult("Which call site?", among: candidates, at_most: 5)
```

The runtime still checks the answer is one of `candidates`; a reviewer still
reads a promise — "one of at most five strings this run produced". Weaker than a
literal list, strictly stronger than free text.

**Sequenced after the release on purpose.** It is a language version, an IR
change, both backends and conformance fixtures — roughly a week — and it must not
ride in the middle of a half-finished interface. Until it lands the working shape
is a free-text `consult` whose *question* carries the options, which costs the
buttons and the guarantee.

## Sequence

| # | Work | Rough size | Depends on |
|---|------|-----------|------------|
| 1 | ~~WebAssembly spike~~ — **done 2026-08-20, passed** | — | — |
| 2 | ~~Studio: a question reaches the page and is answered~~ — **done 2026-08-20** | — | — |
| 2b | ~~Studio: modular assets, and the look~~ — **done 2026-08-20** | — | — |
| 3 | ~~`linux-aarch64` in the release matrix~~ — **done 2026-08-20** | — | — |
| 4 | Publish the editor extension to both registries | 1 day | — |
| 5 | Signing the release archives, answering GAP-029 with it | 2–3 days | — |
| 6 | ~~The install script, Unix and PowerShell~~ — **done 2026-08-20** | — | — |
| 7 | winget manifest and Homebrew tap (`cargo-binstall` **done**) | 2 days | 5 |
| 8 | The refusals page generator | 2–3 days | — |
| 9 | ~~Studio: create a project from a template~~ — **done 2026-08-20** (Track 13) | — | — |
| 10 | Studio first-run path | 2–3 days | 9 |
| 11 | Studio: the whole run as a transcript | 3–4 days | 2 |
| 12 | Studio: authoring by conversation, repair loop visible | 4–7 days | 9, 11 |
| 12b | ~~The words on the screen (Track 10)~~ — **done 2026-08-20** | — | — |
| 12c | ~~The conversation tab (Track 11)~~ — **done 2026-08-20** | — | — |
| 12d | ~~Contained runs offered and guided (Track 12)~~ — **done 2026-08-20** | — | — |
| 12e | ~~Create a project, record a run, build the image (Track 13)~~ — **done 2026-08-20** | — | — |
| 13 | Landing page and hero | 3–5 days | Track 6 |
| 14 | Playground integration | 3–7 days | 1 |
| 15 | `ingot report` and artifact badges | 3–5 days | — |
| 16 | Outside programs | calendar | runs in parallel from the start |
| 17 | Announcement | 1 day | 6, 8, 13, 14, 16 |

The canvas is not in this table because it is **already built** — RFC-0016 was
accepted and implemented on 2026-08-16 and shipped in 0.6.0, so the studio
already edits a flow through span-targeted edits, showing each gesture as a diff
of the lines it will change before it changes them. Rows 4 to 7 are what sits
around it.

## Asset licensing

Everything the project ships or serves must be usable commercially and
sublicensable under Apache-2.0, which rules out most of the pixel art and fonts
that make this kind of page pleasant. Assets are self-made, CC0, or commissioned
with an assignment — decided per asset, in writing, before it is committed.
Fonts included; the studio cannot fetch one anyway.

The distinction between *auditability* and *regulatory compliance* is load
bearing. The first is demonstrable today and is what the material claims. The
second is a legal question, needs professional review, and is not claimed
anywhere.

## Open decisions

1. **`ingot studio` is started by a command**, which is awkward for the surface
   whose point is that there are no commands. Leave it (the audience has a
   terminal), ship a Start Menu or `.desktop` entry from a real installer, or
   put a desktop shell around it. The third answer is a dependency decision, not
   a UI one.
2. Whether sponsorship exists at all, and in what form.
3. Whether this plan becomes an RFC once the playground spike answers the one
   question that changes its shape. It is a plan, not a design, so it lives
   here until something needs deciding rather than doing.

See [RFC-0007](../rfcs/0007-the-ingot-product-loop.md) for the product loop this
sits around, and [`guide/getting-started.md`](guide/getting-started.md) for the
path every one of these tracks is trying to shorten.
