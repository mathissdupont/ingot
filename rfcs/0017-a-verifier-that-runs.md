# RFC-0017: A verifier that runs

- Status: Draft
- Author(s): Heptapus Group
- Created: 2026-08-12
- Affects: language, IR, runtime, CLI

## Problem

`verify` is the only statement in Ingot that does nothing.

This is from `examples/research-agent/main.ing`, unabridged:

```ingot
/// Rejects a draft that does not cite enough distinct sources.
verifier CitationCheck(draft: markdown, min_sources: int)
```

```ingot
    draft = ask<markdown>("Produce a source-grounded report. Cite every claim.")
    verify CitationCheck(draft, min_sources: 8)
    emit report = draft
```

It parses, type-checks, and lowers to a `verify` node carrying the name and the
arguments. The doc comment says the verifier *rejects* a draft. Nothing is
rejected. At run time the node emits:

```json
{"event":"verified","node":"n7","verifier":"CitationCheck","outcome":"notPerformed"}
```

and the run continues to the `emit` regardless.

`ingot check` warns about it (`ING6006`). The run says plainly that the property
is unchecked. Nothing is misleading — and nothing is verified.
[GAP-030](../docs/gaps.md#gap-030) is the entry for this, in class **Absent**,
and it is the one gap that a consumer of a run cannot work around: an artifact's
declared checks are the part a third party would most want to rely on, and they
are the part that has never run.

The register said closing it needs a design first, because "a verifier is either
a tool call, a model call with a rubric, or host-provided code, and those have
different security stories". This RFC is that design.

## What the specification had already decided

The register listed three candidates. The language specification had already
eliminated one of them, in [Language 0.1 §5.2](../specs/language/v0.1.md):

> A verifier is a **deterministic** check the runtime performs on a value.

A model call with a rubric is not deterministic. It costs tokens, needs a
budget, needs `model_access`, and — the part that matters most here — its
verdict cannot be reproduced from a run record without a cassette entry, which
would make a check's outcome depend on a file that travels beside the artifact
rather than inside it. Whatever a model-graded assessment is, it is not what
this language calls a verifier. It is an `ask`, and Ingot already has one.

That leaves two candidates that differ only in **where the check's code lives**:
outside the artifact (a tool, or host-provided code) or inside it. This RFC
puts it inside.

## Goals and non-goals

Goals:

- A verifier declares a body, and that body is executed.
- The body is pure, total and deterministic, so a check's outcome is a function
  of the events that precede it and survives replay unchanged.
- No new effect, no new budget, no new capability, no new IR node kind, and no
  change to `agent-ir.schema.json`.
- Every backend that can evaluate an IR value can perform such a check, so
  `notPerformed` stops being a universally available answer.
- Existing source keeps working, unchanged, with unchanged behaviour.

Non-goals:

- Checks over prose. `len(draft.sources) >= 3` inspects a field; nothing here
  reads markdown and counts citations in it.
- Checks that reach outside the run — resolving a URL, querying a registry.
- Model-graded assessment, per the section above.
- Host-provided verifier code, in any language, shipped with or beside an
  artifact.
- Recursion, and verifier-to-verifier calls.

The first two non-goals are not permanent. They are
[the gap this RFC leaves open](#what-this-does-not-close), and the invariant in
[Determinism](#determinism) is written so a later slice can close them without
reopening this one.

## Proposed syntax

A verifier gains an optional body: `=` followed by a single expression of type
`bool`. This mirrors `fn` ([Language 0.2 §7](../specs/language/v0.2.md)) with the
return type omitted, because a verifier's return type is the only one there is.

```ingot
language 0.2

type source {
  url: string
  title: string
}

type draft {
  body: markdown
  sources: source[]
}

verifier MinSources(d: draft, min: int) = len(d.sources) >= min
verifier NotEmpty(d: draft)             = len(d.body) > 0

agent Researcher(topic: string) -> report<markdown> {
  model requires { structured_output }

  budget {
    steps <= 8
    tokens <= 20000
  }

  policy {
    network deny
    filesystem_write deny
  }

  flow {
    found = ask<draft>("Research ${topic}. Cite your sources.")
    verify MinSources(found, min: 3)
    verify NotEmpty(found)
    emit report = found.body
  }
}
```

The shape of that flow is not incidental. An artifact's content type must be
`text`, `markdown`, `json` or `file` (`ING3008`), so a record cannot be emitted
directly — the model answers into a typed binding, and the `emit` takes a field
out of it. Which means the natural place for a `verify` is exactly where a check
can still do something: between the answer and the publication.

Grammar, replacing the production in
[Language 0.1 §5.2](../specs/language/v0.1.md):

```ebnf
verifier-decl = "verifier" , IDENT , params , [ "=" , expression ] ;
```

A body requires `language 0.2` or newer. Under `language 0.1` a body is rejected
with `ING1020`, the code that already covers a construct used below the version
that introduced it.

**A bodyless verifier stays legal**, keeps its current meaning, keeps its
`ING6006` warning, and still reports `notPerformed`. That is the whole migration
story: this RFC adds a way to say what a check *is*, and takes nothing away from
source that does not say it.

### What a body may contain

The same expression subset as a helper body, which the checker already enforces
via `ING2019`:

- parameters;
- literals and lists;
- field and path reads from parameter values;
- builtin pure functions such as `len`;
- unary and binary operators;
- calls to declared `fn` helpers.

Not permitted: `ask`, `call`, `parallel map`, state reads, `emit`, and calls to
other verifiers. The helper call is allowed because Language 0.2 §7 forbids
helper-to-helper calls, so verifier→helper is depth-1 and terminates by
construction. If helper composition is ever added, this permission needs
revisiting in the same RFC that adds it.

The body's inferred type must be `bool`. A body of any other type is `ING2020`,
a new code: *a verifier body must decide, and this one produces `int`*.

## IR semantics

The `verify` node gains `condition`: the verifier body, with the call's
arguments substituted for its parameters, lowered as a pure `Value` — exactly
the substitution `lower_function_call` already performs for helpers.

Today's node for `verify MinSources(found, min: 3)` — copied from a real build,
with the `sourceSpan` elided — is:

```json
{
  "id": "n1",
  "kind": "verify",
  "verifier": "MinSources",
  "args": [
    { "name": "d",   "value": { "kind": "ref", "scope": "binding", "path": ["found"] } },
    { "name": "min", "value": { "kind": "literal", "type": "int", "value": 3 } }
  ],
  "next": "n2"
}
```

Under this RFC it gains one key:

```json
  "condition": {
    "kind": "binary",
    "op": ">=",
    "lhs": {
      "kind": "builtin",
      "name": "len",
      "args": [ { "kind": "ref", "scope": "binding", "path": ["found", "sources"] } ]
    },
    "rhs": { "kind": "literal", "type": "int", "value": 3 }
  }
```

Substitution is what turns `len(d.sources)` into a `ref` with
`path: ["found", "sources"]`: the parameter `d` is replaced by the argument it
was called with, and the body's field read extends that argument's path. `min`
disappears entirely, into the `literal` on the right. Both survive in `args`,
which is why `args` is kept.

Three things are deliberate here.

**`condition` is not a new field.** It is already declared on the node object in
`agent-ir.schema.json`, where `branch` uses it and the object is
`additionalProperties: false`. A `verify` node carrying a `condition` validates
against the **existing, unmodified schema**. No IR version needs to move for
validation to pass; §"Compatibility" covers whether one should anyway.

**`args` stays**, redundantly, even though every argument also appears inlined
inside `condition`. It is what the `verified` event's reader and the canvas use
to show *what was checked with what*, and dropping it to avoid duplication would
trade a readable artifact for a shorter one.

**`verifier` stays**, because the event reports a name and a name is what a
consumer indexes on. The body says what the check is; the name says which check
it is.

A `verify` node **may** omit `condition` — that is a bodyless verifier, and it
is the only case in which a backend may report `notPerformed`.

## Target lowering

**Reference interpreter.** `run_verify` evaluates `condition` with the same
evaluator it already uses for `branch` guards, and emits `passed` or `failed`
instead of the unconditional `notPerformed` it emits today. Absent `condition`,
behaviour is byte-identical to the current implementation.

**Python backend.** The same: the prelude's value evaluator already handles
every `Value` form a verifier body can produce, because the subset is the one
helper inlining already emits into ordinary bindings. This adds no portability
limitation, which is the point of choosing a form the backends already execute.

**Conformance tightens.** [Runtime 0.2 §1](../specs/runtime/v0.2.md) permits any
backend to report `notPerformed` on the grounds that "Agent IR records a
verifier's name and signature and carries no representation of the check
itself". That sentence stops being true. Restated for Runtime 0.4:

> A backend **must** perform a `verify` node that carries a `condition`, and
> **must** report `passed` or `failed` accordingly. `notPerformed` is reserved
> for a `verify` node with no `condition`. A backend that reports `notPerformed`
> for a node carrying a `condition` is non-conformant.

`notPerformed` remains *unknown* rather than a failure, unchanged from 0.2.

## Determinism

A verifier body is a pure expression over values already bound by earlier
events. Its outcome is therefore a function of the run's own event prefix: no
cassette entry, no provider, no clock, no host state. A replay reproduces the
`verified` event byte for byte for the same reason it reproduces a `branch` —
the same inputs reach the same evaluator.

This generalises into the rule that governs any future verifier kind:

> **A `verified` outcome must be derivable from the run record alone.**

A tool-backed verifier can satisfy this, because a tool call is recorded and
Cassette 0.2 replays it. A model-graded one cannot without making a check's
verdict depend on a file outside the artifact. Host-provided code cannot at all.
The rule is what lets a later slice add reach without giving up replay, and what
rules out the two candidates this RFC declines.

## Security and policy impact

**No new effect, capability or grant.** A pure expression over already-bound
values reaches nothing: no network, no filesystem, no secret, no model. There is
nothing for a policy to permit and nothing default-deny could deny, so
default-deny holds vacuously and the effect set of a `verify` node is empty
before and after this change.

**A verifier gets no policy of its own**, now or later. When a future slice
gives verifiers reach, the effects belong to the `verify` site and are checked
against the *agent's* policy, exactly as a `call` is. A second policy surface
would be a second place to be wrong about what an agent can reach, and the
project has one such surface on purpose.

**Nothing here can widen reach without an explicit grant in source**, because
nothing here can reach at all.

One consequence worth stating: a verifier body is source, and it is also
inlined into the canonical IR. `package::scan_project` already scans both —
every project source and every agent's canonical JSON — so a credential written
into a body is refused at both points it could travel, with no new scanning
surface and no change to the scanner. Nothing about a body makes a credential
more likely there than in any other expression.

## Static bounds

A verifier body is inlined, like a helper body, and consumes **no step budget**.
A `verify` node's step cost stays what it is today: the cost of its arguments.

This is safe because a body has no loop, no recursion, no call to another
verifier, and at most a depth-1 call to a helper that itself cannot call one.
The evaluation is therefore bounded by the source text, statically, at compile
time — the same argument Language 0.2 §7 makes for helpers.

Token cost is unchanged: a verifier makes no model call, so it appears in
neither the compile-time cost ceiling nor the run's spend.

## Compatibility

**Existing source.** Unchanged, in both senses: it still compiles, and it still
means what it meant. Every verifier in existing source is bodyless, so every
`verify` node still lowers without `condition` and still reports
`notPerformed`. No program changes meaning. This is a minor language change.

**Existing IR documents.** Unchanged and still valid; `condition` is optional
and absent from all of them. New documents validate against the current schema
without modification, since `condition` is already a declared node property.

**Versions.**

- **Language 0.2** gains §5.2's optional body. No new version.
- **Agent IR 0.2** gains a paragraph documenting `condition` on `verify`. The
  schema file does not change. Whether this warrants IR 0.3 is a judgement call:
  the encoding is additive and every 0.2 reader ignores an unknown-to-it field
  it already knows how to parse. It does not warrant one; the paragraph does the
  work.
- **Runtime 0.4** is needed, because the conformance statement above *is* a
  breaking change for a backend: an implementation that was conformant while
  answering `notPerformed` to everything stops being conformant. That is the
  honest place to record it, and it is one section long.

## Failure semantics

This is the decision this RFC most needs settled, and it is the one place where
two defensible answers lead to materially different implementations. Both are
stated here in full; the recommendation follows.

### A. A failed check ends the run

`RunError::VerificationFailed { node, verifier }`, reported like an approval
denial: an outcome, not a crash. Everything already emitted stays in the record,
the `verified: failed` event is written before the run ends, and no statement
after the failing `verify` executes.

*For:* the project's posture is to refuse rather than proceed under a claim that
does not hold. A check that stops nothing is documentation with a type
signature, which is the exact phrase [GAP-030](../docs/gaps.md#gap-030) uses to
describe the thing being fixed — closing the gap by making the check *run* but
still not *matter* would be closing it on a technicality.

*Against:* [Runtime 0.2 §1](../specs/runtime/v0.2.md) states that "a `verify`
node still does not stop a run", so this is a documented-behaviour change. And
it interacts badly with the common flow shape: `emit` usually comes first, so
ending the run does not prevent publication of the thing that failed — it only
prevents whatever came after.

### B. A failed check is reported, and the run outcome carries it

The run continues. The `verified: failed` event is written, and the run's final
outcome becomes distinguishable from a clean one — `completedWithFailures`
rather than `completed` — so a consumer reads one field rather than scanning
events.

*For:* a consumer gets strictly more information: the artifact *and* the verdict
against it, and can decide for itself. A market refusing to release payment on a
failed check does not need the producer to have crashed. It also matches the
spec as written, and matches what `verify` does today in every respect except
that the answer is now real.

*Against:* an agent can complete having failed its own declared property, and
the distinction lives in a field a careless consumer will not read.

### Recommendation: A, plus a lint that makes it mean something

Take A, and add `ING6007`: **a `verify` whose argument has already been emitted,
in whole or in part, cannot prevent its publication.** Warning, not error, with
the fix in the message — move the `verify` above the `emit`, as in this RFC's
example, where `found` is bound, checked, and only then does `found.body` leave.

"In part" is what makes the lint work rather than being trivially satisfied: a
record cannot be emitted, so the emitted value is always a field of the verified
one, and a lint that compared whole bindings would never fire.

The lint is what makes A worth having. Without it, A is a fatal error that fires
after the damage; with it, the language teaches the ordering in which a check is
a gate rather than a postmortem, and the fatal outcome is the thing that makes
the ordering worth learning. B has no equivalent — under B the ordering never
matters, so nothing ever teaches it.

The Runtime 0.2 sentence is a real cost and it should be paid explicitly in
Runtime 0.4 rather than quietly: nothing that runs today can reach `failed`, so
no existing run's behaviour changes, and this is the last moment when the
semantics of a reachable `failed` are free to choose.

## Alternatives

**Do nothing.** GAP-030 stays open, and `verify` stays the one statement in the
language that is decoration. The gap is currently the register's oldest Absent
entry with no workaround beyond "write a tool instead", which is advice to not
use the feature.

**A verifier is a tool call.** Reuses the effect model, the MCP host and
Cassette 0.2's replay, and could check prose and reach the network — everything
this RFC's non-goals exclude. It fails on the definition: a tool call is not
deterministic, and a check whose implementation lives in a process outside the
artifact means an artifact's declared properties cannot be evaluated by reading
the artifact. It is also the workaround that already exists today, and promoting
a workaround to the design would leave `verify` and `call` differing only in
which word you type.

**A verifier is a model call with a rubric.** Excluded by Language 0.1 §5.2, as
argued above. Worth noting separately that this is the option a reader coming
from LLM tooling expects, so Runtime 0.4 should say why it was not taken rather
than leaving the absence to be read as an oversight.

**Host-provided verifier code**, shipped as WASM or as a plugin. Genuinely
general and genuinely portable, and it is what a mature version of this feature
might look like. It needs a code-shipping format, a sandbox, a determinism story
and a supply-chain answer — four designs, each larger than this one, to make the
first check run. It is also the option most likely to be regretted: once
arbitrary code can produce a `verified` event, the event means only as much as
the code shipping it, and the invariant in [Determinism](#determinism) is gone.

**Add `condition` as a new node kind rather than a field.** A `check` node
between `branch` and `verify` would need a new evaluator construct in every
backend and a schema change, to express something the existing `condition`
already expresses.

**Require a body.** Simpler language, no `notPerformed` path, no `ING6006`. It
breaks every existing program that declares a verifier, to remove a state that
Runtime 0.2 deliberately introduced so backends could be honest about what they
cannot do. Backends that cannot do it will exist again.

## What this does not close

After this, a verifier can check the *shape* of a value and nothing else. The
motivating example — `CitationCheck(draft: markdown, min_sources: int)`, which
is the one in `examples/research-agent/main.ing` and in the register — is still
not expressible, because counting citations in markdown means reading prose.

What is expressible is the same agent restructured: ask for a record with a
`sources` field, verify the record, emit its markdown field. That is a real
constraint on the author and it is not free — the model now has to answer in a
structured shape, which needs `structured_output` and gives it less room. It is
also a better program, because a check over a field is a check the compiler
understands and a check over prose is a hope. But it is a restructure, not a
drop-in, and the example in the repository will have to change to demonstrate
the feature.

GAP-030's text is "a verifier cannot be executed at all", and that stops being
true, so **GAP-030 closes** and a narrower entry replaces it:

> **GAP-034 — A verifier can only inspect the shape of a value.** Class:
> **Absent**. A verifier body is a pure expression, so a check can test fields,
> lengths and thresholds. It cannot read prose, call a tool, or reach the
> network. A property that needs any of those must still be a `tool` call, whose
> result is a value rather than a verdict. Closing it needs a verifier kind that
> has effects while satisfying the record-derivability invariant in RFC-0017.

Filing the narrower gap in the same change that closes the wider one is the
point of the register: the fix is real, and the honest description of what is
left is smaller than what was there before.

## Conformance tests

- [ ] `verifier_with_a_body_parses_and_formats`
- [ ] `language_0_1_rejects_a_verifier_body`
- [ ] `verifier_body_must_be_bool`
- [ ] `verifier_body_rejects_ask`
- [ ] `verifier_body_rejects_a_tool_call`
- [ ] `verifier_body_rejects_calling_another_verifier`
- [ ] `verifier_body_may_call_a_pure_helper`
- [ ] `verify_lowers_the_body_into_condition_with_arguments_substituted`
- [ ] `verify_keeps_args_and_verifier_name_beside_condition`
- [ ] `a_verify_node_with_a_condition_validates_against_the_unchanged_schema`
- [ ] `bodyless_verifier_still_lowers_without_condition`
- [ ] `bodyless_verifier_still_reports_not_performed`
- [ ] `bodyless_verifier_still_warns_ING6006`
- [ ] `a_passing_check_emits_verified_passed`
- [ ] `a_failing_check_emits_verified_failed_before_the_run_ends`
- [ ] `a_failing_check_ends_the_run_with_a_distinct_outcome`
- [ ] `statements_after_a_failing_verify_do_not_execute`
- [ ] `artifacts_emitted_before_a_failing_verify_stay_in_the_record`
- [ ] `verify_after_emit_of_the_same_value_warns_ING6007`
- [ ] `a_verify_consumes_no_step_budget_beyond_its_arguments`
- [ ] `replaying_a_run_reproduces_every_verified_event_byte_for_byte`
- [ ] `the_python_backend_and_the_interpreter_agree_on_every_verifier_example`
- [ ] `the_research_agent_example_verifies_something`
- [ ] `no_reference_example_still_reports_not_performed`
