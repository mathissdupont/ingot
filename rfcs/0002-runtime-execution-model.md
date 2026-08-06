# RFC-0002: Runtime interface and execution model

- Status: **Accepted**
- Created: 2026-08-06
- Affects: runtime spec, CLI

## Problem

After M2 the toolchain checks and compiles, and produces an Agent IR document.
Nothing runs it. That leaves the central claim — that a typed source compiles to
something portable and executable — unproven, and it leaves the IR design
unfalsified: a representation nobody has executed is a guess.

There is a second, quieter problem. Every rule the compiler enforces —
capabilities, budgets, approvals — is enforced *only* at compile time. If a
backend ignores an `approval` node or a `deny` decision, nothing catches it. The
project needs at least one implementation that demonstrably honours the IR's
guarantees, and against which other backends can be compared.

## Goals and non-goals

**Goals**

1. `ingot run` executes an agent end to end against a real model provider.
2. A backend interface (`ModelProvider`, `ToolHost`) other implementations can
   target.
3. Runtime enforcement of every guarantee the IR states — capabilities,
   budgets, loop bounds, approvals — independently of the compile-time check.
4. Deterministic, offline execution for tests and CI, via recorded cassettes.
5. A normalised event stream, so callers do not parse provider-specific output.

**Non-goals for this RFC**

Concurrency (`parallel` runs sequentially — see below), MCP transport, streaming
responses, checkpoint resumption, distributed execution, and a scheduler. Each
is separately motivated and separately risky.

## Is this the "native runtime" ADR-0002 deferred?

No, and the distinction matters enough to state precisely.

[ADR-0002](../docs/adr/0002-compiler-not-runtime.md) defers building a general
agent runtime that competes with existing ones — provider adapters, streaming,
retry policy, context management, tool lifecycles, the whole maintenance
surface. It lists five conditions for revisiting, one of which is that the scope
be "genuinely narrow — a policy-enforcing runner, say, not a general execution
engine."

This is that narrow thing. `ingot-runtime` is a **reference interpreter**: the
executable definition of what the IR means, the oracle a conformance suite
compares backends against, and the thing that makes `ingot test` possible. It
deliberately does not compete: no context management, no provider routing, no
session state, no orchestration features.

The distinction is testable rather than rhetorical. If the reference interpreter
starts growing features that exist to make agents *better* rather than to make
the IR's meaning *precise*, it has crossed the line ADR-0002 draws, and that is
a review objection worth raising by name.

## Proposed design

### The two interfaces

```rust
trait ModelProvider {
    fn name(&self) -> &str;
    fn complete(&self, request: &CompletionRequest) -> Result<CompletionResponse, ProviderError>;
}

trait ToolHost {
    fn call(&self, tool: &ToolInvocation) -> Result<Value, ToolError>;
}
```

Everything provider-specific lives behind the first; everything tool-specific
behind the second. The interpreter itself has no knowledge of any vendor.

### Typed responses are the point

`ask<T>` is where Ingot's type system either becomes real at runtime or stays
decorative. The interpreter turns the IR's `responseType` into a JSON Schema and
asks the provider to constrain its output to it:

| Ingot type | Runtime treatment |
|------------|-------------------|
| `text`, `markdown` | plain completion; the response text is the value |
| `string`, `int`, `float`, `bool` | schema-constrained, wrapped |
| `T[]` | schema-constrained, wrapped |
| a record type | schema-constrained, wrapped |
| `json` | plain completion, parsed as JSON |

Non-object schemas are wrapped as `{"value": <schema>}` and unwrapped on the way
back, because provider structured-output implementations generally expect an
object at the root. Prose types are deliberately *not* schema-constrained —
asking for markdown and receiving a JSON string containing markdown is worse
than asking for markdown.

A response that does not validate against the schema is a runtime error, not a
best-effort parse. That is the whole value of having declared the type.

### Enforcement is duplicated on purpose

The compiler already rejects a call whose effects the policy denies. The
interpreter checks again before every call, from the IR's own `policy` object.

This is not redundant. The compile-time check protects the author; the runtime
check protects whoever runs the artifact, who may not have compiled it and
cannot see the source. An artifact that arrived over a registry is exactly the
case where "the compiler already checked" is not good enough.

Runtime enforces:

- **Capabilities** — a `tool.call` whose effects are not permitted by the
  artifact's own `policy` object fails before the tool is invoked.
- **Budgets** — `steps`, `tokens` and `cost` are counted and enforced. Exceeding
  any of them aborts the run.
- **Loop bounds** — `maxIterations` is honoured even if a guard never falsifies.
- **Approvals** — an `approval` node blocks until the handler answers, and a
  denial aborts the run.

### `parallel` runs sequentially, and that is correct

The interpreter executes `parallel` bodies one element at a time.

This is a valid implementation, not a shortcut, because of what the compiler
already guaranteed: a `parallel` body contains no state write, no emission and
no checkpoint (`ING6005`), so iterations cannot observe each other. Sequential
execution therefore produces the same result as concurrent execution, only
slower.

The IR calls the node `parallel` because it describes an opportunity for
concurrency, not an obligation. A backend that can parallelise should; the
reference interpreter optimises for being obviously correct. The conformance
suite will assert the *result*, never the schedule.

### Determinism through cassettes

A cassette records each model exchange as an ordered interaction:

```json
{
  "cassetteVersion": "0.1",
  "agent": "heptapus.examples.summarizer.DocumentSummarizer",
  "interactions": [
    {
      "index": 0,
      "node": "n0",
      "requestDigest": "b4ee7a…",
      "responseType": "markdown",
      "response": { "text": "…" },
      "usage": { "inputTokens": 812, "outputTokens": 240 }
    }
  ]
}
```

Replay matches by index and verifies `requestDigest`, so a changed prompt
produces a loud mismatch rather than a stale answer. Cassettes make `ingot test`
runnable in CI with no API key and no network — which is the only way agent
tests get run at all.

### Events

Execution emits a normalised event stream (`RunEvent`), which the CLI can print
as human-readable lines or JSON Lines. Events carry no timestamps: a run of the
same cassette produces the same event sequence byte for byte, which is what
makes them assertable in tests.

## Target lowering

Not applicable — this RFC defines an IR consumer, not a source construct. It
does constrain future backends, and those constraints are normative:

- A backend that cannot enforce a policy decision must reject the artifact
  rather than run it.
- A backend that cannot pause for an `approval` node must reject the artifact.
- A backend must reject an unknown node kind, an unknown value kind, or an
  `irVersion` major it does not implement.

## Security and policy impact

This RFC is mostly security surface, so the invariants are worth stating flatly:

- No new effects or capabilities are introduced.
- Runtime enforcement is defence in depth, never a relaxation: the runtime can
  refuse what the compiler allowed, never the reverse.
- Secrets reach the provider from the environment only. They are not read from
  the IR, not written to cassettes, and not included in events.
- Cassettes record prompts and responses. They are test fixtures and must be
  reviewed like any other checked-in data — a cassette recorded against real
  inputs may contain whatever those inputs contained.
- The default `ToolHost` denies everything. A tool runs only through a host the
  operator supplied.

## Static bounds

Unchanged. The runtime enforces the bounds the compiler computed rather than
introducing new ones. One addition: a wall-clock limit, because a provider that
never answers is not something a step budget can catch.

## Compatibility

No language or IR change. `ingot run` and `ingot test` are new commands; the CLI
version moves to 0.2.0. Agent IR 0.1 is unchanged, and artifacts built by 0.1.0
run unmodified.

## Alternatives

**Ship a backend that generates another runtime's configuration instead.** The
plan's original M3. Still the right M5, and the multi-target claim is not proven
until it lands. It was not the right first step: the target's configuration
schema is external and moving, the result cannot be tested in CI without
installing that runtime, and a generator whose output nobody executes proves
nothing about the IR. Building the reference interpreter first gives the
generator something to be *compared against* — which is what makes a portability
report meaningful rather than aspirational.

**Skip execution; go straight to OCI packaging.** Packaging an artifact nobody
can run optimises the distribution of an unproven format.

**Use an existing agent framework as the interpreter.** Would import exactly the
semantics the IR is trying to define, and make "does the IR mean this?"
unanswerable.

## Conformance tests

- [x] `runs_the_document_summarizer_end_to_end`
- [x] `structured_response_types_are_schema_constrained`
- [x] `prose_response_types_are_not_schema_constrained`
- [x] `a_response_that_violates_the_schema_is_an_error`
- [x] `a_denied_capability_is_refused_at_runtime`
- [x] `an_unlisted_policy_subject_is_refused_at_runtime`
- [x] `the_step_budget_is_enforced`
- [x] `the_token_budget_is_enforced`
- [x] `a_loop_stops_at_its_static_bound`
- [x] `an_approval_denial_aborts_the_run`
- [x] `parallel_map_visits_every_element`
- [x] `a_cassette_replays_deterministically`
- [x] `a_changed_prompt_fails_cassette_replay`
- [x] `the_event_stream_is_identical_across_replays`
