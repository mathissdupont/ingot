# RFC-0021: A fan-out that overlaps

- Status: **Accepted**, implemented 2026-08-18
- Created: 2026-08-17
- Affects: runtime spec, CLI, `ingot-runtime`
- Closes: [GAP-010](../docs/gaps.md#gap-010)
- Opens: [GAP-043](../docs/gaps.md#gap-043)
- Builds on: [RFC-0002](0002-runtime-execution-model.md),
  [RFC-0013](0013-streaming.md)
- Specifies: [Runtime 0.6](../specs/runtime/v0.6.md)

## Problem

`parallel map` runs its iterations one after another.

```ingot
summaries = parallel map documents as document {
  ask<summary>("Summarise ${document}.")
}
```

Ten documents, ten model calls, and the tenth starts when the ninth finishes.
Each call is thirty seconds of waiting on a socket, so the node takes five
minutes to do five minutes of nothing. The word in the source says the
iterations are independent — the compiler proves it, refusing a state write, an
`emit`, a `checkpoint` or a `consult` inside the body with `ING6005` — and the
runtime then declines to use what it was told.

**Nothing here is misleading, which is why it is Degraded rather than
Unenforced.** [Runtime 0.1 §5.1](../specs/runtime/v0.1.md) says the node marks
an opportunity for concurrency rather than an obligation, and that conformance
asserts the result and never the schedule. The result of a sequential fan-out
is the result of a concurrent one. Only the wall clock differs.

But the wall clock is the whole reason the construct exists. An author who
writes `parallel map` and gets sequential execution has written a comment.

### Why this was not done sooner

Because the honest version is not "spawn some threads". Concurrency makes the
order of model and tool calls nondeterministic, and this project has spent
several RFCs making sure that a run is reproducible. Three things depend on
that order today, and a fourth on exclusive access:

- **Replay matches by position.** `ReplayProvider`, `ReplayToolHost` and
  `ReplayConsultations` each hold a cursor and take the next recorded row.
  Concurrent calls arriving in a different order than they were recorded would
  make `ingot test` flaky — the one thing a test may never be.
- **The event stream is compared across backends.**
  `the_event_streams_agree_on_kind_and_order` asserts that the reference
  interpreter and the Python backend emit the same events in the same order.
  The Python backend is straight-line and sequential. A reference interpreter
  that emitted in arrival order would diverge from it, and the divergence would
  be in the exact place this project's central claim lives.
- **The budget is charged against one counter.** `charge_step` increments
  `self.steps` and ends the run when it passes the ceiling.
- **`ModelProvider::complete`, `ToolHost::call` and `EventSink::emit` all take
  `&mut self`,** and none of the three carries a `Send` bound. Rust therefore
  refuses, at compile time, to let any of them cross a thread boundary. This is
  not an oversight to be patched around; it is the type system stating the
  current design accurately.

[GAP-010](../docs/gaps.md#gap-010) already names the first, second and fourth of
those, and says the entry "should not be started by someone who believes the
shorter sentence". This RFC is the longer sentence.

## Goals and non-goals

**Goals**

- Iterations of one `parallel map` overlap in wall-clock time.
- The event stream is **byte-identical** to the sequential one, for the same
  inputs, on every run.
- `ingot test` stays deterministic: a cassette recorded under one schedule
  replays under any other.
- A recorded cassette is byte-identical run to run, so `--record` produces a
  file somebody can review and diff.
- The cost of a failing run does not depend on the schedule.
- The change to `ingot-runtime`'s public traits is the smallest one that works.

**Non-goals**

- **Concurrency anywhere else.** A top-level flow is a sequence and stays one.
  `loop` is sequential by definition — its guard reads what the last iteration
  wrote.
- **Concurrent tool calls.** See *What does not overlap*, below. Model calls are
  where the wall clock goes; tool servers are child processes with handshakes
  and, in the `--allow-write` case, with state.
- **Async.** Replacing the blocking HTTP layer with an async runtime is a
  larger change than this one and buys the same thing. Rejected in
  *Alternatives*.
- **A guarantee of concurrency.** Runtime 0.1 §5.1 stays exactly as written:
  the node marks an opportunity. A backend that runs the body sequentially — the
  Python one does, and will keep doing — remains conforming.
- **Speed as a promise.** Nothing in the artifact, the IR or the conformance
  suite asserts that anything got faster. What is asserted is that nothing else
  changed.

## What overlaps, and what does not

**Model calls overlap.** They are the wall clock: a fan-out of ten `ask`s is ten
connections that spend their whole life waiting on a socket.

**Tool calls do not, in this RFC.** A `ToolHost` is an MCP client attached to a
child process that was started once, handshaken once, and may have been started
`--allow-write`. Eight concurrent iterations cannot each have their own without
starting eight copies of somebody else's program, and a write-capable server in
eight instances is a different arrangement from the one the operator approved —
the shape [GAP-035](../docs/gaps.md#gap-035) already warns about for memory
stores.

**And in this RFC a body that calls a tool does not overlap at all.** Sharing the
host behind a lock — so that a mixed body's `ask`s overlap while its `call`s
queue — is better and is deliberately left for a second change: it needs `ToolHost`
to gain `Send`, a proxy that locks per call, and per-iteration recording buffers so
`--record` still writes one reviewable cassette. Taking the concession now keeps
the first change to one trait's return type and no locks anywhere.

**The consequence, stated plainly.** A `parallel map` whose body calls a tool
gains nothing yet. One whose body only asks gains almost everything. This is a
real limitation and it gets a register entry rather than a footnote — see *Opens*,
below.

## The ceiling

A fan-out over a thousand items must not open a thousand connections.

```toml
[model]
max-concurrency = 4
```

Absent, a small built-in default. Zero or one means sequential execution, which
is what a machine gets when it says so.

**It is deployment configuration and the artifact may not state it,** for the
reason [GAP-040](../docs/gaps.md#gap-040) settled two days ago for
`timeout-seconds`: how many concurrent requests a service tolerates is a
property of that service and of the machine calling it, not of the program. The
same artifact against a hosted API and against Ollama on a laptop wants
different numbers, and an artifact carrying one would be wrong on one of them
with nothing in the program to explain why.

It sits under `[model]` rather than on a `[[model.provider]]` because a run
routes to one provider at a time and the ceiling bounds the run's appetite, not
an endpoint's capacity. If that turns out to be wrong — a deployment with a
generous gateway and a stingy local server — moving it to the provider is
additive and can happen later.

## What reading the code changed

This section was added when the RFC was picked up for implementation. Three of
its claims did not survive contact with the code, and each is corrected in place
below rather than left to be discovered by whoever built it.

**Digest matching does not buy determinism.** The rule was: replay takes the
first unconsumed row whose digest matches, and that is well defined even when two
rows share a digest, "because which *answer* each iteration gets cannot change
the result — the requests were identical". The requests are identical; **the
recorded answers need not be.** A fan-out over `["X", "X"]` issues two identical
requests, a live model answers `"foo"` and `"bar"`, and the recording holds two
rows with one digest and two values. Replay them concurrently and the iteration
that arrives first takes row 0, so the collected list is `[foo, bar]` or
`[bar, foo]` depending on the schedule. That is the flake the rule existed to
prevent. The RFC's own test list missed it:
`two_iterations_with_identical_requests_each_get_an_answer` asserts that each
gets *an* answer, never *which*.

**And concurrency under replay was never real anyway.** One cassette is one tape
with one position in it, so every replayed call serialises behind it — the
per-iteration-instance trick that makes live calls overlap has nothing to
duplicate. Concurrent replay and sequential replay therefore differ in exactly
one respect, arrival order, which is the defect above, and they differ in no
other: there is no socket to wait on, so the wall-clock win is zero. The rule is
replaced by the ceiling below, and **the cassette format is not touched at all.**

**The interpreter cannot reach `catalogue::build`.** "Each iteration gets its own
instance, built from the same declaration by `catalogue::build`" has nothing to
stand on: `run` is handed a `&mut dyn ModelProvider` and never sees a
`ProviderConfig`. What is needed is a **factory**, and its absence turns out to
be the honest signal for the ceiling — see *The contract change*.

**A locked step counter is less deterministic than no lock.** Charging every
iteration against one counter makes *which* iteration trips a budget depend on
which thread got there first. Per-iteration counters summed in index order
afterwards reproduce the sequential outcome exactly. See *Failure, and what it
costs*.

## Determinism: three rules, none optional

### The event stream is spliced in index order

Each iteration collects its events into its own buffer. When the fan-out
completes, the buffers are concatenated in **index order** and emitted into the
run's stream, wrapped by the `mapIteration` event that already exists.

So the reference interpreter emits exactly the sequence it emits today, in
exactly the order, for exactly the same inputs — and
`the_event_streams_agree_on_kind_and_order` keeps passing against a sequential
Python backend without being touched. That test is not adjusted to accommodate
this feature. If it needs adjusting, the feature is wrong.

The cost is a watcher's: nothing from inside a fan-out appears until the fan-out
is done. That is what buys the property, and it is the honest trade.

### A fan-out overlaps only what can be duplicated

Overlap needs one thing from every source of answers an iteration touches: a
second instance of it. **A source that exists once is a lock, and a lock is
sequential execution with extra steps** — plus, as the digest rule found out, a
schedule-dependent assignment of answers to iterations.

So the rule is stated the other way round from how it is usually reached. Rather
than a list of situations that get an exception, there is one question put to each
source of answers, and four of them answer no:

| Source | Why there is only one | What the runtime looks at |
|---|---|---|
| A cassette | one tape, one position in it | the run was given no provider factory |
| A contained run's supervisor | request/reply over one pair of pipes | the same — the guest's provider *is* the channel |
| A person | there is one operator, and **the order they are asked in is observable to them** | the body holds an `approval` or a `consult` node |
| A tool server | a child process, started once, handshaken once, possibly `--allow-write` | the body holds a `call` |

Each of those caps the fan-out at one iteration, which is sequential execution —
what this project has been doing all along, and conforming while doing it.

The first two need no detection at all: they *are* the absence of a factory, so
they cost no code and cannot be got wrong. The third is read from the artifact,
where the compiler has already put it — a policy that requires approval for an
effect makes the compiler insert an `Approval` node ahead of the call, so a body
that will stop for a person says so in the IR before the run starts. **This is why
`Interlocutor` needs no `Send` bound and `HumanChannel` never crosses a thread.**
It is also the same argument `ING6005` already makes about `consult`, applied to
the gate a policy inserts rather than the one an author writes.

The fourth is the expensive one, and it is a concession rather than a principle: a
shared `ToolHost` behind a lock *would* let a body's `ask`s overlap while its
`call`s serialised. That is strictly better, and it is left for a second change,
because it needs `ToolHost` to gain `Send`, a locking proxy, and per-iteration
recording buffers for `--record`. Until then a body that calls a tool runs
sequentially, and the register says so rather than the release notes.

**Nothing about a cassette changes.** Not the format, not the version, not the
matching rule, not one diagnostic. `ReplayProvider`, `ReplayToolHost` and
`ReplayInterlocutor` keep taking the next row by position, which is exactly right
for a tape written in index order and played back by a run with a ceiling of one.

### A recording is written in index order

`--record` collects each iteration's rows into its own buffer and appends them
in index order, exactly as the event stream is spliced.

Without this, the same run against the same inputs would produce a different
file every time, and a cassette nobody can diff is a cassette nobody reviews.
It is also what makes position matching still correct on the replay side: a tape
written in index order is a tape a sequential run plays back row for row. The two
halves of that sentence are the reason this rule survived and the digest rule did
not.

Until a shared `ToolHost` lands, `--record` reaches this rule only for a body
that makes no tool call. A body that makes one records sequentially, which is
index order already.

## Failure, and what it costs

**A failing iteration does not cancel the others.** In-flight iterations run to
completion; then the fan-out fails with the first failure in **index order**,
not the first in time.

The alternative — cancel everything the moment one fails — is faster and
cheaper, and it makes the amount of money a failing run spends depend on the
scheduler. The same artifact with the same inputs would produce two different
bills. This project refuses that shape everywhere else, and a run that is
already failing is not the place to start making exceptions.

Taking the first failure *in index order* rather than in time is the same rule
in a second place: which error an operator is shown must not depend on which
socket answered first.

**The budget, and why it does not get a lock.** One shared counter behind a mutex
is the obvious move and it is the wrong one: whichever thread reaches the ceiling
first is the iteration that fails, so *which* iteration an operator is told about
depends on the schedule. That is the failure this section just refused, arriving
through the back door.

Instead each iteration counts against **its own** counters, seeded with the run's
remaining headroom so that no single iteration can outspend the run, exactly as a
sub-agent already is
(`self.max_steps.saturating_sub(self.steps).max(1)`). When every iteration has
finished, the fan-out charges their totals **in index order** and fails at the
first crossing. That is arithmetic replaying what sequential execution would have
done, so the answer is not merely deterministic — it is the *same* answer,
iteration for iteration.

What a fan-out spends before it notices is the whole fan-out, because iterations
are drained rather than cancelled. That was already true of the lock version, and
it is the price the paragraph above pays for a bill that does not depend on a
scheduler.

## The live channel

**No deltas from inside a fan-out.** A watcher sees the `mapIteration` events
and no text; text resumes when the fan-out does.

Eight concurrent answers interleaved line by line on one terminal is not a
feature, and prefixing each fragment with its index makes it identifiable
without making it readable. A delta is not an event
([Runtime 0.3 §2](../specs/runtime/v0.3.md)) — it is never recorded, never
replayed and never asserted on — so this choice touches nothing except what a
person watching sees, which is exactly why it should be decided on that basis
alone.

`streams()` still governs the output-token ceiling as it does today. A provider
that streams keeps the larger ceiling inside a fan-out; it simply streams into a
sink that discards.

## The contract change

Three traits are involved and **none of them changes shape.** What changes is
that a run can be handed a way to make a second provider.

**A run may carry a provider factory, and whether it does is the ceiling.**
`complete(&mut self, …)` stays exactly as it is, and each concurrent iteration is
given **its own provider instance**, so exclusive access remains literally true
and no implementer has to reason about being called twice at once. The instance
has to come from somewhere, and `run` never sees a `ProviderConfig` — so the
caller that built the first provider supplies the means to build the rest:

```rust
/// A source of fresh providers, one per concurrent iteration.
///
/// Absent, a fan-out has a ceiling of one: a run that cannot make a second
/// provider has exactly one source of answers, and one source is a lock.
pub type ProviderFactory =
    Box<dyn Fn() -> Result<Box<dyn ModelProvider + Send>, ProviderError> + Send + Sync>;
```

`RunOptions` gains one optional field holding that. **A caller who passes nothing
gets today's behaviour exactly**, which is what makes replay, containment and
every embedder correct without knowing this feature exists. The CLI supplies a
factory when it built a live provider from the catalogue and withholds it when it
built a `ReplayProvider` — not as a special case for cassettes, but because there
is nothing to build a second of.

`catalogue::build` gains `+ Send` on what it returns, so the factory can hand its
product across a thread boundary:

```rust
// before
fn build(...) -> Result<Box<dyn ModelProvider>, ProviderError>

// after
fn build(...) -> Result<Box<dyn ModelProvider + Send>, ProviderError>
```

For somebody embedding `ingot-runtime` that is a one-line change to a type
annotation, and an existing `impl ModelProvider` compiles untouched unless it
holds something genuinely thread-hostile. It is still a semver-breaking release of
the crate — the new `RunOptions` field breaks a struct literal too — and this is
the cheapest moment it will ever be: no backend outside this repository is known
to exist ([GAP-039](../docs/gaps.md#gap-039)). Doing it later means doing it to
somebody.

The cost is real and worth naming: N providers means N HTTP connection pools
instead of one. At a ceiling of four to eight this is not interesting. It is
also why the ceiling is not optional.

**`EventSink` is not shared at all.** Each iteration writes into a plain
in-memory buffer; the run's real sink is touched only by the splicing step, on
the calling thread. `EventSink` needs no change of any kind.

**`ToolHost` and `HumanChannel` do not cross a thread boundary, so they need no
bound.** A body that would touch either runs sequentially, by the rule above.
Sharing the tool host behind a lock instead — which buys a mixed body its `ask`
overlap — is the second change, and it is the one that would add `Send` to
`ToolHost`.

## Security and policy impact

**None.** No new effect, no new capability, no change to what an agent may
reach. Every policy check happens where it happens today — at the node, before
the effect — and running two of them at once does not make either weaker. A
policy denial inside a fan-out fails its iteration, and the fan-out then fails
by the rule above.

The egress boundary is unaffected: `--sandbox` bounds tool servers, and tool
servers do not gain concurrency here. A contained run's supervisor channel is
request/reply over one pair of pipes, so a contained run keeps a ceiling of one
and executes its fan-out sequentially — the same answer, and it must be stated
in the spec rather than discovered.

## Static bounds

A `parallel map` executes its body once per element, which is exactly what it
did before. Step counting reaches the same total, charged in index order. Loop
bounds are unaffected: a `parallel` body may not contain a `checkpoint` or a
state write, and a nested `parallel` inside a `parallel` shares the one run-wide
ceiling rather than multiplying it.

## Compatibility

- **Language: unchanged.** No new syntax, no new keyword.
- **IR: unchanged.** `Parallel` already carries everything needed. Artifacts
  compiled before this produce byte-identical IR and run identically.
- **Cassette: not touched.** Not the format, not the version, not the matching
  rule. A run that replays one has a ceiling of one, so position matching stays
  correct for the reason it always was.
- **Runtime spec: 0.6**, additive — §5.1 keeps its wording and gains the
  splicing rule, the ceiling and what caps it, and the index-order budget.
- **`ingot-runtime` the crate: breaking**, for the `+ Send` bound on what
  `catalogue::build` returns and for the new `RunOptions` field. The CLI and every
  crate in this workspace are updated in the same change.
- **`[model] max-concurrency`: new and optional.** A manifest without it behaves
  as it does today apart from the wall clock.
- **The Python backend: unchanged, and still conforming.** It stays sequential,
  which §5.1 has always permitted.

## Alternatives

**Do nothing.** The register has carried this entry honestly for months and
nothing is misleading. But `parallel map` is a word an author writes to mean
something, and it currently means nothing.

**Concurrency for live providers, sequential under replay — now adopted, and it
was on this list.** [GAP-010](../docs/gaps.md#gap-010) ruled it out as the cheap
version, on the grounds that it makes an artifact behave one way in a test and
another in production. Two things overturn that, and both were found in the code
rather than argued from the armchair.

The divergence is in the **schedule**, not the behaviour. Runtime 0.1 §5.1 says a
`parallel` node marks an opportunity and that conformance asserts the result and
never the schedule. The Python backend runs a fan-out sequentially in production
and is conforming while doing it. A test that runs one sequentially cannot be the
divergence this project exists to refuse, when a shipped backend doing the same
thing is not.

And the alternative on offer was not concurrent replay — it was *nondeterministic*
concurrent replay, because one tape is one lock and the row a duplicate-digest
request gets depends on which thread asked first. Between a schedule difference
the spec declines to assert and a flake in `ingot test`, there is no contest.

What GAP-010 was right about is the thing that would have made it cheap for the
wrong reason: **this must not be a switch on "is there a cassette".** It is a
consequence of a general rule about sources that exist once, and the cassette case
falls out of it without being named.

**Async instead of threads.** `ureq` is blocking, so this means replacing the
HTTP layer, adding an async runtime to a crate that currently has one dependency
on that axis, and rewriting three providers and the MCP client. It buys the same
overlap. The connection-pool cost that per-iteration providers pay is the only
thing it would improve, and a ceiling of four to eight makes that cost
uninteresting.

**One shared provider with interior mutability.** `complete(&self, …)` and a
lock inside. One connection pool, conceptually tidier — and every implementer of
the central trait now has to answer "what happens when two threads call me at
once", which is a harder contract to write correctly than the one it replaces.
The break for an outside embedder is also much larger. Rejected for the smallest
change that works.

**Keeping position matching and sorting the recorded rows — also adopted, and for
the reason this entry gave for rejecting it.** "Only if replay also *executes* in
index order, which is sequential execution" was correct in every word. It was
filed as an objection; it is the design. The entry stands as written and the
conclusion drawn from it was the wrong way round.

**Cassette 0.4, with each row naming the iteration it belongs to.** This is what
concurrent replay would actually cost: an `iterationPath` on a row, matching by
node and iteration and position within it, digest demoted to a check. It works, it
is deterministic, and 0.3 cassettes would keep replaying by position. It was
rejected because it buys a wall-clock win of zero — under replay there is no
socket to wait on — in exchange for a format version, the day after 0.7.0 shipped
saying that no format had moved.

**A `limit` in the source: `parallel map items as x limit 4`.** Puts a
deployment fact in the program, and would be wrong on the next machine. Same
argument that kept `timeout-seconds` out of the artifact.

## Opens

- **A fan-out whose body calls a tool does not overlap.** *Closed the same day, in
  the change after this one.* The host is shared behind a lock, so its calls queue
  and it never serves two iterations at once. The `--allow-write` decision this
  entry asked for needed no new rule: what the server sees is the same serial
  stream of calls in a different order, and Language 0.1 §6.4 has always said a
  `parallel` body runs in an unspecified order. What is left of the register entry
  is `--record`, which still keeps a ceiling of one.
- **A fan-out whose body can stop for a person does not overlap.** The same entry
  covers it, and this half may never be worth closing: the order an operator is
  asked in is observable to them, which is the argument `ING6005` already makes
  about `consult`.
- **A contained run keeps a ceiling of one.** Covered by the same entry as
  [GAP-031](../docs/gaps.md#gap-031)'s reasoning — the supervisor channel is
  request/reply — and stated in the spec rather than left to be discovered.

## Conformance tests

- [x] `a_fan_out_produces_the_same_values_as_a_sequential_one`
- [x] `the_event_stream_of_a_fan_out_is_byte_identical_to_the_sequential_one`
- [x] `the_event_streams_agree_on_kind_and_order` (existing; must keep passing
      unchanged against a sequential Python backend)
- [x] `a_run_without_a_provider_factory_executes_a_fan_out_sequentially`
- [x] `a_replayed_fan_out_takes_its_rows_in_order`
- [x] `two_iterations_over_identical_items_get_the_rows_recorded_for_them`
      (the case the digest rule got wrong: same digest, different recorded
      values, and the collected list must be the recorded one every time)
- [x] `a_cassette_recorded_from_a_fan_out_is_byte_identical_run_to_run`
- [x] `a_failing_iteration_lets_the_others_finish`
- [x] `the_reported_failure_is_the_first_in_index_order_not_in_time`
- [x] `a_budget_trips_at_the_same_total_and_names_the_same_iteration`
- [x] `max_concurrency_of_one_is_sequential_execution`
- [x] `a_fan_out_over_a_thousand_items_opens_no_more_than_the_ceiling`
- [x] `no_deltas_are_shown_from_inside_a_fan_out`
- [x] `a_contained_run_executes_a_fan_out_sequentially` — covered by
      `a_run_without_a_provider_factory_executes_a_fan_out_sequentially`
      rather than by a container: the guest supplies no factory, which is
      the whole of the mechanism. A test that started a box would be
      testing the box.
- [x] `a_body_that_needs_a_person_executes_sequentially`
- [x] `a_body_that_calls_a_tool_executes_sequentially`
