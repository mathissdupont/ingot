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
| [GAP-001](#gap-001) | A policy's host allowlist is not enforced | Unenforced | an egress proxy in the runner |
| [GAP-002](#gap-002) | `verify` reports a check that never ran | Unenforced | a verifier execution model (RFC) |
| [GAP-003](#gap-003) | `cost` budgets are never charged | Unenforced | per-model pricing in the runtime |
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
| [GAP-017](#gap-017) | No conformance suite or backend author guide | Absent | M8 |
| [GAP-019](#gap-019) | The name has had no trademark or registry clearance | Absent | legal review |
| [GAP-020](#gap-020) | The boundary needs Linux containers | Refused | a second expression of the boundary |
| [GAP-022](#gap-022) | Nothing has been released; you build from source | Absent | one tag |
| [GAP-023](#gap-023) | A contained run cannot cross a boundary to a sub-agent | Refused | a box per agent, over the supervisor |
| [GAP-024](#gap-024) | A wedged contained run is not timed out | Degraded | a deadline on the supervisor channel |
| [GAP-025](#gap-025) | The product loop is fragmented across commands and raw output | Degraded | M11 |
| [GAP-028](#gap-028) | A model-authored project has no offline test until one run is recorded | Degraded | cassette synthesis, or nothing |
| [GAP-029](#gap-029) | An image cannot be verified by signature, so acquisition stays manual | Refused | a signature scheme and a trust root |

---

## Unenforced

These are the ones that can mislead. Each is a place where reading the source
would lead you to believe something the toolchain does not check.

### GAP-001

**A policy's host allowlist is not enforced.**

*Narrowed 2026-08-07.* This entry used to cover paths as well. It no longer
does: `ingot sandbox` derives a boundary from `filesystem_read allow [...]` and
`filesystem_write allow [...]`, and paths are now defined relative to the
workspace so they mean the same thing on two machines
([Language 0.1 §7.1](../specs/language/v0.1.md),
[RFC-0004](../rfcs/0004-ingot-containers.md)). What remains is the network.

`network allow ["arxiv.org", "github.com"]` parses, type-checks, and reaches the
IR as a `values` array. The boundary can give a tool server a network or
withhold one; it cannot bound egress to named hosts.

*How it shows up.* An agent granted `network allow ["arxiv.org"]` can call a
tool that contacts anything. `ingot sandbox` says so rather than implying
otherwise, and `ingot run --sandbox` refuses to start unless the operator
acknowledges it — but the limit itself is not applied.

*Narrowed again 2026-08-08.* `network deny` used to be enforced for an agent's
tool servers and not for the agent, because the interpreter ran on the host with
the host's network. `ingot run --contained` closes that half
([RFC-0005](../rfcs/0005-the-contained-run.md)): the interpreter runs with
`--network none` too, and its model call leaves through the supervisor rather than
through a socket. What is still unenforced is only the *allowlist* — the choice
between a network and none is now made and kept in both arrangements.

*Why not yet.* An allowlist needs an egress proxy: a component every tool
container routes through, which resolves and filters by host. That is a real
piece of infrastructure with its own failure modes — DNS rebinding, IP-literal
requests, TLS SNI versus Host header — and doing it badly would be worse than
not doing it, because a sandbox that is trusted and wrong is the bad case.

*What closing it needs.* An egress proxy in the runner, and a conformance test
that a backend which cannot honour a scope **refuses** rather than ignoring it,
per [Runtime 0.1 §2](../specs/runtime/v0.1.md). [GAP-013] is the separate,
language-side half: expressing a scope per call rather than per agent.

*Recorded in.* [`examples/research-agent/README.md`](../examples/research-agent/README.md),
[Runtime 0.1 §7](../specs/runtime/v0.1.md),
[RFC-0004](../rfcs/0004-ingot-containers.md).

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

### GAP-029

**An image cannot be verified by signature, so acquisition stays manual.**

*Narrowed by M6.* An image reference may now be digest-pinned —
`ingot/run@sha256:…` in `[run] image` or `--image` — and a contained run compares
the pin with the digest the local image carries, refusing when they differ
([Ingot Package 0.1 §9](../specs/image/v0.1.md)). What a pin cannot yet say is
*who* produced those bytes.

*How it shows up.* `ingot run --contained` never downloads an image. A missing
one is an error naming `ingot image build`, and never a pull and never a host-run
fallback. Getting an image onto a machine is therefore a step the operator takes
deliberately, with whatever tool they already trust.

*Why not yet.* A signature needs a trust root, a key custody story and a
revocation story. None of those are a compiler's to invent, and signature
verification without a trust root is theatre — it would look like a guarantee
while checking that a file signed itself. Shipping digest pinning, which is a
complete property on its own, and naming what is missing is better than shipping
something that resembles both.

*What closing it needs.* A signature over the manifest digest, a documented trust
root, and a refusal path for an unsigned or unverifiable image. All three, before
a pull may become automatic.

*Recorded in.* [RFC-0012](../rfcs/0012-the-ingot-package.md),
[Ingot Package 0.1 §9](../specs/image/v0.1.md),
`crates/ingot-cli/src/image.rs`.

### GAP-020

**The boundary needs Linux containers.**

`ingot run --sandbox` expresses a boundary as a read-only root filesystem,
`--cap-drop ALL`, `--network none` and a POSIX working directory. Those are
Linux-container features. A Windows-container daemon accepts `--volume` and
rejects `--read-only`, so proceeding would produce a boundary with the hardening
silently missing.

*How it shows up.* On a machine whose Docker is in Windows-container mode —
including GitHub's `windows-latest` runner — `--sandbox` refuses, naming the
reason. Running without it still works, with the policy checked rather than
enforced. A Windows host with Docker Desktop in Linux-container mode is fine,
and is where this was developed.

*Why not yet.* Expressing the same boundary a second way — Windows containers,
or an OS-level mechanism such as Landlock or seccomp without containers at all —
is a second implementation of the security-critical part, and a second one that
is subtly weaker is worse than not having it.

*What closing it needs.* A `Boundary` abstraction with more than one backing,
and a conformance test per backing asserting the same properties the container
tests assert today.

*Recorded in.* [RFC-0004](../rfcs/0004-ingot-containers.md),
`crates/ingot-sandbox/src/executor.rs`.

### GAP-023

**A contained run cannot cross a boundary to a sub-agent.**

`ingot run --contained` puts the whole run in one box, planned from the entry
agent's policy. A program whose agents would get different boundaries is
**refused**, naming both and what each would have got.

*How it shows up.* `examples/code-review-team --contained` does not run: the
coordinator may write `target/review` and the reviewer may not, and one box for
both would hand the reviewer a write grant its own policy denies. Single-agent
programs, which is most of them, are unaffected. `--sandbox` still works on the
two-agent case, because it gives each agent's tool servers their own boundary.

*Why not yet.* The fix is one box per agent, with an `agent.call` crossing the
supervisor so the host starts a fresh boundary for the callee. The channel is
already there and the method is a small addition; what needs deciding first is
what a `file` handle means when it crosses between two boxes. A path under the
workspace is portable, because both boxes mount the workspace at the same place;
a path outside it is not, and silently passing one would be a hole exactly where
this feature claims to close one.

*What closing it needs.* An `agent` method on the supervisor protocol, a rule for
handles that cross, and a test that a sub-agent in its own box cannot reach the
caller's write mount.

*Recorded in.* [RFC-0005](../rfcs/0005-the-contained-run.md),
[ADR-0007](adr/0007-containing-the-run-is-not-blocked-on-a-second-backend.md),
`crates/ingot-cli/src/contained.rs`.

## Degraded

### GAP-024

**A wedged contained run is not timed out.**

The supervisor reads the guest's output synchronously. A guest that neither
answers nor exits — a tool server deadlocked inside the box, a container the
runtime has lost track of — leaves the host blocked on a read.

*How it shows up.* `ingot run --contained` hangs instead of failing. Ctrl-C ends
it and the container is `--rm`, so nothing is left behind, but a run in CI with no
outer timeout would sit there.

*Why not yet.* A deadline on the channel is not the obvious `read_timeout`: the
guest is legitimately silent for as long as a model call takes, and that call is
being served by *us*, so the host cannot distinguish "waiting for my answer" from
"wedged" without tracking which side owes the other a message. That state machine
is worth writing carefully rather than quickly. `ingot-mcp`'s `ChildTransport`
solves the easier version of this problem, where every wait has a fixed bound.

*What closing it needs.* A deadline that applies only while the guest owes the
host a message, plus a bounded wait for the first `config` call so a guest that
never starts is reported quickly.

*Recorded in.* [RFC-0005](../rfcs/0005-the-contained-run.md),
`crates/ingot-supervisor/src/host.rs`.

### GAP-028

**A model-authored project has no offline test until one run is recorded.**

`ingot new --provider …` writes source, a manifest, a README and example inputs.
It does not write a cassette, so `ingot test` has nothing to replay until the
operator runs `ingot run --record …` once against a configured provider. The
template path is unaffected: `ingot init` and `ingot new` without a provider ship
a reviewed cassette, because the prompt they generate is one the maintainers
wrote and can record against.

*How it shows up.* A freshly authored project passes `check`, `build` and `test`
with no key — but `test` passes by having nothing to run, and the generated
README says so and prints the command that fixes it.

*Why not yet.* A cassette pairs a request digest with the answer a model gave.
The digest depends on the prompt after interpolation, which is only known by
executing the flow; the answer is only known by asking a model. Writing a
plausible answer for an authored prompt would produce a green `ingot test` that
demonstrates nothing, which is worse than an empty one — the same reason
[GAP-002](#gap-002) is on this list.

*What closing it needs.* Either recording the first run as part of authoring,
which makes `ingot new` execute the agent it just wrote and needs its own
consent step, or nothing: the one recorded run may simply be the right price for
a test that means something.

*Recorded in.* [README](../README.md#authoring-with-a-model), the generated
project README, [RFC-0007](../rfcs/0007-the-ingot-product-loop.md).

### GAP-010

**`parallel` executes sequentially.**

`parallel map` runs its iterations one after another. The result is identical:
the compiler guarantees a `parallel` body contains no state write, no emission
and no checkpoint, so iterations cannot observe one another. Only the wall clock
differs.

*What closing it needs.* Concurrency in the interpreter, and a decision about
what a provider rate limit does to a fan-out.

*Recorded in.* [Runtime 0.1 §5.1](../specs/runtime/v0.1.md).

### GAP-025

**The product loop is fragmented across commands and raw output.**

The compiler, builder, cassette runner, event stream and policy-derived
container all work. Maintained templates, `ingot doctor`, `ingot dev` and the
human trace now cover creation, readiness, the edit loop and run diagnosis;
integrated tool/safe-run guidance is still missing.

*How it shows up.* Safe-run and tool onboarding remain separate from the edit
loop.

*What closing it needs.* M11 and the P1–P5/P8 work packages in
[RFC-0007](../rfcs/0007-the-ingot-product-loop.md): templates whose instructions
are tested, `ingot doctor`, `ingot dev`, a human trace and integrated tool/safe-run
readiness.

*Recorded in.* [Vision](vision.md#one-product-loop-around-the-language),
[RFC-0007](../rfcs/0007-the-ingot-product-loop.md).

## Absent

### GAP-011

**One file per program; no modules.**

A compilation unit is a single `.ing` file. There is no `import`. A shared
`type` or `tool` declaration must be copied.

*How it shows up.* `ingot fmt` formats the entry file only, and a project cannot
be split by concern.

*Initial Language 0.2 fix.* [RFC-0008](../rfcs/0008-language-v0.2-modules-and-imports.md)
specifies project-local imports, and the first implementation can import shared
`type`, `tool` and `verifier` declarations. Broader package semantics,
wildcards, re-exports and agent imports remain out of scope.

*Recorded in.* [Language 0.1 §3 and §9](../specs/language/v0.1.md).

### GAP-012

**No optionals, unions, generics or user-defined functions.**

The type universe is fixed: scalars, `T[]`, and declared records. A value is
always present, and a tool that may or may not return something has no way to
say so.

*Initial Language 0.2 fixes.* Optionals and unions are specified and implemented
in [RFC-0009](../rfcs/0009-language-v0.2-optionals-and-unions.md). Pure helper
functions are specified and implemented in
[RFC-0010](../rfcs/0010-language-v0.2-pure-helper-functions.md). Generics are
deferred until repeated real source demonstrates the need in
[RFC-0011](../rfcs/0011-language-v0.2-generics-decision.md).

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

### GAP-022

**Nothing has been released; you build from source.**

`cargo build --release` works and is tested on three platforms, but there is no
tag, no GitHub Release and no prebuilt binary. Trying Ingot therefore starts
with installing a Rust toolchain, which is a large first step for someone
deciding whether the idea is worth ten minutes.

*What closing it needs.* One command —
`git tag v0.3.0 && git push origin v0.3.0`. The machinery is in place:
[`.github/workflows/release.yml`](../.github/workflows/release.yml) builds
`ingot` and `ingot-mcp-fs` for Linux, macOS on both architectures, and Windows;
refuses a tag that disagrees with the workspace version; runs each archived
binary before shipping it; and publishes one `SHA256SUMS` covering all of them.

*Not closed yet on purpose.* Publishing is outward-facing and
[GAP-019](#gap-019) — no trademark or registry clearance for the name — is still
open. That is a decision for the maintainer, not a build step.

### GAP-017

**No conformance suite or backend author guide.**

[Runtime 0.1 §13](../specs/runtime/v0.1.md) states what a conforming backend
must do. There is no suite to run against it and no guide for writing one. The
interpreter's own tests are the closest thing, and they are not packaged for
anyone else.

*Closes with.* M8. M5 closed [GAP-018] and supplied the first independent
differential tests; packaging them for third-party backends and writing the guide
remain.

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

When a gap closes it moves here with the release that closed it, and its section
stays where a link can find it. Identifiers are never reused: a link to GAP-007
must keep meaning what it meant.

### GAP-015

**No OCI artifact, lockfile or digest addressing.**
*Closed in Unreleased (M6).*

`ingot package` writes a standard OCI image layout holding one artifact manifest:
the Agent IR blobs verbatim, a lockfile, and optionally a portability report per
target. The package digest is `sha256` of the manifest, and it is reproducible —
no timestamps, no build-machine paths, no compression, one canonical JSON
encoding throughout — so the same inputs give the same digest on every platform.
`ingot package --verify` names every source, agent and metadata field that moved
since the package was written.

There is deliberately no registry client
([RFC-0012](../rfcs/0012-the-ingot-package.md)): the layout is the interoperable
thing, and `oras`, `skopeo` and `crane` already move it. Signature verification
for images is separately open as [GAP-029](#gap-029).

*Recorded in.* [Ingot Package 0.1](../specs/image/v0.1.md),
[RFC-0012](../rfcs/0012-the-ingot-package.md), [README](../README.md),
[CHANGELOG](../CHANGELOG.md).

### GAP-004

**No build-time secret scan.**
*Closed in Unreleased (M6).*

`ingot build` and `ingot package` scan the project's source, the compiled Agent
IR bytes and every cassette for credential-shaped **values**, and refuse rather
than warn. The refusal names the file, the line and the shape, and never the
value: a report that quoted it would have copied the credential into a terminal
and a CI log.

The scan is about values rather than words, so an agent may legitimately be about
password resets or key rotation. The same scanner guards model-assisted
authoring, because a generator must not be able to write what the packager would
refuse.

*What this does not claim.* It is a check on the author, not a security boundary:
a credential shaped like an English sentence passes. The property
[SECURITY.md](../SECURITY.md) states is that the toolchain provides no *path* for
a secret to reach an artifact, and the scanner does not replace it.

*Recorded in.* [Ingot Package 0.1 §8](../specs/image/v0.1.md),
[SECURITY.md](../SECURITY.md), `crates/ingot-package/src/secrets.rs`.

### GAP-027

**Agent IR carried no portable source spans.**
*Closed in Unreleased (IR 0.2).*

Agent IR 0.2 adds optional `sourceSpan` metadata to every node the compiler
lowers from Ingot source. The field stores a project-relative, slash-normalized
source identifier plus UTF-8 byte offsets. Compiler-inserted approval nodes
inherit the gated call's span, so a human trace points at the source expression
that caused the approval requirement.

The metadata is descriptive only: runtime event JSON, execution semantics,
canonical node ids, policy, budgets and cassette request digests are unchanged.
IR without `sourceSpan` remains valid for non-Ingot producers and older
artifacts.

*Recorded in.* [Agent IR 0.2](../specs/ir/v0.2.md),
[RFC-0007](../rfcs/0007-the-ingot-product-loop.md),
`crates/ingot-cli/src/trace.rs`.

### GAP-026

**The reference contained run needed a manually prepared image.**
*Closed in Unreleased (M11).*

`ingot image build` now finds the Ingot source checkout, verifies its workspace
version against the running binary, and builds the shipped recipe under the
exact `ingot/run:<cli-version>` tag. `ingot run --contained` selects that local
image when no deliberate `[run] image` or `--image` override exists, and
`ingot doctor` reports its readiness with the same command as the fix.

The command does not weaken the supply-chain boundary: it never downloads an
image, custom images remain explicit operator choices, and a missing runtime or
image never falls back to a host run. M6 added digest pinning for an image
reference; signed acquisition is separately open as [GAP-029](#gap-029).

*Recorded in.* [README](../README.md#putting-the-agent-in-the-box-too),
[CHANGELOG](../CHANGELOG.md),
[RFC-0007](../rfcs/0007-the-ingot-product-loop.md).

### GAP-018

**The IR had one consumer, so the portability claim was undemonstrated.**
*Closed in Unreleased (M5).*

Closed by the independent Python backend in
[RFC-0006](../rfcs/0006-a-second-backend.md). It shares no runtime code with the
reference interpreter: one Agent IR artifact and one cassette run through both,
and the differential test compares emitted artifact bytes and event order.

The portability report separately names every degraded or unimplemented node
kind before a build. That distinction matters: the test demonstrates the common
subset (`llm.call`, control flow, state, emission, budgets, policy and replay);
it does not claim Python implements `tool.call`, `agent.call`, or `verify`.

This closes the unproven claim, not [GAP-017]: the tests are repository-local
and are not yet a packaged conformance suite or backend author guide.

*Recorded in.* [README roadmap](../README.md#roadmap),
[CHANGELOG](../CHANGELOG.md), [RFC-0006](../rfcs/0006-a-second-backend.md).

### GAP-021

**One model vendor.** *Closed in 0.3.0.*

Until 0.3.0 the only network provider was Anthropic. An artifact could say
`model exact "openai/gpt-5.1"` — the language and the IR had carried a
`vendor/model` reference since 0.1 — and the runtime would drop the vendor half
and send the call to Anthropic anyway. That was the worst available behaviour:
not a refusal, but a plausible answer from a model the artifact did not name.

Closed by an OpenAI-compatible provider and a `RoutingProvider` that dispatches
on the vendor prefix. A vendor the run cannot reach is now an error naming it.

The provider speaks Chat Completions rather than a vendor-only shape, so
`INGOT_OPENAI_BASE_URL` reaches Azure, a gateway, or a local server with no
change to the artifact — which is what makes "one vendor" no longer the right
way to count.

*Recorded in.* [README](../README.md), [CHANGELOG](../CHANGELOG.md).

[GAP-001]: #gap-001
[GAP-007]: #gap-007
[GAP-013]: #gap-013
[GAP-018]: #gap-018
