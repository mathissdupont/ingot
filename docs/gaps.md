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
| [GAP-009](#gap-009) | MCP prompts, resources and sampling unsupported | Refused | language support for each |
| [GAP-010](#gap-010) | `parallel` executes sequentially | Degraded | a scheduler in the interpreter |
| [GAP-011](#gap-011) | No package semantics beyond project-local imports | Absent | a package model (RFC) |
| [GAP-012](#gap-012) | No generics | Absent | evidence, then an RFC |
| [GAP-020](#gap-020) | The boundary needs Linux containers | Refused | a second expression of the boundary |
| [GAP-023](#gap-023) | A contained run cannot cross a boundary to a sub-agent | Refused | a box per agent, over the supervisor |
| [GAP-028](#gap-028) | A model-authored project has no offline test until one run is recorded | Degraded | cassette synthesis, or nothing |
| [GAP-029](#gap-029) | An image cannot be verified by signature, so acquisition stays manual | Refused | a signature scheme and a trust root |
| [GAP-031](#gap-031) | A contained run does not stream, and keeps the 16k ceiling | Refused | a delta notification on the supervisor channel |
| [GAP-033](#gap-033) | `--effort` cannot be honoured by the Gemini protocol | Refused | one thinking control that holds across model generations |
| [GAP-034](#gap-034) | A verifier can only inspect the shape of a value | Absent | a verifier kind with effects |
| [GAP-035](#gap-035) | Two runs sharing one memory store are not made safe | Refused | a lock, or a per-field merge with a conflict model |
| [GAP-036](#gap-036) | A generated Python program cannot be resumed | Refused | a node walker in the generated code, at the cost of its readability |
| [GAP-037](#gap-037) | A remote tool server cannot be used under a boundary | Refused | a channel for a tool call out of a contained run |
| [GAP-038](#gap-038) | No backend outside this repository has ever run the suite | Unproven | somebody else's backend, and what they hit |
| [GAP-039](#gap-039) | No agent outside this repository is known to run | Unproven | a program somebody depends on, and its friction |

---

## Unenforced

These are the ones that can mislead: a place where reading the source would lead
you to believe something the toolchain does not check.

**There are none.** GAP-001 was the last, and it closed on 2026-08-10. That is
worth saying rather than deleting the heading — the class exists, this project
has had entries in it, and an empty section is a claim that can be falsified
next week.

The **Unproven** section below was empty on the same terms until 2026-08-13, and
is not any more. An empty class is a claim, and this one turned out to be
false.

---

## Refused

These stop the run and say what they could not do. They limit what you can
build; they do not mislead you about what you built.

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

*Recorded in.* [README](guide/the-toolchain.md#authoring-with-a-model), the generated
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

### GAP-034

**A verifier can only inspect the shape of a value.**

*The narrower half of [GAP-030](#gap-030), which closed by giving verifiers a
body.* A verifier body is a pure expression, so a check can test fields, lengths
and thresholds:

```ingot
verifier MinSources(d: draft, min: int) = len(d.sources) >= min
```

It cannot read prose, call a tool, or reach the network. The register's own
motivating example — *does this markdown draft cite eight distinct sources?* —
is still not expressible, because counting citations in prose is not something
an expression over fields can do.

*How it shows up.* You restructure: ask the model for a record with a `sources`
field, verify the record, emit its markdown field. That is a better program and
a real cost — the model now has to answer in a structured shape, which needs
`structured_output` and gives it less room. Where restructuring is not possible,
the property stays unchecked.

*Why not yet.* Reach is what the [RFC-0017](../rfcs/0017-a-verifier-that-runs.md)
design deliberately deferred, and the rule it left behind is the hard part: a
`verified` outcome must be derivable from the run record alone. A tool-backed
verifier can satisfy that, because Cassette 0.2 records tool calls. A
model-graded one cannot without making a verdict depend on a file beside the
artifact — and would not be deterministic, which is what
[Language 0.1 §5.2](../specs/language/v0.1.md) defines a verifier to be.

*What closing it needs.* A verifier kind with effects: how it declares them, how
they are checked at the `verify` site against the agent's policy, and how its
outcome stays reproducible under replay.

*The workaround.* A `tool` call, whose result is a value the flow can then check
with a verifier. That splits the reaching from the deciding, which is the shape
the eventual design is likely to keep.

*Recorded in.* [Runtime 0.4](../specs/runtime/v0.4.md),
[Language 0.2 §10.1](../specs/language/v0.2.md).

### GAP-036

**A generated Python program cannot be resumed.**

The reference interpreter stops at a resumable `checkpoint` and continues from a
snapshot. `ingot build --target python` does not, and its portability report
says so on every build that contains a checkpoint.

*Why not.* The backend emits a **straight-line program**: an agent's flow becomes
statements in a function, and a top-level node becomes a line. There is no node
walker to hand a `resumeAt` to. Re-entering the middle would mean emitting a
dispatch table over every top-level node — which is a node walker, written in
Python — or generators, which cannot be serialised. The first gives up the
readability that is the generated backend's entire reason to exist.

*What you get instead.* The checkpoint's event, in the right place in the
stream. `--stop-at` is not a flag a generated program has, so nothing is
silently ignored.

*Recorded in.* [RFC-0018](../rfcs/0018-state-that-outlives-a-run.md),
`crates/ingot-backend-python/src/report.rs`.

### GAP-037

**A remote tool server cannot be used under a boundary.**

`--sandbox` and `--contained` both refuse a `[[mcp.server]]` that carries a
`url`, naming the server.

*Why.* `--sandbox` bounds a process this machine starts, and a remote server is
not one — there is nothing to put inside a boundary. `--contained` puts the
interpreter in a box whose network is denied, and the supervisor channel carries
a model call and an approval gate; there is no channel for a tool call to cross.

*Why refused rather than degraded.* Connecting anyway would report a boundary
that covers nothing, which is the one outcome worse than not offering the flag.

*What closing it needs.* For `--contained`, a tool call on the supervisor
channel — which is the same shape as [GAP-023](#gap-023) but not the same
problem: that one is about a boundary that could exist and does not, this one is
about a hop with no channel. For `--sandbox` there may be nothing to close: the
server is somebody else's machine, and bounding it is not this toolchain's to
do.

*Recorded in.* [MCP binding 0.2 §5](../specs/tools/mcp-v0.2.md),
[RFC-0019](../rfcs/0019-a-tool-server-that-is-not-a-child-process.md).

### GAP-035

**Two runs sharing one memory store are not made safe.**

Each run writes the whole document when it ends, so two runs against the same
store interleave and the second to finish wins outright. There is no lock and
no detection.

*Why not yet.* The fix is not small. A lock file brings every staleness
question with it — what happens when a run is killed, how long a stale lock is
honoured, whether a reader blocks — and per-field merge needs a conflict model
the language does not have. Neither is worth designing before anybody has hit
the problem.

*Why it is survivable.* The default store path is per-agent and under the build
directory, so reaching the collision takes two runs of the same agent, at the
same time, in the same project. `--memory <FILE>` makes it reachable
deliberately, which is the case this entry exists to warn.

*Recorded in.* [RFC-0018 §5](../rfcs/0018-state-that-outlives-a-run.md),
`crates/ingot-cli/src/memory.rs`.

---

## Unproven

A claim the project makes that nothing yet demonstrates. Not a limitation — a
limitation is something we know. These are the places where we do not, and where
being wrong would be expensive.

Every milestone is done, which is exactly why this section is no longer empty:
the question stopped being *is it built* and became *does it survive contact
with somebody who did not build it*. Two entries, and they are the same shape.

### GAP-038

**No backend outside this repository has ever run the conformance suite.**

Both backends the suite tests were written by the people who wrote the suite,
against the specifications they also wrote. The claim it exists to support —
*somebody else can implement Agent IR from the specification, and find out where
they are wrong* — has not been tested by anybody else.

*Why it is not nothing.* The suite has already earned its keep. Its first run
across the two shipped backends found the reference writing `response_type`
where the specification and the second backend said `responseType`, and the four
cases added for M8 found three more bugs. Two implementations catch what one
cannot. But two implementations sharing a set of assumptions catch less than
they look like they do, and nobody outside has stress-tested the assumptions.

*What closing it needs.* One backend somebody else wrote, and an honest account
of what they hit. The likely findings are not bugs in the cases — they are
places the specification is silent, or clear only to somebody who already knew
the answer.

*One data point, a day later.* This entry was written on 2026-08-13. On
2026-08-14, preparing the crates.io publish, the first question asked on behalf
of somebody outside — *what does a person who runs `cargo install` actually
get?* — found that they would have got a binary with **no cases in it**. The
suite was embedded from `specs/conformance`, two directories above the crate,
and a package carries nothing above itself. The build script read a directory
that was not there, which is not an error, and the binary would have shipped
looking complete.

Nothing in this repository could have caught it, because everything in this
repository is a checkout. That is the whole entry in one bug: the failure was
not in the cases or the specification, it was in the distance between here and
somebody else, and it was invisible from this side. Fixed by moving the cases
into [`ingot-conformance`](../crates/ingot-conformance/README.md) — but the
finding is the point, not the fix.

*Recorded in.* [`crates/ingot-conformance/README.md`](../crates/ingot-conformance/README.md),
[the backend author's guide](guide/writing-a-backend.md).

### GAP-039

**No agent outside this repository is known to run.**

Every `.ing` program here was written to exercise the compiler. They are
examples, and they are honest ones — they compile, they run, their cassettes
replay. None of them is a thing anybody depends on.

*Why it matters more than it sounds.* The gap register records limitations the
project *knows about*. A program written to exercise a compiler finds the
limitations the compiler's author thought to look for. The friction that stops
somebody using a language is usually not on this list, because nobody has hit
it yet.

*Why it is stated rather than fixed.* Writing a program and calling it a user is
not evidence, and would be the least honest entry in this file. The only thing
that closes this is somebody depending on one.

*Recorded in.* [`examples/`](../examples/), [`docs/vision.md`](vision.md).

---

## Closed

When a gap closes it moves here with the release that closed it, and its section
stays where a link can find it. Identifiers are never reused: a link to GAP-007
must keep meaning what it meant.

### GAP-007

**MCP over stdio only.**
*Closed 2026-08-12.*

Every tool server had to be a program on the same machine. An organisation
running a hosted MCP server could not point Ingot at it, and the workaround was
a local proxy process somebody had to write, deploy and keep alive beside every
runner.

*What closed it.* A `[[mcp.server]]` may carry a `url` instead of a `command`,
spoken to over Streamable HTTP.

*The question that had to be answered first.* This gap was a decision, not an
omission. [ADR-0005](adr/0005-mcp-over-stdio-only.md) refused the transport
because reaching a remote server is network access the language could not
express, and ruled out the easy fix in advance: an artifact carrying
`network deny` that nonetheless ships every tool argument to a vendor over TLS
is an artifact whose policy is a lie, and the operator having chosen the vendor
does not make the agent's declaration true.

So the endpoint stays in the manifest — the artifact must keep naming no
server — and the server's **host** is checked against the calling agent's own
`network` grant before anything connects. `network deny` means no remote server
at all. No new effect, no new policy subject: `PolicySubject` maps one-to-one
onto `Effect`, and a `tool_endpoint` subject would have had no effect to pair
with, because no tool would ever declare one.

*The cost, which is real and is the design telling the truth.* The same artifact
needs a wider policy to be served remotely than locally. An agent whose only
tool reads files needs `network allow ["mcp.example.com"]` in its source to use
a hosted server — because serving that tool remotely does put its arguments on
the network.

*What did not change.* Nothing above the transport. `McpClient` still writes a
line and reads a line; the handshake, the routing and the result conversion are
the code that already existed. That is why `Transport` was a trait from the
first commit, and it is the same discipline the streaming work settled on: one
parser, two transports.

*What it deliberately does not do.* No retry on `tools/call` — it is not
idempotent, so a server that sent mail and failed to answer must not be asked
twice. No deprecated HTTP+SSE transport. No use under `--sandbox` or
`--contained`, which is [GAP-037](#gap-037). The `http` support is behind the
CLI's `remote-tools` feature, so a build that hosts only local servers carries
no TLS stack for it.

*Recorded in.* [RFC-0019](../rfcs/0019-a-tool-server-that-is-not-a-child-process.md),
[MCP binding 0.2](../specs/tools/mcp-v0.2.md),
[ADR-0005's amendment](adr/0005-mcp-over-stdio-only.md),
`crates/ingot-mcp/src/http.rs`.

### GAP-008

**`checkpoint` cannot be resumed from.**
*Closed 2026-08-12.*

`checkpoint "sources-collected"` lowered to a node that emitted an event and
nothing else. [Runtime 0.1 §5](../specs/runtime/v0.1.md) said so outright —
*"Emit an event. Resumption is not defined in 0.1"* — so the keyword was named
for something it did not do.

*What closed it.* `--stop-at <LABEL>` writes a snapshot and `--resume <FILE>`
continues from it. The snapshot is a JSON document holding the inputs, the
bindings, working memory, the outputs so far and the counters.

*The decision that shaped it.* **Only a checkpoint at the top level of a flow is
resumable.** One inside a branch arm or a loop body is reached with a partially
unwound interpreter, and resuming into it would mean serialising a continuation
— which is not a file a person can read, and readability is the property the
rest of the design rests on. The compiler marks each checkpoint `resumable`, and
`--stop-at` on a nested one is refused with the reason rather than running to
completion and quietly never stopping.

This is a restriction, not a staging post. Lifting it means choosing a
continuation format, which is a different design that should argue for itself.

*The test that makes it real.* The events of the two halves, with the framing
events removed, concatenate to exactly the events of one uninterrupted run —
byte for byte, since events carry no clock. It is
[Runtime 0.5 §2.5](../specs/runtime/v0.5.md) and it is an executable assertion
in `crates/ingot-cli/tests/resume.rs`.

*Two refusals with no override.* An artifact that changed since the run stopped
is refused: continuing against a modified program produces a result that is
neither program's. Supplying different inputs alongside a resumption is refused
for the same reason.

*What it does not do.* The generated Python backend cannot resume — see
[GAP-036](#gap-036). A contained run cannot stop, because the supervisor channel
reports a finished run or a failed one and stopping is not one of its outcomes.

*Recorded in.* [RFC-0018](../rfcs/0018-state-that-outlives-a-run.md),
[Agent IR 0.2 §7.3](../specs/ir/v0.2.md),
[Runtime 0.5 §2](../specs/runtime/v0.5.md),
`crates/ingot-runtime/src/snapshot.rs`.

### GAP-014

**No persistent memory or state migration.**
*Closed 2026-08-12.*

`memory { working ephemeral { … } }` was the only form, and state lived for one
run. An agent that read a page today could not know tomorrow that it already
had.

*What closed it.* A `persistent { … }` block, addressed by `memory.`, and a
store on disk that carries the declaration it was written under.

Three decisions did the work, and each is the answer to a question the register
did not ask:

* **A second root, not a lifetime flag.** `state.x` and `memory.x` are told
  apart at every use site, because a write that outlives the run is a different
  act from a write to a scratchpad.
* **Every persistent field declares a literal initial value.** That removes
  "read before written" from persistent memory entirely, rather than making
  every author guard against the first run at every read site.
* **The store carries the full declaration, not a digest.** A digest can only
  say no. A changed declaration is refused with a per-field diff — added,
  removed, retyped — and `--migrate-memory` keeps what still matches, drops the
  rest, and says what it dropped even under `--events quiet`.

*Migration, which the gap's title also asked for.* A store written under a
different declaration is refused by default. `--migrate-memory` is the way
through, and it loses data loudly rather than reinterpreting a stored value as a
new type.

*Both backends.* The reference interpreter and the generated Python program read
and write the same format, so a store written by one is readable by the other.
The `memory-initial` conformance case holds them to the same seeding behaviour.

*What it deliberately is not.* Not a database, not a shared key-value store, and
not safe against concurrent writers — see [GAP-035](#gap-035). A store over
4 MiB is refused on open, because a store that large is an agent accumulating
without bound into a document rewritten in full every run.

*Recorded in.* [RFC-0018](../rfcs/0018-state-that-outlives-a-run.md),
[Language 0.2 §11](../specs/language/v0.2.md),
[Agent IR 0.2 §7](../specs/ir/v0.2.md),
[Runtime 0.5 §3](../specs/runtime/v0.5.md),
`crates/ingot-cli/src/memory.rs`.

### GAP-017

**There was no conformance suite and no backend author guide.**
*Closed 2026-08-12.*

[Runtime 0.1 §13](../specs/runtime/v0.1.md) said what a conforming backend must
do, and there was no way to find out whether yours did. The interpreter's own
tests were the closest thing, and they were not packaged for anybody else.

*What closed it.* `ingot conform`, a suite of seven cases in
[`crates/ingot-conformance/`](../crates/ingot-conformance/README.md), and
[a guide](guide/writing-a-backend.md).

A backend under test is a **command**: the suite writes a request file, runs the
command with it, and compares the event stream, the artifacts and the outcome
against what the case requires. Each case names the clause it enforces, so a
failure says what to read rather than only that something differs.

*The part that makes it real.* The reference interpreter is not privileged — it
reaches the suite through the same adapter a third party writes. Both shipped
backends are held to the same seven cases by the same code, and `conformance.rs`
keeps it that way. The first run across both **found a real divergence**: the
reference was writing `response_type` where the specification and the second
backend both said `responseType`. On an enum, serde's `rename_all` renames the
variants and not their fields, and no single-implementation test could have seen
it.

*What is still missing, and known to be.* No case covers a tool call, a
sub-agent, a policy denial at run time, a budget being exhausted, or a
`checkpoint`. The suite's own README says so: a suite that implied coverage it
does not have would be worse than a small one.

*Recorded in.* [`crates/ingot-conformance/README.md`](../crates/ingot-conformance/README.md),
[the backend author's guide](guide/writing-a-backend.md),
`crates/ingot-cli/src/conform.rs`,
`crates/ingot-cli/tests/conformance.rs`.

### GAP-032

**The Python backend did not stream, so the two backends accepted different
answer lengths.**
*Closed 2026-08-12.*

The Python prelude made one whole-body request per `ask` and kept the
16,000-token ceiling [Runtime 0.3 §4](../specs/runtime/v0.3.md) reserves for
that transport, while the reference interpreter asked for up to 64,000 against a
provider that streams. An answer between the two completed on one backend and
ended the run with a truncation error on the other — a portability difference in
the one place the project's central claim lives.

*What closed it.* An event-stream reader in the prelude, and both providers
given `streams()` and `complete_streaming()`. The ceiling now comes from asking
the provider, so the generated program no longer names a number at all: Runtime
0.3 §4 forbids an artifact selecting its own ceiling, and the emitter used to
write one into every `ask`.

*The part worth keeping.* **One parser, two transports.** Each accumulator's
only job is to rebuild the payload a whole-body call would have returned —
`stop_reason`, `content`, `usage`, down to the field names — and hand it to the
same reader. There is deliberately no second parser, which is what makes
"a streamed call and a whole-body call produce identical values *and* identical
errors" structural rather than tested-in. `prelude_streaming.rs` drives the
accumulators without a socket and asserts exactly that.

*What this did not change.* A delta is still not an event
([Runtime 0.3 §2](../specs/runtime/v0.3.md)), so the event streams agreed before
and agree now for the same reason. Cassette replay reports `streams() == False`:
a recording produces its answer at once, and a replay that invented plausible
fragments would be indistinguishable from a call that never happened.

*Recorded in.* [Runtime 0.3 §4](../specs/runtime/v0.3.md),
[RFC-0013](../rfcs/0013-streaming.md),
`crates/ingot-backend-python/src/prelude.py`,
`crates/ingot-cli/tests/prelude_streaming.rs`.

### GAP-025

**The product loop is a sequence of commands with no single surface.**
*Closed in 0.4.0.*

`ingot studio` serves one page over the reports the other commands print:
projects, a project's diagnostics, readiness, boundary and agents, its run
history, and what this machine can reach. Specified in
[RFC-0015](../rfcs/0015-ingot-studio.md).

*Why this is not the surface [RFC-0007](../rfcs/0007-the-ingot-product-loop.md)
refused.* That non-goal names "a hosted no-code workflow editor as the source of
truth", and the objection was that a surface built then would have invented the
semantics the language lacked. This one cannot invent anything:
[`crates/ingot-studio`](../crates/ingot-studio/) has no dependencies and no
compiler, and receives everything it shows through one trait the CLI implements
by calling `doctor::report`, `sandbox::plan_all`, `compile_path` and the
language service — the same functions the subcommands call. The tests are
equalities against `ingot doctor --json` and `ingot check` rather than
assertions about the page's own arithmetic.

*The one new thing it needed.* A run could not be re-derived after the terminal
scrolled, so `ingot run` now writes `<out-dir>/runs/<id>.jsonl`: the JSON event
stream verbatim, wrapped in two `record` lines carrying the wall clock, the
process id and the outcome. Wall clock lives there and not in an event, because
[Runtime 0.1 §9](../specs/runtime/v0.1.md) requires a replay to reproduce the
event sequence byte for byte. `--no-history` writes nothing.

*Starting a run.* The page offers the agents the artifact declares with a field
per input it takes, and spawns the same command a person would type. A *launch*
is tracked apart from a record because a record only exists once the interpreter
reaches `runStarted`: a child that fails while compiling writes none, and a
button that appears to have done nothing is worse than an error. `--yes` and
`--no-history` are not fields the page has, and an unknown one is refused rather
than ignored.

*What it deliberately does not do.* Edit a program or edit a manifest.
Connecting a model service still means writing `[[model.provider]]` by hand and
naming the variable it reads; the page shows the block and where it goes.
Writing it would mean re-serializing a hand-written manifest and losing its
comments, which is the same mistake as regenerating source from a diagram — and
solving it properly is the canvas's problem, not this one's.

*Recorded in.* [Vision](vision.md#one-product-loop-around-the-language),
[RFC-0015](../rfcs/0015-ingot-studio.md),
`crates/ingot-studio/tests/server.rs`, `crates/ingot-cli/tests/studio.rs`.

### GAP-001

**A policy's host allowlist is not enforced.**
*Closed in 0.4.0.*

`network allow ["arxiv.org"]` now bounds a contained tool server to that host.
A request to anywhere else is refused, from inside the box, and there is a
container test that makes the request and watches it fail.

*The arrangement.* The server joins a container network created `--internal`,
which has no route out. A proxy — [`crates/ingot-egress`](../crates/ingot-egress/),
one dependency-free binary in [its own image](../tools/egress.Dockerfile) —
joins that network and an ordinary one, so it is the only thing on the internal
side that can reach anything. `HTTP_PROXY` points the server at it.

*Where the enforcement actually lives.* In the network, not the variable. A
server that reads `HTTP_PROXY` goes through the filter; a server that ignores it
reaches **nothing**, because there is nowhere for its packets to go. That is the
difference between this and setting an environment variable and hoping, and it
is the third of the three container tests: unset every proxy variable inside the
box and the request still fails.

*The four ways this is written wrongly, and what stops each.* DNS rebinding —
the client never resolves anything, so the check and the connection cannot
disagree. Address literals — refused as their own kind of refusal, because a
policy grants names. TLS SNI against the Host header — neither is read; a
`CONNECT` tunnel goes to the address the proxy dialled, and for plain HTTP the
request target decides. A granted name pointing inward — every resolved address
is checked against loopback, link-local, private and carrier-grade ranges, with
`169.254.169.254` in the list by name. Each has a test that connects a real
socket.

*What it costs.* The proxy image must be present, and `ingot run --sandbox` says
so and falls back to reporting the allowlist as unenforceable when it is not —
the plan and the arrangement agree, because the plan is made after asking. One
proxy serves a whole run, so its list is the union of every agent's grant; each
agent is still bounded to its own by the compiler ([GAP-013](#gap-013)). The
boundary is Linux containers, as it already was ([GAP-020](#gap-020)).

*What is still true and was not before.* Nothing in this project's register sits
in the Unenforced class. That is the class that could mislead, and it is empty.

*Recorded in.* [`crates/ingot-egress`](../crates/ingot-egress/),
[`crates/ingot-sandbox/src/egress.rs`](../crates/ingot-sandbox/src/egress.rs),
[`crates/ingot-sandbox/tests/container.rs`](../crates/ingot-sandbox/tests/container.rs),
[RFC-0004](../rfcs/0004-ingot-containers.md),
[Language 0.1 §7.1](../specs/language/v0.1.md).

### GAP-013

**A capability cannot be scoped to an endpoint or a path at the call site.**
*Closed in 0.4.0 (Language 0.2 §9).*

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
*Closed in 0.4.0 (Runtime 0.3).*

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

*What was deliberately not done at the time.* A contained run does not stream
and keeps the 16k ceiling ([GAP-031](#gap-031)); the Python backend did not
stream either, so the two backends accepted different answer lengths — that was
[GAP-032](#gap-032), and it has since closed. Deltas carry no determinism
guarantee, which is what "not an event" means.

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
*Closed in 0.4.0 (Runtime 0.2).*

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
*Closed in 0.4.0 (cassette 0.2).*

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
*Closed in 0.4.0.*

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
[README](guide/the-toolchain.md#putting-the-agent-in-the-box-too),
`crates/ingot-supervisor/src/host.rs`.

### GAP-002

**`verify` reported a check that never ran.**
*Closed in 0.4.0 (Runtime 0.2).*

The event carried `passed: true` for a node nothing had performed. A boolean can
describe two states and there are three, so the field was replaced rather than
extended: `outcome` is `notPerformed`, `passed` or `failed`, and `passed` is
gone. Leaving it present and adding `performed` beside it would have kept the
misleading reading available and made the honest answer the one you get by
reading two fields in the right order.

The compiler now says it earlier too: `ING6006` warns at `ingot check` rather
than leaving the gap to be discovered in an event stream after the fact.

*What this did not close.* Verifiers still could not be executed —
[GAP-030](#gap-030) was that half, and it closed later. What this change did was
stop the run claiming otherwise.

*Recorded in.* [Runtime 0.2](../specs/runtime/v0.2.md),
[`ingot explain ING6006`](../crates/ingot-diagnostics/src/codes.rs).

### GAP-030

**A verifier could not be executed at all.**
*Closed in Runtime 0.4 / Language 0.2.*

A verifier was a name and a signature. It parsed, type-checked and reached the
IR, and nothing could carry the check out: every `verify` reported
`notPerformed`, so a declared property was documentation with a type signature.

The register said this needed a design first, because "a verifier is either a
tool call, a model call with a rubric, or host-provided code". It turned out the
language specification had already eliminated one of those —
[Language 0.1 §5.2](../specs/language/v0.1.md) calls a verifier a *deterministic*
check, which a model with a rubric is not — and the other two differ only in
where the check's code lives. [RFC-0017](../rfcs/0017-a-verifier-that-runs.md)
put it inside the artifact:

```ingot
verifier MinSources(d: draft, min: int) = len(d.sources) >= min
```

The body is a pure `bool` expression, inlined at each `verify` site as the
node's `condition` — the same field and the same value forms a `branch` already
uses, so no IR schema change and no new evaluator construct in either backend.
A failing check ends the run, after emitting its `failed` event; `ING6007` warns
when a `verify` comes after the `emit` of what it checks, where it could not
have prevented anything.

The rule the design left behind is the part that outlives it: **a `verified`
outcome must be derivable from the run record alone**. That is what keeps a
replay byte-exact, and what a later verifier kind with reach has to satisfy.

*What this did not close.* A check can only inspect the shape of a value —
[GAP-034](#gap-034) is that half.

*Recorded in.* [Runtime 0.4](../specs/runtime/v0.4.md),
[Language 0.2 §10](../specs/language/v0.2.md),
[Agent IR 0.2 §5](../specs/ir/v0.2.md).

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

*It cost one name, 2026-08-14.* "Costs nothing" was very nearly right and not
quite. Preparing the crates.io publish found that the same owner also holds
**`ingot-types`** — the crate this project's type, effect and capability model
was called. Every other name in the workspace was free. Renaming the package to
`ingot-lang-types` while keeping `[lib] name = "ingot_types"` moved no Rust at
all, so the cost really was one line in a manifest; but it is worth recording
that a neighbouring name in an occupied namespace is not free by default, and
the next one may not be this cheap. This does not reopen the entry: the decision
it records was about a trademark search, and nothing here bears on that.

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
*Closed in 0.4.0 (M6).*

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
*Closed in 0.4.0 (M6).*

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
*Closed in 0.4.0 (IR 0.2).*

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
*Closed in 0.4.0 (M11).*

`ingot image build` now finds the Ingot source checkout, verifies its workspace
version against the running binary, and builds the shipped recipe under the
exact `ingot/run:<cli-version>` tag. `ingot run --contained` selects that local
image when no deliberate `[run] image` or `--image` override exists, and
`ingot doctor` reports its readiness with the same command as the fix.

The command does not weaken the supply-chain boundary: it never downloads an
image, custom images remain explicit operator choices, and a missing runtime or
image never falls back to a host run. M6 added digest pinning for an image
reference; signed acquisition is separately open as [GAP-029](#gap-029).

*Recorded in.* [README](guide/the-toolchain.md#putting-the-agent-in-the-box-too),
[CHANGELOG](../CHANGELOG.md),
[RFC-0007](../rfcs/0007-the-ingot-product-loop.md).

### GAP-018

**The IR had one consumer, so the portability claim was undemonstrated.**
*Closed in 0.4.0 (M5).*

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
