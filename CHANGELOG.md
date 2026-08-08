# Changelog

All notable changes are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

Three versions move independently — the language, the Agent IR and the CLI. Each
entry states which of them it affects. See [GOVERNANCE.md](GOVERNANCE.md).

## [Unreleased]

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
- Agent IR 0.1 cannot map node ids to source ranges. This is recorded as
  [GAP-027](docs/gaps.md#gap-027), with the minimum privacy-safe IR 0.2 change
  tracked in [#11](https://github.com/mathissdupont/ingot/issues/11).

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
  doctor checks both a configured custom image and the version-matched
  `ingot/run:<cli-version>` reference image without pulling either one.

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

**Known**

- A wedged contained run is not timed out ([GAP-024](docs/gaps.md#gap-024)).

## [0.3.0] — 2026-08-07

More than one model service, and the policy block enforced rather than only
checked. The first release with prebuilt binaries.

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

[Unreleased]: https://github.com/mathissdupont/ingot/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/mathissdupont/ingot/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/mathissdupont/ingot/releases/tag/v0.1.0
