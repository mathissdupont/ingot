# Gap register

Every limitation this project knows about, in one place, with a stable
identifier.

It exists because the same gaps were being restated in six files — a README
paragraph, a `Not in 0.1` section, an RFC's closing notes, an ADR's
consequences, a `Known gaps` block, an example's README — and restatements
drift. Those documents keep their reasoning; this one keeps the list. When they
disagree, **this file is the list and the specs are the behaviour**.

## What is and is not a gap

A **gap** is a limitation we know about and have decided, for now, to live
with. It is recorded here, and where it can be seen from the outside it is
documented there too.

A **bug** is not a gap. Something that does not do what it says it does gets
fixed, not catalogued. If a bug cannot be fixed now, it becomes a gap only once
the documentation stops claiming otherwise.

## How a gap shows up

The class is the most important column. It is not severity; it is *what happens
to you*.

| Class | What happens |
|-------|--------------|
| **Unenforced** | The artifact states something and nothing checks it. It looks like a guarantee and is not. Read these first. |
| **Refused** | Attempting it stops the run, naming what could not be done. Limited, but not misleading. |
| **Degraded** | It works and the result is correct; it is weaker in some other way. |
| **Absent** | You cannot express it. The compiler rejects it, or there is no syntax for it. |
| **Unproven** | A claim the project makes that nothing yet demonstrates. |

## Index

| ID | Gap | Class | Closes with |
|----|-----|-------|-------------|
| [GAP-001](#gap-001) | Policy allowlist values are carried but never enforced | Unenforced | GAP-013, then a runtime change |
| [GAP-002](#gap-002) | `verify` reports a check that never ran | Unenforced | a verifier execution model (RFC) |
| [GAP-003](#gap-003) | `cost` budgets are never charged | Unenforced | per-model pricing in the runtime |
| [GAP-004](#gap-004) | No build-time secret scan | Unenforced | M6 |
| [GAP-005](#gap-005) | No streaming; one call, 16k output ceiling | Refused | a streaming provider interface (RFC) |
| [GAP-006](#gap-006) | Cassettes carry no tool results | Refused | cassette format 0.2 |
| [GAP-007](#gap-007) | MCP over stdio only | Refused | GAP-013, then a transport |
| [GAP-008](#gap-008) | `checkpoint` cannot be resumed from | Refused | a resumption model (RFC) |
| [GAP-009](#gap-009) | MCP prompts, resources and sampling unsupported | Refused | language support for each |
| [GAP-010](#gap-010) | `parallel` executes sequentially | Degraded | a scheduler in the interpreter |
| [GAP-011](#gap-011) | One file per program; no modules | Absent | a module system (RFC) |
| [GAP-012](#gap-012) | No optionals, unions, generics or functions | Absent | language 0.2 |
| [GAP-013](#gap-013) | A capability cannot be scoped to an endpoint or a path | Absent | a policy subject for resources (RFC) |
| [GAP-014](#gap-014) | No persistent memory or state migration | Absent | a memory model (RFC) |
| [GAP-015](#gap-015) | No OCI artifact, lockfile or digest addressing | Absent | M6 |
| [GAP-016](#gap-016) | No language server | Absent | M7 |
| [GAP-017](#gap-017) | No conformance suite or backend author guide | Absent | M8 |
| [GAP-018](#gap-018) | The IR has one consumer, so portability is undemonstrated | Unproven | M5 |
| [GAP-019](#gap-019) | The name has had no trademark or registry clearance | Absent | legal review |

---

## Unenforced

These are the ones that can mislead. Each is a place where reading the source
would lead you to believe something the toolchain does not check.

### GAP-001

**Policy allowlist values are carried but never enforced.**

`network allow ["arxiv.org", "github.com"]` and
`filesystem_read allow ["src", "crates"]` parse, type-check, and reach the IR as
a `values` array. Nothing reads that array. The runtime checks the *decision* —
allow, deny, require approval — and stops there.

*How it shows up.* An agent granted `network allow ["arxiv.org"]` can call a
tool that contacts anything, because the reach of a tool is decided by the tool
server, not by the artifact. Ingot cannot see inside another process.

*Why not yet.* Enforcing it needs somewhere to enforce it. For filesystem paths
that could be the tool host; for hosts it cannot be, without either proxying
every tool's traffic or trusting the server to self-limit. Both are designs, not
patches. See [GAP-013] for the language side.

*What closing it needs.* [GAP-013] first, then a runtime that passes the scope
to a host that can honour it, and a conformance test that a backend which cannot
honour it **refuses** rather than ignoring it — the rule in
[Runtime 0.1 §2](../specs/runtime/v0.1.md).

*Recorded in.* [`examples/research-agent/README.md`](../examples/research-agent/README.md),
[Runtime 0.1 §7](../specs/runtime/v0.1.md).

### GAP-002

**`verify` reports a check that never ran.**

`verify CitationCheck(draft, min_sources: 8)` compiles, lowers to a `verify`
node, and at run time the interpreter evaluates the arguments and emits
`Verified { passed: true }`.

*How it shows up.* An event stream that says a verification passed, when no
verifier exists to have performed one. [Runtime 0.1 §5](../specs/runtime/v0.1.md)
permits a backend to treat `verify` as a no-op, so this is legal; `passed: true`
is nonetheless the wrong thing to say.

*Why not yet.* IR 0.1 carries a verifier's *name and signature* and no way to
execute one. A verifier is either a tool call, a model call with a rubric, or
host-provided code, and choosing between those is a design.

*What closing it needs.* Short term, and cheaply: the event should distinguish
"not performed" from "passed". That is a change to the documented event stream,
so it goes with the next runtime revision rather than as a silent edit. Long
term, an execution model for verifiers.

*Recorded in.* [Runtime 0.1 §5](../specs/runtime/v0.1.md) (as permitted),
nowhere as a warning — which is part of the problem.

### GAP-003

**`cost` budgets are never charged.**

`cost <= 5 usd` parses, is checked for a supported currency, and reaches the IR.
The interpreter charges steps and tokens; it never charges cost.

*How it shows up.* A run that exceeds its stated cost budget completes normally.
Step and token budgets do bound it, so this is not unbounded spend — it is an
unenforced *second* bound.

*Why not yet.* Charging cost means knowing the price of the model actually used,
which is provider- and time-dependent data the compiler must not embed: a price
table in an artifact would make it stale the moment it is published.

*What closing it needs.* A price source the provider supplies at run time, plus
a decision on what to do when a provider cannot price a request.
[Runtime 0.1 §8](../specs/runtime/v0.1.md) already says a backend that cannot
price must not pretend to.

*Recorded in.* [Runtime 0.1 §8](../specs/runtime/v0.1.md).

### GAP-004

**No build-time secret scan.**

[SECURITY.md](../SECURITY.md) commits to secrets never entering an artifact.
Nothing checks it. The commitment holds today because there is no syntax for a
secret literal and no path from the environment into the IR, but that is an
argument, not a test.

*How it shows up.* A prompt with an API key pasted into it compiles and ships.

*What closing it needs.* A scanner over source, IR and cassettes, run by
`ingot build`. Scheduled with M6, when there is an artifact to sign.

*Recorded in.* [SECURITY.md](../SECURITY.md).

---

## Refused

These stop the run and say what they could not do. They limit what you can
build; they do not mislead you about what you built.

### GAP-005

**No streaming; one call, and a 16k output ceiling.**

Every `ask` is one non-streaming request. A response longer than the ceiling
ends the run with a truncation error.

*What closing it needs.* A streaming shape on `ModelProvider`, and a decision
about what a partially streamed structured response means when it fails
validation.

### GAP-006

**Cassettes carry no tool results, so `ingot test` cannot test a tool-using
agent.**

A cassette records model exchanges. `ingot test` hosts no tools deliberately —
replaying a tool call would mean reaching a real server, and a test that touches
the filesystem is not the offline, repeatable thing `ingot test` promises. So a
tool-using agent fails under `ingot test` rather than passing by luck.

*How it shows up.* Of four examples, only `document-summarizer` has cassette
tests. The others' execution paths are covered by integration tests in
`crates/ingot-cli/tests/`, not by `ingot test`.

*The workaround.* `ingot run --provider replay --cassette …` *does* host tools:
a deterministic model with real tools. That is what the end-to-end tests use.

*What closing it needs.* Cassette format 0.2, recording tool invocations and
their results alongside model exchanges, keyed by a digest of the invocation the
way model requests already are. Note the review burden this adds: a recorded
tool result contains whatever the tool returned.

*Recorded in.* [Runtime 0.1 §10](../specs/runtime/v0.1.md),
[RFC-0003](../rfcs/0003-mcp-tool-host.md).

### GAP-007

**MCP over stdio only.**

A tool server is a local child process. Remote servers — including hosted ones
an organisation may already run — cannot be used. The workaround is a local
proxy process, which is a real cost.

*Why not yet.* Reaching a server over a network is itself a `network` effect,
and the language cannot yet scope that to an endpoint ([GAP-013]). The honest
alternatives were to make every HTTP tool call require blanket `network allow`,
which makes the effect useless, or to design the scoping properly first.

*Recorded in.* [ADR-0005](adr/0005-mcp-over-stdio-only.md),
[MCP binding 0.1 §1](../specs/tools/mcp-v0.1.md).

### GAP-008

**`checkpoint` cannot be resumed from.**

`checkpoint "sources-collected"` lowers to a node that emits an event. There is
no way to stop at one and continue later.

*What closing it needs.* A serialised interpreter state, which means deciding
what is in it — bindings, working memory, usage so far — and what happens when
the artifact changes between the stop and the resume.

*Recorded in.* [Runtime 0.1 §5](../specs/runtime/v0.1.md).

### GAP-009

**MCP prompts, resources and sampling are unsupported.**

A server that requires one of these is refused at the handshake rather than
partially supported.

*Why not yet.* An agent's prompts are compiled into its artifact, so a server
supplying prompts would be supplying something the artifact already fixed.
Sampling — a server asking to use the agent's model — would be an effect nothing
declared. Resources have no type in the language.

*Recorded in.* [MCP binding 0.1 §1](../specs/tools/mcp-v0.1.md).

---

## Degraded

### GAP-010

**`parallel` executes sequentially.**

`parallel map` runs its iterations one after another. The result is identical:
the compiler guarantees a `parallel` body contains no state write, no emission
and no checkpoint, so iterations cannot observe one another. Only the wall clock
differs.

*What closing it needs.* Concurrency in the interpreter, and a decision about
what a provider rate limit does to a fan-out.

*Recorded in.* [Runtime 0.1 §5.1](../specs/runtime/v0.1.md).

---

## Absent

### GAP-011

**One file per program; no modules.**

A compilation unit is a single `.ing` file. There is no `import`. A shared
`type` or `tool` declaration must be copied.

*How it shows up.* `ingot fmt` formats the entry file only, and a project cannot
be split by concern.

*Recorded in.* [Language 0.1 §3 and §9](../specs/language/v0.1.md).

### GAP-012

**No optionals, unions, generics or user-defined functions.**

The type universe is fixed: scalars, `T[]`, and declared records. A value is
always present, and a tool that may or may not return something has no way to
say so.

*Recorded in.* [Language 0.1 §4 and §9](../specs/language/v0.1.md).

### GAP-013

**A capability cannot be scoped to an endpoint or a path at the call site.**

A policy grants `network`; it cannot grant "network to this host, for this
tool". The `values` list looks like it does that, and does not ([GAP-001]).

This is the root cause of [GAP-001] and [GAP-007], and it is the most valuable
language change on this list: it is what would let the effect system make a
statement about *reach* rather than only about *kind*.

*What closing it needs.* An RFC. At minimum: what a scope is, whether it
attaches to the policy or the tool declaration, how a backend that cannot
enforce it must refuse, and how it composes with a sub-agent's own policy.

### GAP-014

**No persistent memory or state migration.**

`memory { working ephemeral { … } }` is the only form. State lives for one run.
There is no `persistent`, and therefore no question yet of migrating it.

*Recorded in.* [Language 0.1 §9](../specs/language/v0.1.md).

### GAP-015

**No OCI artifact, lockfile or digest addressing.**

`ingot build` writes canonical JSON to a directory. There is no packaging, no
lockfile pinning what an artifact was built against, and nothing addressed or
signed by digest — even though the IR encoding was designed for exactly that
([ADR-0004](adr/0004-canonical-ir-encoding.md)).

*Closes with.* M6.

### GAP-016

**No language server.**

No completion, no hover, no inline diagnostics. `ingot check` in a terminal is
the whole editor story.

*Closes with.* M7.

### GAP-017

**No conformance suite or backend author guide.**

[Runtime 0.1 §13](../specs/runtime/v0.1.md) states what a conforming backend
must do. There is no suite to run against it and no guide for writing one. The
interpreter's own tests are the closest thing, and they are not packaged for
anyone else.

*Closes with.* M8, and it is partly blocked by [GAP-018]: a conformance suite
written against one implementation tends to encode that implementation.

---

## Unproven

### GAP-018

**The IR has one consumer, so the portability claim is undemonstrated.**

The project's central claim is *write once, run on any compliant runtime*.
Everything is built for it — a target-neutral IR, a byte-reproducible encoding,
a written runtime specification, a conformance section. One thing is missing:
a second implementation.

Until an artifact runs somewhere that is not our own interpreter, "portable" is
a design intention, not an observed property, and the IR's design is
unfalsified — a representation nobody else has executed is a guess that
type-checks.

*How it shows up.* Nowhere, which is the problem. Everything passes.

*What closing it needs.* M5: a second backend, plus the portability report —
a machine-readable statement of what a given target does and does not support,
so an artifact can be checked against a target before it is shipped there.

*Recorded in.* [README roadmap](../README.md#roadmap),
[RFC-0002](../rfcs/0002-runtime-execution-model.md).

---

## Non-technical

### GAP-019

**The name has had no trademark, domain or registry clearance.**

"Ingot" was chosen for fit, not availability. No trademark search, no domain
check, no crates.io reservation. This is not something the maintainers can
resolve by reading; it needs a professional search before any release under the
name.

*Recorded in.* [README](../README.md), [CHANGELOG](../CHANGELOG.md).

---

## Closed

None yet. When a gap closes, move it here with the release that closed it, and
leave its section in place. Identifiers are never reused: a link to GAP-007 must
keep meaning what it meant.

[GAP-001]: #gap-001
[GAP-007]: #gap-007
[GAP-013]: #gap-013
[GAP-018]: #gap-018
