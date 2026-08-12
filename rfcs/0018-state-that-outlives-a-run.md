# RFC-0018: State that outlives a run

- Status: **Accepted**
- Author(s): Heptapus Group
- Created: 2026-08-12
- Affects: language, IR, runtime, CLI

## Problem

Two entries in the gap register describe the same missing thing from opposite
ends.

[GAP-014](../docs/gaps.md#gap-014) — **no persistent memory**. This is the whole
of the memory grammar:

```ingot
memory {
  working ephemeral { notes: string[] }
}
```

`ephemeral` is the only lifetime, and `SUPPORTED_MEMORY_LIFETIMES` is a
one-element list. An agent that reads a page today cannot know tomorrow that it
already read it.

[GAP-008](../docs/gaps.md#gap-008) — **`checkpoint` cannot be resumed from**.
This is the whole of `checkpoint`:

```rust
NodeKind::Checkpoint => {
    self.sink.emit(RunEvent::Checkpoint { .. });
    Ok(())
}
```

The specification is candid about it. [Runtime 0.1 §5](../specs/runtime/v0.1.md)
says, in the row for `checkpoint`: *"Emit an event. Resumption is not defined in
0.1."* The keyword is named for something it does not do.

The register asks for two designs — "a memory model (RFC)" and "a resumption
model (RFC)". This is one RFC because they are one mechanism.

## What the two have in common

Strip the wrapping off each.

Persistent memory needs to take some of a run's state, write it somewhere, read
it back at the start of a later run, and refuse when what it reads back does not
belong to the program reading it.

Resumption needs to take **all** of a run's state, write it somewhere, read it
back at the start of a later run, and refuse when what it reads back does not
belong to the program reading it.

The difference is *how much* and *how often*, not *what*. Both are a snapshot,
both need an identity check on the way back in, and both fail the same way when
that check fails. Building them twice would give Ingot two snapshot formats, two
identity rules, and two chances to get the refusal wrong.

So this RFC defines one thing — a **snapshot** — and two uses of it.

| | Persistent memory | Resumption |
|---|---|---|
| Holds | the fields declared `persistent` | every binding, all state, the outputs so far, the counters, and where to continue |
| Identity | the declared shape of those fields | the digest of the whole artifact |
| Lifetime | read and rewritten by every run | written once, consumed once |
| On mismatch | refuse, with a diff, and offer migration | refuse; there is nothing to migrate to |

The asymmetry in the last row is the one interesting difference, and §6 argues
it rather than assuming it.

## Goals and non-goals

**Goals.**

* A `persistent` memory block whose fields survive between runs.
* A `checkpoint` that a run can be stopped at and later continued from, such
  that the two halves produce exactly the events one uninterrupted run would.
* A refusal, in both cases, that names what changed instead of guessing.
* Everything that crosses between runs is a file a person can read.

**Non-goals.**

* **A database.** A snapshot is a JSON document. An agent that needs indexed
  queries over a large corpus wants a tool, and a tool is what `!filesystem_read`
  and MCP are for. Persistent memory is for the handful of facts an agent
  carries forward, and if it grows past what fits comfortably in one document,
  the design was wrong and not the file format.
* **Concurrent writers.** One run owns a store for its duration. Two runs
  sharing one store is not made safe here and is not detected here; §5 says so
  plainly rather than implying a lock that does not exist.
* **Resuming into a nested region.** §4.1 restricts resumable checkpoints to the
  top level of a flow, and argues why that is a design choice and not a
  limitation to be lifted later.
* **Resumption in the generated Python backend.** §7 explains why a generated
  straight-line program cannot be re-entered in the middle, and what the support
  report says about it.

## 1. The snapshot

Both uses share a header:

```json
{
  "ingotSnapshot": "0.1",
  "kind": "memory",
  "agent": "research.Report"
}
```

`kind` is `memory` or `resumption`. Reading a snapshot of the wrong kind is an
error naming both kinds, because pointing `--resume` at a memory store is a
plausible mistake and "unexpected field" would not explain it.

Every snapshot is written with sorted keys and a trailing newline, for the same
reason the IR is: a file that two identical runs produce differently is a file
that cannot be diffed.

## 2. Persistent memory: syntax

```ingot
memory {
  working ephemeral { draft: markdown }
  persistent { seen_urls: string[] = [] }
}
```

`persistent` is a sibling of `working ephemeral`, not a second lifetime keyword
after `working`. Writing `working persistent` would have been a smaller grammar
change and it is rejected here on the grounds that it is a contradiction:
working memory is the scratchpad for one run, and a block that outlives the run
is not working memory however it is spelled.

### 2.1 Persistent fields are addressed by `memory.`

```ingot
flow {
  if not contains(memory.seen_urls, url) {
    page = fetch(url)
    memory.seen_urls = append(memory.seen_urls, url)
  }
}
```

Ephemeral state stays `state.`. Persistent state is `memory.`. The two are
distinguishable at every use site.

The alternative — one `state.` root, with the lifetime discoverable only by
scrolling up to the declaration — was rejected. A write to persistent memory is
a side effect that outlives the run that performed it, and the language already
takes the position that consequences should be legible where they happen. This
is the same argument that gave `verify` three outcomes instead of a boolean and
kept a streaming delta out of the event stream: the distinction that matters is
the one you can see without looking anything up.

### 2.2 Every persistent field declares an initial value

`= []` above is not optional.

Ephemeral state has a coherent "not yet written" state, and reading it is an
error ([Runtime 0.1 §5](../specs/runtime/v0.1.md): *"Reading before writing is an
error, not `null`"*). Persistent state does not: on the first run there is
nothing, on every later run there is something, and an agent that had to guard
every read against the first run would be writing the initial value anyway —
once per read site, in the flow, where it can be got wrong.

So the initial value is declared once, in the type, and the first run starts
from it. `ING3013` when it is missing.

The initial value must be a **literal**. Not because evaluating an expression
would be hard, but because the initial value has to be in the artifact: a run
resolves it before any node executes, so there is no evaluation context for it
to depend on, and an expression that appeared to have one would be misleading.
`ING3014` when it is not a literal.

## 3. Persistent memory: the store

```json
{
  "ingotSnapshot": "0.1",
  "kind": "memory",
  "agent": "research.Report",
  "shape": { "seen_urls": "string[]" },
  "fields": { "seen_urls": ["https://arxiv.org/abs/2401.00001"] }
}
```

`shape` is the declaration the store was written under. It is stored in full
rather than as a digest, and that is deliberate: a digest can only say *no*,
and the failure this design most needs to handle well is the one where an author
changed a declaration and wants to be told which field.

### 3.1 Where it lives

`<out-dir>/memory/<agent>.json`, beside the run records and for the same
reasons: it is output, it is already ignored by version control, and deleting
the build directory is expected to lose it.

`--memory <FILE>` puts it somewhere durable. `--no-memory` runs against the
declared initial values and discards every write.

An artifact that declares no `persistent` block opens no store, so this affects
exactly the programs that asked for it. Every run that does open one says so on
its first line of output. A store is not a thing that should be discoverable
only by finding the file.

### 3.2 The shape check

On open, the store's `shape` is compared with the artifact's declared persistent
fields.

Equal — load it.

Not equal — refuse, and say which of the three things happened to each field
that differs:

```
error: the memory store was written for a different declaration
  --> target/ingot/memory/research.Report.json
  |
  = added:    depth: int          (no stored value; would start from `0`)
  = removed:  seen_urls: string[] (1 stored value would be dropped)
  = retyped:  cache: string[]     (stored as `int[]`)
  |
  = help: `--migrate-memory` keeps what still matches and drops the rest
  = help: `--no-memory` ignores the store for this run without changing it
```

`--migrate-memory` keeps every field whose name **and** type still match, starts
added fields from their declared initial value, drops the rest, prints exactly
what it dropped, and rewrites the store's `shape`. The dropping is the point: a
migration that silently reinterprets a stored value as a new type is how state
stores corrupt themselves, and this one would rather lose data loudly.

## 4. Resumption

```console
$ ingot run main.ing --stop-at "sources-collected"
  …
  n7   checkpoint
        checkpoint "sources-collected"
stopped at "sources-collected": target/ingot/snapshots/research.Report-sources-collected.json

$ ingot run main.ing --resume target/ingot/snapshots/research.Report-sources-collected.json
```

`--snapshot <FILE>` relocates the file. `--stop-at` names a label, not a node
id: a node id is a compiler artefact and an author who moves a statement should
not have to find the new one.

### 4.1 Only a top-level checkpoint is resumable

A checkpoint inside an `if` arm or a `loop` body is reached with a partially
unwound interpreter: the loop's iteration counter, the arm being executed, and
every enclosing region's position are live state that resuming would have to
reconstruct. Serialising that means serialising a continuation, and a
continuation is not something a person can read — which would give up the
property that everything crossing between runs is inspectable.

So the compiler marks each `checkpoint` node `resumable`, true only at the top
level of the flow. A nested checkpoint still emits its event; it is a marker,
which is all it ever was.

`--stop-at` naming a non-resumable checkpoint is an error that says why and
where the checkpoint is, rather than running to completion and quietly never
stopping. A flag that silently does nothing is worse than one that refuses.

This is a restriction and not a staging post. Lifting it later would mean
choosing a serialised continuation format, and if that is ever wanted it is a
different design that should argue for itself.

### 4.2 What a resumption snapshot holds

```json
{
  "ingotSnapshot": "0.1",
  "kind": "resumption",
  "agent": "research.Report",
  "artifact": "sha256:…",
  "label": "sources-collected",
  "resumeAt": "n8",
  "inputs":   { "topic": "…" },
  "bindings": { "sources": [ … ] },
  "state":    { "notes": [ … ] },
  "outputs":  { },
  "steps": 7,
  "usage": { "inputTokens": 8123, "outputTokens": 4001, "cacheReadTokens": 0 },
  "spend": { … }
}
```

`resumeAt` is the node **after** the checkpoint, so a resumed run does not
re-emit the checkpoint event. §4.4 turns that into a property.

`steps`, `usage` and `spend` carry forward. A budget is a bound on a run, and a
run that stopped and continued is one run: allowing a stop to reset the counters
would make `--stop-at` a way to spend twice what the artifact permits.

Persistent memory is deliberately **not** in here. A resumption snapshot holds
what belongs to the interrupted run; the store belongs to the agent and is read
and written by both halves through §3 as normal.

### 4.3 A changed artifact is refused, with no override

`artifact` is the SHA-256 of the canonical IR — the same bytes the artifact
digest is taken over everywhere else.

If it differs on resume, the run refuses and there is no flag to insist.
Continuing an interrupted run against a modified program produces a result that
is not what either program would have produced, and no one reading the run
record afterwards could tell which parts came from which. This is the same
position [Runtime 0.1 §10](../specs/runtime/v0.1.md) already takes on a cassette
recorded for a different request, and for the same reason.

### 4.4 The property this has to satisfy

> Let `E(r)` be the events of run `r` with `runStarted`, `runFinished`,
> `runFailed` and `runStopped` removed. For an artifact, inputs and cassette
> that produce an uninterrupted run `U`, and a stop at any resumable checkpoint
> producing `A` then `B`:
>
>     E(A) ++ E(B) == E(U)

Byte for byte, since events carry no clock. This is normative, it goes in
Runtime 0.5 §2, and it is the test that makes resumption more than a plausible
story.

### 4.5 A stopped run is not a finished one

A new event:

```json
{"event":"runStopped","node":"n7","label":"sources-collected"}
```

Without it a stopped run and a run that finished having produced nothing look
identical in a record, and the check that every declared output was emitted
would have to be skipped on a guess. `runStopped` is that guess made explicit:
it is the only thing that suppresses the check, and it is in the record where a
reader can see it was suppressed.

## 5. Concurrent writers are not handled

Two runs sharing one memory store will interleave, and the second to finish
wins outright — it holds the whole document.

There is no lock. There is no detection. An operator who runs two agents against
`--memory shared.json` at once gets whichever answer the scheduler produced, and
this RFC does not pretend otherwise.

This is stated rather than fixed because the fix is not small — it is either a
lock file with all the staleness questions that brings, or per-field merge with
a conflict model — and because the default store path is per-agent and under the
build directory, which makes the collision hard to reach by accident. It goes in
the gap register as a **Refused** entry, which is where a known and stated
limitation belongs.

## 6. Why memory migrates and a resumption does not

Both refuse on a mismatch. Only one offers a way through, and the asymmetry is
not an oversight.

A memory store's contents are *the agent's*, accumulated over many runs, and the
declaration changing is an ordinary event in the life of a program that is being
worked on. Losing the accumulation every time a field is added would make
persistent memory unusable during development, which is when it is most used.
The value is worth keeping across a shape change, and which parts survive is
decidable field by field.

An interrupted run's snapshot is *that run's*, it is minutes old, and the
artifact changing under it is not an ordinary event — it means someone edited
and rebuilt the program between the two halves. There is nothing to preserve:
the correct action is to start again, which costs one run. And unlike the memory
case, there is no field-by-field answer, because `resumeAt` names a node id in
a graph that no longer exists.

Migration is offered where there is value to preserve and a rule for preserving
it. Both are absent for a resumption.

## IR semantics

Two additions, both optional and both omitted when empty, so every existing
artifact stays byte-identical and the IR version stays `0.2`.

On the document:

```json
"persistent": {
  "seen_urls": { "type": "string[]", "initial": [] }
}
```

Sorted by field name, like `state`. The `initial` value is already-canonical
JSON of the declared type.

On a `checkpoint` node:

```json
{"id": "n7", "kind": "checkpoint", "label": "sources-collected", "resumable": true, "next": "n8"}
```

`resumable` is omitted when false, which keeps every nested checkpoint —
and every artifact compiled before this RFC — encoded exactly as it is now.

No new node kinds. A read of `memory.x` lowers to `state.read` with
`scope: "memory"`, and a write to `state.write` with the same, reusing the two
nodes that already do this job rather than adding a parallel pair that would
have to be kept in step.

## Target lowering

**The reference interpreter** implements all of it.

**The generated Python backend** implements persistent memory — the store is a
file it opens at the top of `main()` and rewrites at the end, and the shape
check is the same comparison in ten lines of Python.

It does **not** implement resumption, and cannot without changing what it emits.
The backend generates a straight-line Python program: an agent's flow becomes
statements in a function, and a top-level node becomes a line. There is no node
walker to hand a `resumeAt` to. Re-entering such a program in the middle would
mean either emitting a dispatch table over every top-level node — which is a
node walker, written in Python, and would give up the readability that is the
generated backend's entire reason to exist — or generators, which cannot be
serialised.

`ingot build --target python` therefore reports `checkpoint` as **supported for
its event and not for resumption**, with that reason, and the run of a generated
program ignores `--stop-at` because it never sees one. The gap register gets a
**Refused** entry saying so. It is a real limitation of a real design decision,
recorded where limitations go.

## Security and policy impact

**No new effect, and the default-deny rule is untouched.**

The case for making a persistent write an effect is that it is observable
outside the run. The case against, which wins here, is that an effect is a
grant an *artifact* makes to itself and this is a resource the *operator*
supplies: no store exists unless the operator's command line or the build
directory provides one, the agent cannot name a path, and the store's contents
are confined to fields the artifact declared and typed. An effect would be a
second lock on a door the operator is already holding.

Two consequences that are easy to miss and are therefore stated:

* A persistent store is **not** confined by `--sandbox` or `--contained`. Under
  `--contained` the store is a file outside the box, so the interpreter inside
  the box cannot open it. Rather than silently running with an empty one, a
  contained run of an artifact that declares `persistent` refuses and says to
  pass `--no-memory` if that is what was meant.
* A memory store may contain anything the agent put in it, including text a
  model produced from a document a tool fetched. It is written under the build
  directory, which is not published, but it is not scrubbed and it is not
  encrypted. An agent that would put a secret in persistent memory should not.

## Static bounds

Unaffected. `steps`, `tokens` and `cost` carry across a resumption (§4.2), so
an interrupted run is bounded exactly as the uninterrupted one is. A memory
store holds values, never a step count.

One new bound: a memory store larger than **4 MiB** is refused on open, naming
the size. Not because there is an algorithmic problem at 5 MiB, but because a
store that large means an agent is accumulating without bound into a document
that is fully rewritten every run, and finding that out from a slow run months
later is worse than being told at the point it first happens.

## Compatibility

Language moves to **0.2** for the `persistent` block and the `memory.` root.
`language 0.1` sources are unaffected: `persistent` is gated exactly as verifier
bodies are, with `ING1020` naming the feature and the version it needs.

The IR stays at **0.2**. Both additions are optional and omitted when empty, so
every artifact compiled before this RFC encodes byte-for-byte identically, and
the golden IR tests are expected to pass unchanged.

The runtime specification moves to **0.5** for §2 (the resumption property) and
the `runStopped` event. A backend that does not emit `runStopped` is a backend
that cannot stop, which is a legal thing to be.

Nothing changes meaning. `ephemeral` still means what it meant; a `checkpoint`
that nobody stops at behaves exactly as it does today.

## Alternatives

**Do nothing.** The strongest argument for it is that both features can be
approximated: persistent memory with a filesystem tool and a JSON file the agent
manages itself, resumption by splitting the agent into two and passing the first
one's output into the second. Both work. Both also move the guarantees out of
the language — the file has no declared shape and nothing checks it, and the
split agents have two budgets, two policies and two records where the author
meant one run. The register's own framing applies: the workaround is real and
its cost is that the artifact stops describing what happens.

**One `state.` root with a per-field lifetime.** Smaller grammar, smaller
implementation, and it loses the property in §2.1 — that the reader can see
which writes outlive the run. Rejected on legibility.

**A resumption snapshot that also carries persistent memory.** Tempting, because
then one file is the whole world. Rejected because it makes the two halves of an
interrupted run disagree with every other run about where memory comes from, and
because a snapshot is consumed once while a store is not.

**A serialised continuation, so any checkpoint is resumable.** This is the
general version of §4.1 and it is a real design with real precedent. Rejected
for this RFC because the artefact stops being readable, and because the demand
is speculative: the checkpoints in every example in the repository are at the
top level, where the author put them to mark a phase boundary.

**A key–value store with its own effect.** The full version — an agent naming
its own keys, granted `memory_write`, with a store that is not per-agent.
Rejected as premature. It answers questions nobody has asked yet (sharing state
between agents, unbounded key sets) at the cost of the two properties this
design is built on: the shape is declared, and the whole store fits on a screen.

## Conformance tests

- [ ] `memory-persists` — a two-run case: the first writes, the second reads
      what the first wrote.
- [ ] `memory-starts-from-initial` — a first run against no store reads the
      declared initial value rather than failing.
- [ ] `memory-shape-changed` — a store written under a different declaration is
      refused, and the message names the field.
- [ ] `resume-produces-the-same-events` — §4.4, as an executable assertion.
- [ ] `resume-rejects-a-changed-artifact` — §4.3.
- [ ] `stop-at-a-nested-checkpoint-is-refused` — §4.1.
