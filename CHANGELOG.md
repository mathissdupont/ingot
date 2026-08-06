# Changelog

All notable changes are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

Three versions move independently — the language, the Agent IR and the CLI. Each
entry states which of them it affects. See [GOVERNANCE.md](GOVERNANCE.md).

## [Unreleased]

Nothing yet.

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
