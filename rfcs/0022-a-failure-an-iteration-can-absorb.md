# RFC-0022: A failure an iteration can absorb

- Status: **Draft**
- Created: 2026-08-18
- Affects: language, IR, runtime spec, compiler, `ingot-runtime`
- Narrows: [GAP-044](../docs/gaps.md#gap-044)
- Builds on: [RFC-0014](0014-a-capabilitys-reach.md),
  [RFC-0021](0021-a-fan-out-that-overlaps.md)
- Would specify: Language 0.4, Agent IR 0.4, Runtime 0.7

## Problem

A program cannot say what to do when something fails, and the shape that wants it
most is the most ordinary one there is: **partial data.**

```ingot
entries = parallel map incidents as incident {
  writeup = call fs.read_file("${incident.id}.md")
  ask<entry>("Summarise this incident.", context: writeup)
}
```

Three incidents, two write-ups on disk, and the third was never filed. Today:

```text
error: at node `n0`: the tool failed: `fs.read_file`: `INC-2.md`:
       The system cannot find the file specified. (os error 2)
```

Exit code 1, no digest, and **the other two summaries are thrown away with it** —
after being paid for. Iterations are drained rather than cancelled
([Runtime 0.6 §3.4](../specs/runtime/v0.6.md)), deliberately, so that a failing
run's cost does not depend on the schedule. The consequence is that a fan-out
which loses one element **spends the whole fan-out and returns nothing.**

That is not a missing convenience. It is a run that burns tokens to produce a
non-result, and the author's remedy — check whether every file exists before
starting — is a thing the language cannot express either.

## What the evidence changed about this RFC

[GAP-044](../docs/gaps.md#gap-044) was written from reading the language surface,
and it proposed the general feature: a handler, a fallback, a retry. Two agents
were then written outside the repository to test that by evidence, and they moved
the design twice.

**One of them never wanted it.** An agent that fans out over documentation pages
and reports which ones no longer say what they should compiled first try and ran
first try. For work whose job is *produce something correct or nothing*, failing
loudly is right and this gap is not felt. So the feature must not make that agent
worse, and in particular **must not become the default**.

**The other wanted six characters, not a handler.** What its author reached for was
`else "no write-up was filed"` on one statement — not a block, not a retry, and
not a second effectful call. That is the smaller feature, it covers what two out of
two real programs actually needed, and — as the next section shows — it collapses
four of the five hard questions the register entry raised rather than answering
them.

## The design: `else`, and only over a pure expression

```ingot
writeup = call fs.read_file("${incident.id}.md") else "no write-up was filed"
grade   = ask<rating>("Rate this.", context: page) else rating { score: 0, note: "unrated" }
```

`else` attaches to an expression that can fail — `ask`, a tool `call`, a sub-agent
`call` — and supplies the value to use when it does. Both sides have the same type,
by the ordinary assignability rule
([Language 0.1 §4.2](../specs/language/v0.1.md)).

**The fallback is a pure expression.** No `ask`, no `call`, no `consult`, no
`parallel map` inside it — literals, record and list construction, reads of things
already bound, arithmetic and the built-ins. Anything effectful is a compile error.

That single restriction is what makes the rest of this RFC short.

### Why the restriction is the whole design

The register entry asked five questions. Four of them stop existing:

| Question | Answer under a pure fallback |
|---|---|
| Does a failed attempt count toward `steps`? | Yes, exactly once, as it does today — the attempt is the step. The fallback is an expression and expressions are not steps, so **the static bound is unchanged**: still one step per statement, and the compiler's walk does not branch. |
| May a fallback reach effects the path it replaces did not? | It cannot reach anything. A pure expression has no effects, so the union-over-paths problem never arises and a policy stays a per-node statement. |
| What does a cassette record? | The failure, which it already can — `ToolExchange` carries `error` and has since 0.2, precisely so a recording can hold the behaviour most worth testing. Replay serves the failure, the fallback runs again, and the run continues identically. **No cassette change.** |
| What does a fallback mean for `verify`? | Nothing: `else` may not attach to `verify`. See *What `else` must not catch*. |

The fifth question — is a recovered failure still in the event stream? — is the one
that needs a real answer, and it gets one below.

### What `else` must not catch

This is the most important rule here, and it is not about ergonomics.

`else` catches a failure of the **attempt**: a tool that errored, a provider that
could not be reached, an answer that did not match its declared type, a sub-agent
that failed. It **must not** catch any of these:

| Not caught | Because |
|---|---|
| A capability denied by policy | The artifact's own statement about what it may do. If `else` routed around it, `deny` would become advisory — which is the one thing this project exists to refuse. |
| A budget exceeded | A budget bounds the **run**, not a statement. A statement that could absorb it would make a budget a suggestion with extra steps. |
| An approval refused | A person said no. Continuing with a default is continuing without them. |
| A `verify` that does not hold | A failed check is the failure most worth recovering from and the one where recovering is most suspect: the property the artifact states either holds or the run is not entitled to its output. |
| A malformed artifact | Not a failure of the world; a failure of the program. |

A backend that got this wrong would turn every guarantee in the language into a
default value, silently. So the enforcement is not left to prose: the runtime's
error type already distinguishes these — `CapabilityDenied`, `BudgetExceeded`,
`ApprovalDenied`, `VerificationFailed`, `MalformedIr` — and the rule is a match on
that enum, with a test per variant.

### The event stream says what happened

A recovered failure **must** be visible. A run that quietly succeeded on a default
is a run whose record does not say what happened, and the record is what this
project sells.

The existing events already carry the failure: a `toolCall` event carries what the
tool returned or the error it produced. What is new is one event saying the value
was replaced:

```json
{"event":"fallbackTaken","node":"n4","because":"tool"}
```

Emitted after the failing node's own event and before execution continues.
`because` is the *kind* of failure — `tool`, `model`, `agent` — and never its text,
which is already in the event before it.

Both backends emit it, so
`the_event_streams_agree_on_kind_and_order` keeps meaning what it means. A reader
counting `fallbackTaken` events knows how much of a digest was made of defaults,
which is exactly the question somebody reviewing one would ask.

### Inside a fan-out, which is the point

Nothing about `else` is special inside a `parallel map`, and that is what makes the
motivating case work: the iteration does not fail, so the fan-out does not fail,
and the collected list keeps one entry per element.

That last part matters more than it looks. The alternative shape — let a failing
iteration contribute *nothing* and shorten the list — was considered and rejected
in *Alternatives*: it silently loses elements, and a verifier like
`len(findings) >= len(pages)` is exactly the kind of check an author writes to
catch that.

## Compatibility

- **Language: 0.4.** New syntax, and purely additive — no program written before it
  parses differently. `else` becomes a reserved word
  ([Language 0.1 §2.5](../specs/language/v0.1.md)); it is already one, as the
  `if`/`else` arm.
- **Agent IR: 0.4.** A node gains an optional `fallback` value. An artifact without
  one behaves exactly as it does today, so **every existing artifact is a valid
  0.4 artifact** and compiles to byte-identical IR.
- **Cassette: unchanged, still 0.3.** A recorded failure is already a recorded
  failure.
- **Runtime: 0.7**, additive: the fallback rule, the not-caught list, and
  `fallbackTaken`.
- **`ingot-runtime` the crate: additive.** No trait changes shape.
- **The Python backend: must implement it**, and it is a small change there — a
  `try` around the emitted call and the fallback expression in the `except`, with
  the not-caught list as a re-raise.

## Alternatives

**A general handler: `try { … } else { … }`.** The thing the register entry
proposed. Rejected *for now* rather than on principle, and the reason is the
questions the table above erased: a block can contain another `ask`, which reopens
step counting, effects-over-paths, and what a cassette records for the path not
taken. That design is worth having and it should be designed against a program that
needs it. Neither program written for this one did.

**A fallback that is itself effectful: `call web.search(q) else call cache.read(q)`.**
The most obviously useful next step, and the one that costs the most: two paths
with different effects means the policy an operator reads no longer describes one
sequence of nodes. It also asks whether the second attempt gets its own step, which
determines whether `steps` still bounds anything. Left out deliberately, and named
here so the next person finds it already thought about.

**A retry: `ask<…>(…) retry 3`.** A different feature wearing the same clothes. A
retry spends money on the same attempt and is a property of the *transport*, not
the program — the same argument that keeps `timeout-seconds` out of the artifact
([GAP-040](../docs/gaps.md#gap-040)). The providers already retry transport
failures, and a timeout deliberately is not retried, because the ceiling was being
multiplied by four.

**Let a failing iteration contribute nothing, shortening the list.** Considered
because it is the other shape the evidence suggested. Rejected: `parallel map` over
`T[]` with a body of type `U` yields `U[]` with one entry per element, and quietly
returning fewer breaks the one thing a reader can rely on about a fan-out. An
author who wants that can map to a record with a flag and filter afterwards — once
there is a filter, which there is not.

**Optional types, so a failure produces an absent value.** The honest general
answer, and much larger than this: it needs a type constructor, a way to test it,
and a rule for every place a value is used. It is the shape a language reaches for
eventually and not the shape a 0.x reaches for to fix a fan-out that burns tokens.

## Opens

- **The general handler and the effectful fallback** both stay open, above, with
  the questions they have to answer. [GAP-044](../docs/gaps.md#gap-044) narrows to
  them rather than closing.
- **How many defaults is too many?** A digest made entirely of `"no write-up was
  filed"` is a successful run and a useless answer. `fallbackTaken` makes it
  countable, and whether the *artifact* should be able to state a ceiling — *fail
  if more than half the iterations fell back* — is a real question and a separate
  one. It is the same shape as a budget, which suggests where it would live.

## Conformance tests

- [ ] `a_tool_failure_is_absorbed_and_the_run_continues`
- [ ] `a_model_failure_is_absorbed_and_the_fallback_is_typed`
- [ ] `a_fallback_that_reaches_anything_does_not_compile`
- [ ] `a_policy_denial_is_not_absorbed`
- [ ] `a_budget_trip_is_not_absorbed`
- [ ] `a_refused_approval_is_not_absorbed`
- [ ] `a_failed_verify_is_not_absorbed`
- [ ] `else_on_something_that_cannot_fail_does_not_compile`
- [ ] `a_fan_out_whose_iteration_falls_back_does_not_fail`
- [ ] `a_fan_out_that_fell_back_collects_one_entry_per_element`
- [ ] `the_fallback_is_visible_in_the_event_stream`
- [ ] `a_recorded_failure_replays_as_a_failure_and_falls_back` (the cassette is
      unchanged, so this must pass against a 0.3 recording)
- [ ] `the_event_streams_agree_on_kind_and_order` (existing; must keep passing,
      which means the Python backend implements this too)
- [ ] `an_artifact_without_a_fallback_compiles_to_byte_identical_ir` (existing
      golden IR; must keep passing)
