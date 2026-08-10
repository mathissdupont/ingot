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
| [GAP-007](#gap-007) | MCP over stdio only | Refused | a transport, now that GAP-013 is closed |
| [GAP-008](#gap-008) | `checkpoint` cannot be resumed from | Refused | a resumption model (RFC) |
| [GAP-009](#gap-009) | MCP prompts, resources and sampling unsupported | Refused | language support for each |
| [GAP-010](#gap-010) | `parallel` executes sequentially | Degraded | a scheduler in the interpreter |
| [GAP-011](#gap-011) | No package semantics beyond project-local imports | Absent | a package model (RFC) |
| [GAP-012](#gap-012) | No generics | Absent | evidence, then an RFC |
| [GAP-014](#gap-014) | No persistent memory or state migration | Absent | a memory model (RFC) |
| [GAP-017](#gap-017) | No conformance suite or backend author guide | Absent | M8 |
| [GAP-020](#gap-020) | The boundary needs Linux containers | Refused | a second expression of the boundary |
| [GAP-023](#gap-023) | A contained run cannot cross a boundary to a sub-agent | Refused | a box per agent, over the supervisor |
| [GAP-025](#gap-025) | The product loop is a sequence of commands with no single surface | Degraded | a surface over the existing contracts |
| [GAP-028](#gap-028) | A model-authored project has no offline test until one run is recorded | Degraded | cassette synthesis, or nothing |
| [GAP-029](#gap-029) | An image cannot be verified by signature, so acquisition stays manual | Refused | a signature scheme and a trust root |
| [GAP-030](#gap-030) | A verifier cannot be executed at all | Absent | a verifier execution model (RFC) |
| [GAP-031](#gap-031) | A contained run does not stream, and keeps the 16k ceiling | Refused | a delta notification on the supervisor channel |
| [GAP-032](#gap-032) | The Python backend does not stream, so the two backends accept different answer lengths | Degraded | streaming in the Python prelude |
| [GAP-033](#gap-033) | `--effort` cannot be honoured by the Gemini protocol | Refused | one thinking control that holds across model generations |

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

*Narrowed again 2026-08-10.* The language-side half is done. A tool now declares
where it goes — `!network("arxiv.org")` — the compiler checks that against the
grant, and the artifact carries it ([GAP-013], [RFC-0014](../rfcs/0014-a-capabilitys-reach.md)).
Two things follow. A runner that gains an egress proxy now has something precise
to enforce, per call rather than per agent. And a program that states a reach no
longer runs as though it had been kept: it refuses, and takes
`--allow-unenforced-scopes` to proceed. What is left here is only the
enforcement, and it can no longer be reached by accident.

*What closing it needs.* An egress proxy in the runner, and a conformance test
that a backend which cannot honour a scope **refuses** rather than ignoring it,
per [Runtime 0.1 §2](../specs/runtime/v0.1.md).

*Recorded in.* [`examples/research-agent/README.md`](../examples/research-agent/README.md),
[Runtime 0.1 §7](../specs/runtime/v0.1.md),
[RFC-0004](../rfcs/0004-ingot-containers.md).

---

## Refused

These stop the run and say what they could not do. They limit what you can
build; they do not mislead you about what you built.

### GAP-007

**MCP over stdio only.**

A tool server is a local child process. Remote servers — including hosted ones
an organisation may already run — cannot be used. The workaround is a local
proxy process, which is a real cost.

*Why not yet.* Reaching a server over a network is itself a `network` effect,
and until [GAP-013] the language could not scope that to an endpoint. The honest
alternatives were to make every HTTP tool call require blanket `network allow`,
which makes the effect useless, or to design the scoping properly first.

*Unblocked 2026-08-10.* GAP-013 is closed: a tool can now declare the host it
reaches and the compiler checks it against the grant. What remains here is the
transport itself, and one decision it forces — whether the endpoint a remote
server lives at is part of the `[[mcp.server]]` declaration, the tool's reach,
or both.

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

### GAP-031

**A contained run does not stream, and keeps the 16k ceiling.**

`ingot run --contained` puts the interpreter inside the boundary and leaves the
provider holding the credential outside it
([RFC-0005](../rfcs/0005-the-contained-run.md)), so a completion already crosses
the supervisor channel. A fragment of an answer would have to cross it too, as a
notification rather than as a reply, and the protocol has no such notification.

*How it shows up.* A contained run prints its trace and no live text, and one
call is capped at 16,000 output tokens rather than 64,000
([Runtime 0.3 §4](../specs/runtime/v0.3.md)), so an answer a host run would
complete can end a contained run with a truncation error. Everything else is
identical: the event stream is the same either way, because a delta is not an
event ([Runtime 0.3 §2](../specs/runtime/v0.3.md)), which is what
[Runtime 0.1 §7.1](../specs/runtime/v0.1.md) already requires of a contained
run.

*Why not yet.* The channel carries a request and gets a reply. A delta is
neither — it is a one-way notification the host must interleave with whatever
else the guest is saying, and the guest's `event` notifications show the shape
exists. What has not been done is the decision to widen the protocol, and the
supervisor protocol is the interface the boundary is expressed through, so
widening it for display text is a change worth making deliberately rather than
in passing.

*What closing it needs.* A delta notification on the supervisor channel, a rule
that keeps it out of the event stream on both sides of the boundary, and the
raised ceiling only once the guest's calls actually stream.

*Recorded in.* [Runtime 0.3 §6](../specs/runtime/v0.3.md),
[RFC-0013](../rfcs/0013-streaming.md),
[RFC-0005](../rfcs/0005-the-contained-run.md).

### GAP-033

**`--effort` cannot be honoured by the Gemini protocol.**

`--effort low|medium|high|xhigh|max` reaches Anthropic and OpenAI, which each
take a named reasoning level. The Gemini protocol has no single equivalent: one
model generation takes a thinking budget in tokens, the next takes a named
level, and the two are not interchangeable. A run that names an effort and
routes to `google` stops and says so.

*Why refused rather than mapped.* Guessing which control a given model accepts
produces a rejected request on half the catalogue. Sending neither and saying
nothing would be worse: an operator would believe a flag took effect on a run
where it did nothing, and the only evidence would be a bill. The refusal is
checked when this provider answers a call rather than when it is constructed,
so exporting a Gemini key does not break `--effort` for an artifact pinned to
another vendor.

*What closing it needs.* One control that holds across model generations —
either because the protocol grows one, or because Ingot carries a table of
which models take which, which is a catalogue this project has so far refused
to keep for exactly the reason model names change.

*Recorded in.* `crates/ingot-runtime/src/google.rs`.

## Degraded

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
[GAP-002](#gap-002) was worth closing.

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

*Nothing here misleads.* [Runtime 0.1 §5.1](../specs/runtime/v0.1.md) already
says the node marks an opportunity for concurrency rather than an obligation,
and that conformance asserts the result and never the schedule. This entry is a
performance gap, which is why it is Degraded rather than Unenforced.

*What closing it needs — the full price.* This entry used to say "concurrency in
the interpreter, and a decision about what a provider rate limit does to a
fan-out". That undersold it. Concurrency in a fan-out makes the order of model
and tool calls nondeterministic, and three things depend on that order today:

- **Replay matches by position.** `ReplayProvider` and `ReplayToolHost` both take
  the next recorded row. Concurrent calls arriving in a different order than they
  were recorded would make `ingot test` flaky, so replay would have to match by
  digest instead.
- **The event stream is compared across backends.**
  `the_event_streams_agree_on_kind_and_order` asserts that the reference
  interpreter and the Python backend emit the same events in the same order. The
  Python backend is sequential, so a concurrent reference diverges unless each
  iteration's events are buffered and spliced back in index order.
- **`ModelProvider::complete` takes `&mut self`.** Calling it concurrently means
  making the central backend contract thread-safe — a change to the interface
  every backend implements, for a wall-clock win.

None of that is impossible; it is a deterministic-concurrency design, and the
honest version of it keeps every property above. It is simply much larger than
"concurrency in the interpreter", and it should not be started by someone who
believes the shorter sentence.

*What must not be done.* Concurrency for live providers and sequential execution
under replay. It is the cheap version, and it makes an artifact behave one way in
a test and another in production — the divergence this project exists to refuse.

*Recorded in.* [Runtime 0.1 §5.1](../specs/runtime/v0.1.md),
`crates/ingot-cli/tests/differential.rs`.

### GAP-025

**The product loop is a sequence of commands with no single surface.**

*Narrowed.* This entry used to say integrated tool and safe-run guidance was
missing. It is not: every work package it named has landed, and
[RFC-0007](../rfcs/0007-the-ingot-product-loop.md)'s conformance list is
complete. Templates, `ingot doctor`, `ingot dev`, the human trace, typed tool
onboarding, contained-run readiness, model-assisted authoring and packaging all
exist, and the editor and the CLI are tested to report the same diagnostics.

What is left is not another command. It is that the loop is *nine* of them, each
correct and each printing to a terminal, with no place that shows a project's
state at once: what compiles, what it may reach, what a run did, what it cost,
what is ready and what is not.

*How it shows up.* Answering "is this agent alright?" means running `check`,
`doctor`, `tools`, `test` and reading three kinds of output. Every fact is
available; none of them are in the same place.

*What closing it needs.* A surface over the interfaces that already exist —
`doctor --json`, `tools --json`, the run event stream, the human trace,
`package --json` — and **nothing behind it**. RFC-0007 rejected building a UI
first precisely because it would have encoded missing semantics and become a
second source of truth; those interfaces now exist, so a consumer of them would
not. A surface that computed anything the CLI cannot would be that second source
of truth arriving late.

*Recorded in.* [Vision](vision.md#one-product-loop-around-the-language),
[RFC-0007](../rfcs/0007-the-ingot-product-loop.md).

### GAP-032

**The Python backend does not stream, so the two backends accept different
answer lengths.**

`crates/ingot-backend-python/src/prelude.py` makes one whole-body request per
`ask` and keeps the 16,000-token ceiling that
[Runtime 0.3 §4](../specs/runtime/v0.3.md) reserves for that transport. The
reference interpreter asks for up to 64,000 against a provider that streams.

*How it shows up.* An answer between 16,000 and 64,000 output tokens completes
on the reference interpreter and ends the run with a truncation error on the
Python backend. Below 16,000 the two agree, which covers every reference example
and every differential fixture.

*Nothing here misleads about the event stream.*
`the_event_streams_agree_on_kind_and_order` still holds, and it holds for a
reason rather than by luck: a delta is not an event
([Runtime 0.3 §2](../specs/runtime/v0.3.md)), so the two backends emit the same
events in the same order whether or not one of them streamed. What differs is
the length of answer each accepts, which is a portability difference rather than
a divergence in what a run means — and it is why this is Degraded rather than
Unenforced.

*Why not yet.* The Python backend exists to demonstrate that Agent IR has more
than one consumer ([RFC-0006](../rfcs/0006-a-second-backend.md)), and it
implements the common subset deliberately. Streaming there is an event-stream
reader per vendor shape plus a second implementation of the rule that a partial
answer is never used ([Runtime 0.3 §3](../specs/runtime/v0.3.md)). The failure
mode of getting that second implementation subtly wrong is a value derived from
a fragment, which is precisely what the rule exists to prevent.

*What closing it needs.* Streaming in the Python prelude: an event-stream reader
per vendor shape, the same accumulate-then-parse path the whole-body case
already uses, and the raised ceiling only where the transport earns it.

*Recorded in.* [Runtime 0.3 §6](../specs/runtime/v0.3.md),
[RFC-0013](../rfcs/0013-streaming.md),
`crates/ingot-backend-python/src/prelude.py`.

## Absent

### GAP-011

**No package semantics beyond project-local imports.**

*Narrowed by Language 0.2.* This entry used to say a program was one file and
there was no `import`. Both are now false:
[RFC-0008](../rfcs/0008-language-v0.2-modules-and-imports.md) landed, and a
project can split shared `type`, `tool` and `verifier` declarations across files
and import them.

What is still absent is everything above that: wildcards, re-exports, importing
an agent, and a package identity that means anything outside one directory. An
`import` is a project-local path, so code cannot be shared between projects
except by copying it.

*How it shows up.* `ingot fmt` formats the entry file only. Two projects that
want the same `tool` declaration keep two copies of it.

*What closing it needs.* A package model, which is a larger question than a
module system: it has to say what a package is named, how a version is resolved,
and how that interacts with the artifact digest
([Ingot Package 0.1](../specs/image/v0.1.md)).

*Recorded in.* [Language 0.2](../specs/language/v0.2.md),
[RFC-0008](../rfcs/0008-language-v0.2-modules-and-imports.md).

### GAP-012

**No generics.**

*Narrowed by Language 0.2.* This entry used to cover optionals, unions and
user-defined functions as well. All three landed:
[RFC-0009](../rfcs/0009-language-v0.2-optionals-and-unions.md) for `T?` and
`A | B`, [RFC-0010](../rfcs/0010-language-v0.2-pure-helper-functions.md) for
expression-only helpers. A tool that may or may not return something can now say
so.

Generics remain absent, and **by decision rather than by omission**:
[RFC-0011](../rfcs/0011-language-v0.2-generics-decision.md) defers them until
repeated real source demonstrates the need. A small agent language that grows a
type parameter system because it seemed principled is how a deliberately small
language stops being one.

*What closing it needs.* Evidence first — real `.ing` that is worse without
them — and then an RFC. Not the other way round.

*Recorded in.* [Language 0.2](../specs/language/v0.2.md),
[RFC-0011](../rfcs/0011-language-v0.2-generics-decision.md).

### GAP-030

**A verifier cannot be executed at all.**

*The half of [GAP-002](#gap-002) that was a design rather than a bug.*
`verifier CitationCheck(draft: markdown, min_sources: int)` parses, type-checks
and reaches the IR as a name and a signature. Nothing can carry the check out.

*How it shows up.* `ingot check` warns (`ING6006`) and the run reports the node
as `notPerformed`, so nothing claims the property holds — but nothing tests it
either. A `verify` is documentation with a type signature.

*Why not yet.* A verifier is either a tool call, a model call with a rubric, or
host-provided code, and those have different security stories: the first needs an
effect, the second needs a budget, the third needs a way to ship code with an
artifact. Picking one by implementing it would settle the design by accident.

*What closing it needs.* An RFC. At minimum: what a verifier *is*, which effects
it declares, how a failing check affects a run, and what a backend that cannot
execute one must do — which [Runtime 0.2 §1](../specs/runtime/v0.2.md) already
answers for the reporting half.

*The workaround.* A `tool` call does execute, and its effects are checked at the
call site. A property worth enforcing today should be a tool.

*Recorded in.* [Runtime 0.2 §4](../specs/runtime/v0.2.md),
[Language 0.1 §6](../specs/language/v0.1.md).

### GAP-014

**No persistent memory or state migration.**

`memory { working ephemeral { … } }` is the only form. State lives for one run.
There is no `persistent`, and therefore no question yet of migrating it.

*Recorded in.* [Language 0.1 §9](../specs/language/v0.1.md).

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

## Closed

When a gap closes it moves here with the release that closed it, and its section
stays where a link can find it. Identifiers are never reused: a link to GAP-007
must keep meaning what it meant.

### GAP-013

**A capability cannot be scoped to an endpoint or a path at the call site.**
*Closed in Unreleased (Language 0.2 §9).*

A tool now declares where it goes, and the compiler checks that against what the
agent granted:

```ingot
tool web.search(query: string) -> search_result[] !network("arxiv.org")

policy { network allow ["arxiv.org", "github.com"] }
```

Reaching a host the policy does not grant is `ING4009`, naming both the
declaration and the grant — the two halves of the mistake are in different
places and either one may be the wrong one
([Language 0.2 §9](../specs/language/v0.2.md),
[RFC-0014](../rfcs/0014-a-capabilitys-reach.md)).

*Why both sides state it.* This entry called scoping "the most valuable language
change on this list", and the reason was that the effect system could speak
about kind and not about reach. A scope written only in the policy would not
have fixed that: the compiler would still have had one statement and nothing to
compare it against. Two statements — the tool says what it needs, the agent says
what it permits — is what makes containment checkable. The alternative of
scoping at the call site was rejected for scattering security decisions through
the flow, and for making a backend discover mid-run that it cannot honour
something.

*What a policy value became.* Before this, adding a host to `network allow [...]`
changed nothing a compiler could see, and removing one changed nothing either.
The list was a claim with no reader. It has one now.

*And the run refuses.* A declared reach is a stronger statement than a policy
value, so an arrangement that cannot keep it stops before starting anything,
rather than running as though it had. `--allow-unenforced-scopes` proceeds and
names every declaration it is proceeding without. This is opt-in: an artifact
written before this change declares no reach and is unaffected, and produces
byte-identical IR.

*What it did not do.* Nothing bounds egress to a host yet — that is the
enforcement half, [GAP-001](#gap-001), which this narrows rather than closes. No
wildcards, no ports, no URL paths: a host is still matched exactly.

*Recorded in.* [Language 0.2 §9](../specs/language/v0.2.md),
[RFC-0014](../rfcs/0014-a-capabilitys-reach.md),
[GAP-001](#gap-001), [GAP-007](#gap-007).

### GAP-005

**No streaming; one call, and a 16k output ceiling.**
*Closed in Unreleased (Runtime 0.3).*

A provider may now deliver an answer as it is produced, and `ingot run` shows
the text as it arrives. A streamed call may ask for up to 64,000 output tokens
instead of 16,000, because the ceiling is a property of the transport rather
than of the artifact: a service that composes a whole body before sending it
holds the connection open for the length of the answer, and several refuse a
larger `max_tokens` outright unless the request streams
([Runtime 0.3](../specs/runtime/v0.3.md),
[RFC-0013](../rfcs/0013-streaming.md)).

*Two channels, and the difference is the point.* `emit` carries the event
stream — ordered, timestamp-free, reproduced byte for byte by a replay. `delta`
and `settled` carry the live channel, which is a property of the connection and
not of the run. A delta is therefore never an event, never recorded in a
cassette, and never asserted on. Putting fragments into the recorded stream
would have broken [Runtime 0.1 §9](../specs/runtime/v0.1.md), broken cassette
position matching, and broken
`the_event_streams_agree_on_kind_and_order`, whose only repair would have been
a second backend fabricating events it never received.

*The question this entry asked, answered.* "What a partially streamed
structured response means when it fails validation." It means nothing: **a
partial answer is not an answer.** The value a run uses is always assembled from
the finished response and validated whole, by the same code path a non-streamed
response takes, and on any failure the accumulated text is discarded and the run
fails with `Truncated` or `InvalidResponse` exactly as it did before. Salvaging
a prefix was rejected because it would make a result depend on where a
connection happened to stop — an input nothing records and nobody controls.
`settled(node, kept: false)` exists because a watcher was shown text that then
got thrown away, and a half-finished answer left on screen looks like a result.

*Retries stop at the first fragment.* A stream that fails part-way is not
retried: the caller has already shown that text to somebody, and a second
attempt would repeat it from the beginning. Retries cover only the window before
anything was observed.

*Additive, all of it.* Both provider methods and both sink methods are
defaulted, so a backend written against Runtime 0.2 satisfies 0.3 with no edit —
it reports that it does not stream, keeps the smaller ceiling, and emits the
event stream it emitted before.

*What is deliberately not done.* A contained run does not stream and keeps the
16k ceiling ([GAP-031](#gap-031)); the Python backend does not stream, so the
two backends accept different answer lengths ([GAP-032](#gap-032)); and deltas
carry no determinism guarantee, which is what "not an event" means.

*Recorded in.* [Runtime 0.3](../specs/runtime/v0.3.md),
[RFC-0013](../rfcs/0013-streaming.md),
[Runtime 0.1 §9](../specs/runtime/v0.1.md).

### GAP-022

**Nothing has been released; you build from source.**
*Closed in 0.4.0-rc.2.*

`ingot`, `ingot-mcp-fs` and `ingot-lsp` are published for Linux, Windows and
macOS on both architectures, with one `SHA256SUMS` covering all of them. Trying
Ingot no longer starts with installing a Rust toolchain.

*What the first candidate found.* v0.4.0-rc.1 was tagged and produced nothing:
its Intel macOS job asked for the `macos-13` runner image, which GitHub has
retired, and a job asking for a retired image queues indefinitely rather than
failing. That binary is now cross-compiled from Apple silicon and still executed
before it ships. This is the entire reason the first release was a candidate.

*Still a candidate on purpose.* The language, the Agent IR and the artifact
format may change before 1.0, and the archives say so.

*Recorded in.* [README](../README.md#install),
[CHANGELOG](../CHANGELOG.md),
[`release.yml`](../.github/workflows/release.yml).

### GAP-003

**`cost` budgets are never charged.**
*Closed in Unreleased (Runtime 0.2).*

The interpreter charges cost alongside steps and tokens, against prices the
project supplies per model in `[[model.price]]`
([Runtime 0.2 §3](../specs/runtime/v0.2.md)). Exceeding the budget ends the run.

*Where the prices had to come from.* Not the artifact and not the binary: a price
is provider- and time-dependent, so either would be stale the moment it was
published. They live where the API keys and the tool servers already live — the
project manifest, which is deployment configuration the operator owns.

*What "must not pretend to" turned into.* A budget is enforced only against a
total that missed nothing. A call the run could not price — no price configured,
or a price in a currency the budget is not in — leaves the budget **unenforced**,
and the run names each model and why. `ingot check` says it earlier still
(`ING5007`) when the project configures no price at all, because learning it
after the money is spent is not learning it.

*Arithmetic.* Millionths of a currency unit, as integers. No cost calculation
touches a float, so a total is exact and identical on every platform — the same
reason [Agent IR](../specs/ir/v0.1.md) stores an amount as a decimal string.

*What is deliberately not done.* Currency conversion. A rate is a second
time-dependent input, and guessing one would make a budget mean something the
operator did not write. A price in the wrong currency is reported as unpriceable.

*Recorded in.* [Runtime 0.2 §3](../specs/runtime/v0.2.md),
[Runtime 0.1 §8](../specs/runtime/v0.1.md).

### GAP-006

**Cassettes carry no tool results, so `ingot test` cannot test a tool-using
agent.**
*Closed in Unreleased (cassette 0.2).*

A cassette now records tool invocations and their results alongside the model
exchanges, keyed by a digest of the invocation the way model requests already
were ([Runtime 0.2 §2](../specs/runtime/v0.2.md)). `ingot run --record` captures
them; `ingot test` serves them. No server is started, nothing is reached, and a
call whose arguments changed since recording is refused rather than answered from
the wrong row.

A recorded **failure** is replayed as a failure, because how an agent behaves
when a tool fails is the behaviour most worth having a test for.

*What a replay does not do.* Perform the effect. An agent whose recorded run
wrote a file receives the handle that write produced and leaves the filesystem
alone. `ingot test` proves what an agent does *with* a tool's answer, not that
the tool still answers that way — checking the second is what a live run is for.

*The review burden this adds, as predicted.* A recorded result contains whatever
the tool returned, and a cassette is committed. The build-time secret scan reads
cassettes for exactly this reason
([Ingot Package 0.1 §8](../specs/image/v0.1.md)).

*What is left over.* The capability exists; the reference examples still carry no
recordings, because making one needs a model. That is one `ingot run --record`
per example with a key exported — the same one-recorded-run price
[GAP-028](#gap-028) describes.

*Recorded in.* [Runtime 0.2 §2](../specs/runtime/v0.2.md),
[RFC-0003](../rfcs/0003-mcp-tool-host.md).

### GAP-024

**A wedged contained run is not timed out.**
*Closed in Unreleased.*

The host now reads the guest's output on its own thread and waits on a channel,
so a wait can end. Two deadlines, because the two silences mean different things:
**60s** from spawn to the first `config` call, for a guest that may not exist
yet, and an idle deadline between two lines from a guest that has started. A
wedged run is ended rather than waited on; the container is `--rm`, so killing it
leaves nothing behind.

The state machine the original entry worried about turned out to be simpler than
feared. Initiative flows one way — the guest calls, the host answers — so the
host is only ever waiting when the guest owes it a message. And the guest is not
silent while it works: it narrates with `event` notifications, so **every line
resets the idle deadline** and the bound only has to cover the gap between two
steps rather than the length of a run.

The longest legitimate gap is one tool call inside the box, which the guest's own
`[mcp] timeout-seconds` already bounds — so the idle deadline is **derived** from
it (`max(120s, timeout × 2 + 60s)`) rather than being a second number somebody
has to keep in step. `[run] timeout-seconds` and `--timeout` override it;
`--timeout 0` waits indefinitely, which is a deliberate choice rather than a
default.

*Recorded in.* [RFC-0005](../rfcs/0005-the-contained-run.md),
[README](../README.md#putting-the-agent-in-the-box-too),
`crates/ingot-supervisor/src/host.rs`.

### GAP-002

**`verify` reported a check that never ran.**
*Closed in Unreleased (Runtime 0.2).*

The event carried `passed: true` for a node nothing had performed. A boolean can
describe two states and there are three, so the field was replaced rather than
extended: `outcome` is `notPerformed`, `passed` or `failed`, and `passed` is
gone. Leaving it present and adding `performed` beside it would have kept the
misleading reading available and made the honest answer the one you get by
reading two fields in the right order.

The compiler now says it earlier too: `ING6006` warns that a declared verifier is
one nothing in the toolchain can perform, so the gap is visible at `ingot check`
rather than in an event stream after the fact.

*What this did not close.* Verifiers still cannot be executed —
[GAP-030](#gap-030) is that half, and it needs an RFC rather than a fix. What
changed is that the run no longer claims otherwise.

*Recorded in.* [Runtime 0.2](../specs/runtime/v0.2.md),
[`ingot explain ING6006`](../crates/ingot-diagnostics/src/codes.rs).

### GAP-019

**The name has had no trademark, domain or registry clearance.**
*Closed by decision, 2026-08-09.*

Ingot is an open-source toolchain, not a brand. The project **claims no rights in
the name** and is not seeking a trademark: "Ingot" was chosen because it fits
what the tool does, and it is used descriptively rather than as an assertion of
ownership.

*What the search would have been for.* Two different things, and separating them
is what let this close. A *registry* collision is a fact anyone can check, and
there is one: `ingot` on crates.io belongs to an unrelated packet-parsing
library. It costs nothing, because that crate publishes no binary and Ingot's CLI
is distributed as `ingot-cli` and as prebuilt archives — `cargo install
ingot-cli` installs a binary called `ingot` either way. What is left is discovery
confusion, not a conflict.

A *trademark* search is the other thing, and it protects against being made to
rename after adoption. That risk is real and is **knowingly accepted**: the cost
of a rename is a maintainer's problem, and holding a release for a search nobody
had commissioned was holding it for a decision that had already been made.

*What would reopen this.* Commercial use of the name, a registered mark asserted
by someone else, or a hosted service under it. Any of those needs professional
legal review; this entry is a record of a maintainer's decision, not advice.

*Recorded in.* [README](../README.md#licence), [GAP-022](#gap-022).

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
