# Conformance suite

What a backend must do, as something you can run.

```sh
ingot conform                                                    # the reference
ingot conform --backend "python my-backend/adapter.py"           # yours
ingot conform --list                                             # what it requires
```

The reference interpreter is **not privileged**. It reaches the suite through
the same adapter a third party writes, so "the reference conforms" is a claim
the suite checks rather than one the runner assumes. Both shipped backends —
the Rust interpreter and the generated Python program — are held to the same
seven cases by the same code.

This works. The first time the suite ran across both backends it found a real
divergence: the reference interpreter was writing `response_type` where the
specification and the second backend both said `responseType`. No
single-implementation test could have seen it.

## The contract

A backend under test is **a command**. The suite writes a request file and runs
the command with that file as its only argument:

```sh
<your command> /tmp/ingot-conform-1234/prose/request.json
```

```json
{
  "conformance": "0.1",
  "artifact": "/abs/path/agent.ir.json",
  "cassette": "/abs/path/cassette.json",
  "inputs": { "topic": "compilers" },
  "outDir": "/abs/path/out"
}
```

One file rather than flags, because a backend's command line is its own
business. An adapter that reads this is forty lines in any language, and the
alternative — a template with placeholders — is a thing every backend spells
differently.

Given that file, a backend **must**:

1. **refuse a `conformance` version it does not implement**, rather than guess
   which fields changed;
2. run `artifact`, taking every completion from `cassette` and **nothing else**
   — no network, no key, no fallback;
3. write the **event stream to standard error**, one JSON object per line;
4. write the run's artifacts into `outDir`, named `<output>.<extension>`;
5. **exit 0** if the run finished, **non-zero** if it failed.

Standard output is left alone: it carries a run's own writing, and splicing
events into it would break any pipeline reading them.

### What is compared

| Compared | How |
|---|---|
| The event stream | In order, field for field, against `expected/events.jsonl` |
| Artifacts | Byte for byte, against `expected/outputs/` |
| The outcome | Finished or failed, as `case.toml` declares |
| The reason a failing case failed | The failure must name the fragment in `failure-names` |

**A line with no `event` key is not an event.** It is the live channel, which
[Runtime 0.3 §2.1](../runtime/v0.3.md) forbids a conformance test from
asserting on: a run against a service that sent forty fragments, replayed with
none, would differ by forty lines. Dropping those lines here is that rule made
executable rather than merely written down. It is also why the discrimination
is `event`-key-or-not: §2.2 requires a consumer to tell the two channels apart
without heuristics, and this is a consumer doing exactly that.

### What a backend may differ on

Two fields, and the list is short on purpose — every entry is a licence to
disagree, and an unexamined one is how a suite stops testing the thing it was
written for.

| Event | Field | Why |
|---|---|---|
| `runStarted` | `provider` | It names the implementation that answered. A backend replaying a cassette has replayed it, whatever it calls the thing doing the replaying. |
| `runFailed` | `reason` | A failure's prose. No specification standardises the wording of an error, and one that did would make every improvement to an error message a conformance break. What a case requires is that the run failed *and named the thing that failed* — `failure-names` is what checks that. |

Everything else is compared exactly, including `model`, `usage`, node ids and
order. [Runtime 0.1 §9](../runtime/v0.1.md) forbids timestamps and durations in
events for precisely this reason: the sequence is comparable across
implementations.

## What conformance is not

**Not identical output.** Two runtimes given the same agent and a live model
will not produce the same text, and the suite must not pretend otherwise. Every
case replays a cassette, so the model's contribution is fixed and what is left
to compare is the backend's own behaviour.

**Not a claim about a backend's whole surface.** A case the suite does not have
is a property nothing checks. The set below is small and each case says what it
holds you to; adding one is the way to make the suite mean more.

**Not a substitute for the specification.** A case cites the clause it enforces
and cannot restate it. Where a case and a clause disagree, the clause wins and
the case is a bug.

## Portability levels

A report is expressed in these, and conformance is the last one.

| Level | Guarantee |
|---|---|
| P0 Parse | the source is a valid program |
| P1 Structural | the target can represent the agent's structure without loss |
| P2 Operational | the required model and tool capabilities are available |
| P3 Policy | the target can enforce the declared permissions and budgets |
| P4 Conformance | the defined behaviour tests pass |

`ingot build --target <backend> --json` reports P1 for the shipped Python
target: `.unimplemented == []` is a deployment gate. The suite here is P4.

## The cases

Each is a directory under `cases/`. `ingot conform --list` prints them with the
clauses they pin.

| Case | What it holds a backend to |
|---|---|
| `prose` | One model call replayed from a cassette, and the artifact it emits |
| `structured` | A record-typed answer, validated whole, with an artifact from one of its fields |
| `branch` | One arm runs and the other leaves no trace |
| `verify-passes` | A check that holds reports `passed` and the run carries on |
| `verify-fails` | A check that fails says so, *then* ends the run, and what followed never runs |
| `verify-no-body` | A verifier with no body is `notPerformed` — never a pass |
| `replay-mismatch` | A replay whose inputs the recording never saw is refused, not answered from the wrong interaction |

### What is missing, and known to be

No case yet covers a tool call, a sub-agent, a policy denial at run time, a
budget being exhausted, or a `checkpoint`. Tools and sub-agents are not
implemented by the Python target, so a case for them would test one backend;
the others are simply not written. Saying so is the point — a suite that
implied coverage it does not have would be worse than a small one.

## Writing an adapter

`tools/python-adapter.py` is a worked example, and it is short on purpose.
[The backend author's guide](../../docs/guide/writing-a-backend.md) is the
longer version: what Agent IR asks of you, what the runtime contract requires,
and the order to build it in.

## Regenerating the fixtures

A case is authored as three files a human wrote — `main.ing`, `case.toml` and
`bless.toml` — and three the tool derives:

```sh
cargo build -p ingot-cli
python specs/conformance/tools/bless.py            # every case
python specs/conformance/tools/bless.py prose      # one
```

Deriving the expectation is what keeps a case honest: it is what the reference
interpreter *did*, not what somebody typed out, and `case.toml` records which
clause makes that the right answer. Completions come from a local stub speaking
the OpenAI-compatible shape — including its event-stream form, because both
providers stream — so a cassette is recorded with real request digests and a
replay of it is a real replay. Nothing touches a network.

A blessed change that nobody can justify against a clause is a regression that
got written down. Read the diff.
