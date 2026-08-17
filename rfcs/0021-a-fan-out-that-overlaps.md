# RFC-0021: A fan-out that overlaps

- Status: **Draft**
- Created: 2026-08-17
- Affects: runtime spec, CLI, `ingot-runtime`
- Closes: [GAP-010](../docs/gaps.md#gap-010)
- Builds on: [RFC-0002](0002-runtime-execution-model.md),
  [RFC-0013](0013-streaming.md)
- Would specify: Runtime 0.6

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
stores. So the host is shared and its calls serialise behind a lock.

**The consequence, stated plainly.** A `parallel map` whose body is mostly
`call` gains almost nothing. One whose body is mostly `ask` gains almost
everything. This is a real limitation and it gets a register entry rather than
a footnote — see *Opens*, below.

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

### Replay matches by digest

`ReplayProvider`, `ReplayToolHost` and `ReplayConsultations` stop taking the
next row by position and take **the first unconsumed row whose digest matches**.

This needs no new cassette format. Every row already carries the digest —
`request_digest` on an interaction, `invocation_digest` on a tool call,
`question_digest` on a consultation — because
[RFC-0020](0020-a-person-in-the-loop.md) and its predecessors put them there to
*verify* a position match. The change is which of the two is the key and which
is the check. **Cassette 0.3 is unchanged, and an 0.2 cassette still replays.**

Two rows may legitimately carry the same digest: two iterations over identical
items ask identical questions. "First unconsumed match" is well defined for
that, and it is deterministic regardless of arrival order, because which
*answer* each iteration gets cannot change the result — the requests were
identical.

What is lost is a diagnostic: today a mismatch can say *interaction 3 was
recorded for a different prompt*. With digest lookup, a request that matches
nothing must say *no recorded interaction matches this request*, and name the
node. That is a slightly worse message for a strictly better property, and the
message should list what the cassette does hold for that node.

### A recording is written in index order

`--record` collects each iteration's rows into its own buffer and appends them
in index order, exactly as the event stream is spliced.

Without this, the same run against the same inputs would produce a different
file every time, and a cassette nobody can diff is a cassette nobody reviews.
Digest matching would still replay it — that is the point of digest matching —
but "it replays" is not the only thing a recorded file is for.

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

**The budget.** `charge_step` and the token and cost counters move behind a
lock. The pass/fail outcome does not change: a `parallel map` runs the same set
of nodes in either schedule, so the same total is reached. What changes is how
much of a fan-out has completed at the moment a budget trips — and since a
failing run drains its in-flight work anyway, the spend is the whole fan-out
either way. Deterministic, and stated.

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

Three traits are involved and only one of them changes shape.

**`ModelProvider` gains `Send` on the trait object, and nothing else.**
`complete(&mut self, …)` stays exactly as it is. Each concurrent iteration is
given **its own provider instance**, built from the same declaration by
`catalogue::build`, so exclusive access remains literally true and no
implementer has to reason about being called twice at once.

```rust
// before
fn build(...) -> Result<Box<dyn ModelProvider>, ProviderError>

// after
fn build(...) -> Result<Box<dyn ModelProvider + Send>, ProviderError>
```

For somebody embedding `ingot-runtime`, that is a one-line change to a type
annotation, and their existing `impl ModelProvider` compiles untouched unless it
holds something genuinely thread-hostile. It is still a semver-breaking release
of the crate, and this is the cheapest moment it will ever be: no backend
outside this repository is known to exist
([GAP-039](../docs/gaps.md#gap-039)). Doing it later means doing it to somebody.

The cost is real and worth naming: N providers means N HTTP connection pools
instead of one. At a ceiling of four to eight this is not interesting. It is
also why the ceiling is not optional.

**`EventSink` is not shared at all.** Each iteration writes into a plain
in-memory buffer; the run's real sink is touched only by the splicing step, on
the calling thread. `EventSink` needs no change of any kind.

**`ToolHost` and the recording/replay providers are shared behind a lock.** A
tool host because it wraps a child process; a recorder because there is one
cassette. Their traits do not change either — a `Mutex` supplies the exclusive
access `&mut self` asks for. This is what makes tool calls serialise, which is
the limitation stated above.

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
did before. Step counting is unchanged in total and moves behind a lock. Loop
bounds are unaffected: a `parallel` body may not contain a `checkpoint` or a
state write, and a nested `parallel` inside a `parallel` shares the one run-wide
ceiling rather than multiplying it.

## Compatibility

- **Language: unchanged.** No new syntax, no new keyword.
- **IR: unchanged.** `Parallel` already carries everything needed. Artifacts
  compiled before this produce byte-identical IR and run identically.
- **Cassette: unchanged, still 0.3.** Digest matching reads the fields that are
  already there; 0.2 and 0.3 cassettes replay untouched.
- **Runtime spec: 0.6**, additive — §5.1 keeps its wording and gains the
  splicing rule, the digest-matching rule and the contained-run ceiling.
- **`ingot-runtime` the crate: breaking**, for the `+ Send` bound. The CLI and
  every crate in this workspace are updated in the same change.
- **`[model] max-concurrency`: new and optional.** A manifest without it behaves
  as it does today apart from the wall clock.
- **The Python backend: unchanged, and still conforming.** It stays sequential,
  which §5.1 has always permitted.

## Alternatives

**Do nothing.** The register has carried this entry honestly for months and
nothing is misleading. But `parallel map` is a word an author writes to mean
something, and it currently means nothing.

**Concurrency for live providers, sequential under replay.** The cheap version:
leave `ingot test` alone by only going concurrent when there is no cassette.
[GAP-010](../docs/gaps.md#gap-010) already names this and rules it out — it
makes an artifact behave one way in a test and another in production, which is
the divergence this project exists to refuse. It is listed here only so that the
next person to think of it finds it already answered.

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

**Keeping position matching and sorting the recorded rows.** If a recording is
written in index order anyway, could replay keep matching by position? Only if
replay also *executes* in index order, which is sequential execution. The
recording order and the execution order are different problems and both need
their own answer.

**A `limit` in the source: `parallel map items as x limit 4`.** Puts a
deployment fact in the program, and would be wrong on the next machine. Same
argument that kept `timeout-seconds` out of the artifact.

## Opens

- **Tool calls inside a fan-out serialise.** A new register entry, class
  Degraded: the result is correct and only the wall clock is affected, exactly
  as GAP-010 itself reads today. Closing it needs a decision about what a
  concurrent `ToolHost` means for a child process and for an `--allow-write`
  server.
- **A contained run keeps a ceiling of one.** Covered by the same entry as
  [GAP-031](../docs/gaps.md#gap-031)'s reasoning — the supervisor channel is
  request/reply — and stated in the spec rather than left to be discovered.

## Conformance tests

- [ ] `a_fan_out_produces_the_same_values_as_a_sequential_one`
- [ ] `the_event_stream_of_a_fan_out_is_byte_identical_to_the_sequential_one`
- [ ] `the_event_streams_agree_on_kind_and_order` (existing; must keep passing
      unchanged against a sequential Python backend)
- [ ] `a_replay_matches_by_digest_regardless_of_arrival_order`
- [ ] `two_iterations_with_identical_requests_each_get_an_answer`
- [ ] `a_cassette_recorded_from_a_fan_out_is_byte_identical_run_to_run`
- [ ] `an_0_2_cassette_still_replays`  (existing; must keep passing)
- [ ] `a_failing_iteration_lets_the_others_finish`
- [ ] `the_reported_failure_is_the_first_in_index_order_not_in_time`
- [ ] `a_budget_trips_at_the_same_total_under_either_schedule`
- [ ] `max_concurrency_of_one_is_sequential_execution`
- [ ] `a_fan_out_over_a_thousand_items_opens_no_more_than_the_ceiling`
- [ ] `no_deltas_are_shown_from_inside_a_fan_out`
- [ ] `a_contained_run_executes_a_fan_out_sequentially`
- [ ] `a_tool_call_inside_a_fan_out_serialises`
