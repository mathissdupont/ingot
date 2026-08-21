# RFC-0023: A fallback you can write, and one that can act

- Status: Draft
- Created: 2026-08-21
- Affects: language, IR, runtime spec, compiler, `ingot-runtime`,
  `ingot-backend-python`, cassette
- Closes: [GAP-045](../docs/gaps.md#gap-045)
- Narrows: [GAP-044](../docs/gaps.md#gap-044)
- Depends on: [GAP-046](../docs/gaps.md#gap-046) for the second half
- Builds on: [RFC-0022](0022-a-failure-an-iteration-can-absorb.md)
- Specifies: Language 0.5, Agent IR 0.5, Runtime 0.8

## Problem

[RFC-0022](0022-a-failure-an-iteration-can-absorb.md) shipped `else`, and it
works. It also shipped a fallback that **cannot be written for most of the types
an agent actually produces**, and the two limits it left are the subject here.

```ingot
score   = ask<int>("Score this out of ten.") else 0                  // compiles
writeup = call fs.read_file(path) else "no write-up was filed"       // compiles
summary = ask<markdown>("Summarise this.") else "nothing to say"     // ING3001
grade   = ask<rating>("Rate this.") else rating { score: 0 }         // does not parse
report  = call fs.write(path, body) else call fs.write(fallback, body) // no such form
```

Three of those five are the state of the language today
([GAP-045](../docs/gaps.md#gap-045)), and the reason is not a special case in the
fallback's type check. It is that a fallback must be a **pure expression**, and
this language can only write pure values of a handful of types. There is no record
literal, and `markdown`, `json` and `file` are more specific than anything that
widens into them.

So `else` reaches an `int`, a `string`, a `bool`, a list of those, and — since 0.4
— `text`. An agent that emits a document or a declared record has a fallback it
cannot spell.

The fifth line is [GAP-044](../docs/gaps.md#gap-044)'s remainder: a fallback that
does something, rather than one that is a constant.

## What this RFC does and does not take on

It takes the two halves RFC-0022 named as the next steps, in the order their costs
say:

1. **A fallback that can be written**, for records and for the prose types. New
   syntax, no new execution semantics: the interpreter already knows how to use a
   pure value when an attempt fails.
2. **A fallback that can act** — `call … else call …`. New execution semantics,
   three questions to answer, and one dependency on a gap that is still open.

It does **not** take on the general handler, `try { … } else { … }`, and the reason
is RFC-0022's own: *that design is worth having and it should be designed against
a program that needs it. Neither program written for this one did.* That is still
true. Designing it here would be designing it against an imagined program, which
is the failure mode the sentence exists to prevent. What it would have to answer is
restated in [Alternatives](#alternatives) so the next person starts from there
rather than from scratch.

## Part 1: a record literal

```ingot
type rating {
  score: int
  note: string
}

grade = ask<rating>("Rate this.") else rating { score: 0, note: "not rated" }
```

**Every declared field must be present.** Records have had no defaults since
[Language 0.1 §4.3](../specs/language/v0.1.md), and a literal that allowed a
missing field would introduce them through the back door — silently, and only for
whichever fields somebody left out. A missing field is `ING3016`, naming the
fields that are missing.

An unknown field is `ING3017`; a field written twice is `ING3018`. Order does not
matter, because a record is not a tuple.

Each field's value must be assignable to the field's declared type under the
existing table ([Language 0.4 §2](../specs/language/v0.4.md)) — so `score: 1` is
accepted for a `float` field, and no new coercion is introduced anywhere.

**A record literal is pure exactly when all of its field values are.** Nesting
works because purity is compositional: a literal of a record whose field is
another record takes a literal there. That also means a record literal is not
only a fallback — it is an ordinary expression, usable anywhere a value of that
type is, which is the honest way to add it. A construct that existed only after
`else` would be a special case wearing a general name.

## Part 2: the prose types get a literal, not a widening

`markdown`, `json` and `file` cannot be reached by widening, and
[Language 0.4 §2](../specs/language/v0.4.md) already argued why they must not be:
every widening in this language goes from the more specific type to the less, and
`string` → `markdown` would go the other way. Admit it and `text` → `markdown`
cannot be refused either, and then `markdown` and `text` are one type with two
names — which is not what a content type or a file extension mean.

That argument is sound and this RFC does not reopen it. It closes the gap from the
other side:

```ingot
summary = ask<markdown>("Summarise this.") else markdown "nothing to say"
config  = call svc.settings() else json "{}"
```

**A typed literal, and deliberately not a cast.** The type name may be followed
only by a literal of the underlying representation — a string literal for
`markdown` and `json`. It is not an operator over an expression, so there is no
form in which an arbitrary `string` *value* becomes a `markdown` value. Nothing is
narrowed, because at the point it is written there is no value yet: the author is
writing the bytes and stating their content type in one breath, which is the same
act as `emit report = …` declaring what a document is.

That distinction is the whole of why this is admissible where the widening is not,
and it is worth stating in the specification rather than only here.

**`json` is checked.** `json "{ not json"` is `ING3019` at compile time. A literal
is the one case where the bytes are available to the compiler, so the type is not
merely asserted — it is verified. A widening could never have offered that.

**`file` gets no literal, and that is not an omission.** A `file` is a handle to
something that exists. There is nothing for an author to write, and inventing a
form that produces a `file` naming nothing would be worse than the gap. A `file`
attempt therefore has no *pure* fallback and needs Part 3 —
`call fs.write(…) else call fs.write(…)` — which is where a fallback that has to
produce a file belongs.

After Part 1 and Part 2, `else` can be written for every type except `file`, and
the exception has a reason a reader can check.

## Part 3: a fallback that can act

```ingot
hits = call web.search(query) else call cache.read(query)
```

RFC-0022 left this out deliberately and named three costs. Each is answerable; the
answers are what this part is.

### 3.1 Effects, over two paths instead of one

The objection: *two paths with different effects means the policy an operator
reads no longer describes one sequence of nodes.*

The answer is that a policy never bounded a sequence — it bounds a **set**. The
compiler already computes an artifact's effects over the whole graph, and
`if`/`else` has produced two paths since 0.1 without weakening anything. So:

* An artifact's declared effects are the union over all paths, as now.
* The policy must grant every effect on **every** path, including the fallback's.
  A fallback that reaches something the policy denies is `ING4001` before the run,
  exactly as the primary attempt would be.
* A denied fallback is refused at compile time rather than at the moment it is
  needed, which is the point of checking a policy at all.

What does change is a claim in prose rather than in code: anywhere the
specifications say a policy describes what the run *will* do, they must say what it
*may* do. Auditing that wording is part of this RFC's work, not a follow-up.

### 3.2 Steps, and whether they still bound anything

The objection: *it asks whether the second attempt gets its own step, which
determines whether `steps` still bounds anything.*

**Yes, and `steps` still bounds, because reading the code shows the compile-time
check was never the bound.** This subsection was drafted with the answer backwards
and is worth leaving in the corrected shape rather than tidied, because the
correction is the answer.

There are two checks, and only one of them is a ceiling:

* `min_steps` in the compiler computes a **minimum** — the cheapest path — and
  `ING5006` refuses a flow whose cheapest path already exceeds the budget
  (*"the flow needs at least N steps but the budget allows M"*). An `if` takes the
  **min** over its arms for exactly this reason.
* `charge_step` in the interpreter increments per step and fails the run at the
  limit. That is where a `steps` ceiling has always been enforced, and it counts
  what actually ran.

So an effectful fallback needs no new rule. Its attempt costs a step when it runs,
`charge_step` counts it like any other, and the run stops at the ceiling naming the
node — the same outcome as any other step over budget. The compile-time minimum is
unchanged, because the cheapest path through `attempt else fallback` is still the
one where the attempt succeeds.

A pure fallback continues to cost nothing, which is not a special case but the
same rule: it reaches nothing, so there is no step to charge.
`a_fallback_does_not_move_the_static_step_bound` in `ingot-semantic` pins that, and
it keeps passing.

What this does cost is honesty in the prose: a `steps` budget bounds what a run
*may* spend and is checked while it spends it, and the compiler's contribution is
to reject a flow that cannot fit at all. Any wording that suggests the compiler
proves the ceiling needs correcting, and finding it is part of this work.

### 3.3 The recording, and the path not taken

Nothing is recorded for the path not taken, because a recording is of what
happened. The events already say which path a run took — `fallbackTaken` carries
the node and the kind of failure ([Runtime 0.7 §2](../specs/runtime/v0.7.md)) — so
a reader can see it and a replay can be checked against it.

Replay is where this part has a real dependency. For the fallback to be *reached*
on replay, the primary attempt's failure has to be replayable:

* **A tool attempt already can.** `ToolExchange` carries `error`, so a recorded
  tool failure replays as a failure and the fallback runs.
* **A model attempt cannot.** `Interaction` requires a value and has no error, so
  a model call that returned nothing is not recordable at all — which is
  [GAP-046](../docs/gaps.md#gap-046), opened by RFC-0022 for this reason.

So `call … else call …` over tools is testable today, and an `ask … else call …`
is not. Two honest options: land Part 3 for tool attempts only and say so, or
close GAP-046 first. This RFC proposes **closing GAP-046 as part of it**, at a new
cassette version, because shipping a fallback whose most interesting case cannot be
covered by `ingot test` would be shipping something the project cannot hold itself
to.

## Compatibility

Part 1 and Part 2 are **purely additive**: they make programs compile that did not,
and no program that compiled behaves differently. Three new literal forms, four new
diagnostics, no change to any existing rule.

Part 3 changes what an artifact can contain, so it moves the language, the IR and
the runtime version. It also moves the cassette, through GAP-046 — the first
cassette change since 0.3, and the reason for it is a field that cannot be
back-filled: a recording made before it has no failures in it to describe.

`markdown`, `json` and `rating`-style literals are new syntax, so a program using
them declares `language 0.5`. Language 0.4 programs are unaffected.

## Alternatives

**A narrowing rule, `string` → `markdown`.** The obvious fix, refused in 0.4 with
an argument this RFC agrees with and does not restate.

**A conversion tool.** Declaring `tool text.as_markdown(s: string) -> markdown`
would work today and is worse in every way: an effect-free call that exists only to
change a type, and a fallback that has to reach a tool server to produce a
constant.

**Optional types, so a failure produces an absent value.** RFC-0022 called this
*the honest general answer, and much larger than this*. Still true, and Part 1 and
Part 2 do not make it harder: a literal is orthogonal to an optional.

**Field defaults on a record, so a literal can be partial.** Rejected for the
reason a total literal is proposed: defaults are a second place a value can come
from, and every reader of the record then has to know both. If defaults are wanted
they are their own RFC, argued for on their own.

**The general handler, `try { … } else { … }`.** Not designed here, for RFC-0022's
reason: it should be designed against a program that needs it, and no program has
needed it yet. What it will have to answer, so the next attempt starts here:

* A block can contain another `ask`, so **step counting** has to bound a set of
  paths rather than take a maximum over two arms.
* **Effects over paths** is the same question as §3.1 and is answered by it, but a
  block can also contain a `verify` and a `checkpoint`, and what a checkpoint means
  on a path that may be abandoned is open.
* **What a cassette records** for a handler that ran halfway and then took the
  other path. §3.3's answer covers one attempt and one fallback; it does not cover
  a partially executed block.
* Whether the two output checks survive paths multiplying: `ING6001` refuses a
  flow that never emits, and `ING6004` **warns** where an output is not emitted on
  every path. The warning is the one that gets harder, and a handler is precisely a
  construct that makes "every path" large.

**Doing Part 3 before Part 1 and Part 2.** Tempting, because an effectful fallback
is the more powerful feature. Rejected on evidence: the three lines in the Problem
section that people actually wrote are all Part 1 and Part 2, and a language whose
constant fallback cannot be written for a document is a language where the powerful
version gets used to work around the missing simple one.

## Opens

* Whether a typed literal should also exist for `text`, which is reachable by
  widening today. Consistency says yes; nothing needs it, so it is not proposed.
* Whether a record literal should be allowed to omit fields that are themselves
  records with all-literal fields. It should not, but the question will be asked.
* What `--record` does with a run that took a fallback whose primary attempt was a
  model call, once GAP-046 lands: the recorded failure has to be matched on replay
  by *digest*, and a digest of a failure is a shape this project has not needed
  before.

## Conformance tests

* A record literal as a fallback, replayed, with the primary attempt failing.
* A `markdown` literal as a fallback, and the artifact's emitted content type
  unchanged by it.
* `json "{ not json"` refused at compile time, with `ING3019`.
* A missing, unknown and duplicated field, each refused with its own code.
* A tool attempt with a tool fallback: both paths' effects present in the
  artifact's declared set, and a policy denying only the fallback's reach refused
  before the run.
* The same program's compile-time minimum with and without an effectful fallback,
  **unchanged**; and a run that takes the fallback stopping at a `steps` ceiling
  one lower than the two attempts need.
* A recorded run that took an effectful fallback, replayed, taking the same path
  and reporting `fallbackTaken` in the same position.
