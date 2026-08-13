# Writing a backend

A backend takes an Agent IR document and runs it. That is the whole job
description, and this guide is about the parts of it that are not obvious.

You do not need Rust, and you do not need this repository's code. Agent IR is
JSON with a published schema; the runtime contract is prose you can implement
in any language. What you do need is to be right about a small number of things
that are easy to get subtly wrong, and the conformance suite exists so that
"subtly wrong" becomes "one failing case with the clause it broke".

## Two kinds of backend

**An interpreter** walks the node graph and does what each node says. The
reference implementation is one.

**A compiler** turns the document into a program in some other language, which
then does what each node says. The shipped Python target is one:
`ingot build --target python` writes a self-contained Python 3 file.

The contract is identical for both, and so is the suite. Choose by what you
want at the end: a program you can deploy without Ingot present, or a runtime
that can load any artifact.

## What you must read

**[Agent IR 0.1](../../specs/ir/v0.1.md)** — the document. Node kinds, values,
arguments, policy, budget. Then
**[Agent IR 0.2](../../specs/ir/v0.2.md)**, which adds source spans (ignorable)
and a `condition` on `verify` (not ignorable).
[`agent-ir.schema.json`](../../specs/ir/agent-ir.schema.json) is machine-readable
and validating against it first will save you a day.

**[Runtime 0.1](../../specs/runtime/v0.1.md)** — the behaviour. This is the long
one and it is the one that matters. §6 on responses, §8 on budgets, §9 on the
event stream, §10 on cassettes.

Then the revisions, each short: **[0.2](../../specs/runtime/v0.2.md)** (the
`verify` outcome, recorded tool calls, charged cost),
**[0.3](../../specs/runtime/v0.3.md)** (streaming, and the live channel that is
not the event stream), **[0.4](../../specs/runtime/v0.4.md)** (a `verify` that
runs, and a failed check that stops the run),
**[0.5](../../specs/runtime/v0.5.md)** (persistent memory, and a checkpoint a
run can stop at).

## Before anything: run the suite

The cases are built into the `ingot` binary. Download it, point it at a command,
and you have a verdict — there is nothing to clone and no version to keep in
step.

```sh
ingot conform --list                      # what each case requires, and why
ingot conform --backend "python adapter.py"
ingot conform --export ./suite            # write the cases out to read
```

An empty backend fails every case, which is the right starting score and a
useful check that your command is being invoked at all.

## The order to build it in

Each step is a case in the suite, so you can run `ingot conform` and watch them
go green rather than guessing how far you are.

1. **Load and validate the document.** Refuse an IR major version you do not
   implement. Do not ignore fields you do not understand — a node kind you skip
   is a program that quietly does less than the artifact says.
2. **Replay a cassette.** Before any live provider. It is deterministic, it
   needs no key, and it is what the suite runs against. `prose` is the case.
3. **Values.** `literal`, `ref`, `list`, `template`, `unary`, `binary`,
   `builtin`. A `ref` is a scope and a path; `path[0]` is the root and the rest
   are field reads. `structured` is the case.
4. **Control flow.** `branch`, `loop`, `parallel`. `branch` is the case; the
   other two have none yet.
5. **`verify`.** Evaluate `condition` when it is there; report `notPerformed`
   when it is not. Three cases.
6. **Everything else.** Tools, sub-agents, approvals, state, checkpoints. No
   cases yet — write them as you go, and send them.

## The five things that are easy to get wrong

### A partial answer is not an answer

If you stream, you **must** assemble the value from the finished response and
validate it whole, through the same code path a non-streamed response takes
([Runtime 0.3 §3](../../specs/runtime/v0.3.md)).

Do not write a second parser for the streaming path. Accumulate the pieces into
the payload shape your non-streaming call returns, and hand *that* to the one
reader you already have. This is the single most valuable structural decision in
a backend: it makes "a streamed call and a whole-body call produce identical
values and identical errors" true by construction rather than by testing.

Truncation is the tempting case. The text on screen when a call hits its limit
is the beginning of a real answer, well formed and on topic. "The model stopped
here" is still not "the model finished", and a truncated JSON object that closes
early and validates is a wrong answer that passes every check.

### A delta is not an event

Live text and the event stream are two channels
([Runtime 0.3 §2](../../specs/runtime/v0.3.md)). A fragment must never enter the
event stream, never be recorded in a cassette, and never be asserted on.

The reason is replay. A recorded run against a service that sent forty fragments,
replayed with none, would differ by forty events — and then the stream is not
assertable, position matching breaks, and cross-backend comparison is over.

If you render both to one place, make them distinguishable without heuristics.
Both shipped backends write JSON to standard error, and a delta line carries no
`event` key: **the event stream is exactly the set of lines that have one.**

### The output ceiling belongs to the transport

16,000 output tokens for a whole-body call, 64,000 for a streamed one
([Runtime 0.3 §4](../../specs/runtime/v0.3.md)). Ask your provider which it is.

**An artifact must not be able to select a ceiling**, and you must not give it a
way to: the same artifact against a streaming provider and a whole-body one is
the same program, and only the second has to keep the smaller number. The Python
target used to write the number into every generated call, which was a
conformance bug nobody noticed until the ceiling had two possible values.

### Events carry no clock

No timestamps, no durations, ever ([Runtime 0.1 §9](../../specs/runtime/v0.1.md)).
This is what makes a run record comparable — to a replay of itself, and to
another backend's record of the same artifact.

It is also easy to violate by accident, because a duration is the obvious thing
to add to a `modelCall` event. Put it somewhere else. The reference CLI keeps
wall-clock on the run record's own framing lines, never in an event.

### A denial is the default

Policy is default-deny and re-checked at run time, not only at compile time
([Runtime 0.1 §7](../../specs/runtime/v0.1.md)). Whoever runs an artifact is
frequently not whoever built it, so a backend that trusts the compiler's
decision has removed the check that mattered.

An approval nobody can grant is a denial. An unattended run stops rather than
proceeding — the safe direction, and not the same as having a decider.

## Writing the adapter

An adapter is what the suite talks to. It reads a request file and runs one
case:

```json
{
  "conformance": "0.1",
  "artifact": "/abs/path/agent.ir.json",
  "cassette": "/abs/path/cassette.json",
  "inputs": { "topic": "compilers" },
  "outDir": "/abs/path/out"
}
```

Refuse a `conformance` version you do not implement. Run the artifact with the
cassette as your only source of completions. Write the event stream to standard
error as JSON Lines, the artifacts into `outDir`, and exit non-zero if the run
failed.

[`specs/conformance/tools/python-adapter.py`](../../specs/conformance/tools/python-adapter.py)
is a worked example, and it is forty lines. Then:

```sh
ingot conform --backend "python my-backend/adapter.py"
```

Every case names the clause it holds you to, so a failure tells you what to read
rather than only that something differs. `--export` writes the case out when
reading the fixture is faster than reading the clause.

## When you disagree with the reference

Sometimes you will be right. The first time the conformance suite ran across
both shipped backends it found the reference interpreter writing `response_type`
where the specification and the second backend both said `responseType` — a
divergence no single-implementation test could have seen, and the reference was
the one that was wrong.

So: **the specification decides.** If it is silent, that is the most valuable
thing you can find, and an issue saying so is worth more than a workaround.

It keeps happening. Adding cases for `parallel map`, `loop`, `checkpoint` and an
exhausted budget turned up three more: `parallel map` was collecting `null` for
every element in **both** backends, a loop guard over working memory never
changed so only `max` ever stopped the loop, and the two backends numbered loop
iterations differently because nothing said which. Two implementations and a
shared suite is the only arrangement that finds these.

## What is not settled yet

Do not build against these expecting them to hold:

- **Tools are MCP over stdio only** ([GAP-007](../gaps.md#gap-007)). Remote
  servers need a local proxy.
- **`checkpoint` cannot be resumed from** ([GAP-008](../gaps.md#gap-008)). It is
  an event and nothing more.
- **No persistent memory** ([GAP-014](../gaps.md#gap-014)). Working memory is
  ephemeral and dies with the run.
- **A verifier can only inspect the shape of a value**
  ([GAP-034](../gaps.md#gap-034)). It cannot reach anything.

[The gap register](../gaps.md) is the whole list, and it is kept honest.
