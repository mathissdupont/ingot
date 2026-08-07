# Changelog

All notable changes are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

Three versions move independently — the language, the Agent IR and the CLI. Each
entry states which of them it affects. See [GOVERNANCE.md](GOVERNANCE.md).

## [Unreleased]

Agents can act. Tools declared in an `.ing` file are served by MCP servers the
operator configures, so the two examples that compiled but could not run now
have a way to run.

- Language version: **0.1** (unchanged)
- Agent IR version: **0.1** (unchanged)
- Runtime version: **0.1** (unchanged; §5.2, §6.1 and §10 clarified)

### Added

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
