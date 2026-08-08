# RFC-0006: A second backend, and the portability report

- Status: **Accepted**
- Created: 2026-08-08
- Affects: CLI
- Closes: [GAP-018](../docs/gaps.md#gap-018) if it works, and is the point of it if it does not
- Delivers: M5, the milestone [RFC-0002](0002-runtime-execution-model.md) deferred

## Problem

The project's central claim is that a typed source compiles to something
portable. Every document says so. Nothing demonstrates it, because there is
exactly one program that reads an Agent IR document — our own interpreter — and a
format with one consumer is indistinguishable from that consumer's internal
representation.

[RFC-0002](0002-runtime-execution-model.md) said this plainly when it chose to
build the interpreter first:

> **Ship a backend that generates another runtime's configuration instead.** The
> plan's original M3. Still the right M5, and the multi-target claim is not
> proven until it lands.

So this is not a feature request. It is a **falsification test** for a design
decision made a year ago. If the IR really describes an agent, a second
implementation can be written from the specification. If it describes our
interpreter, writing one will hurt in specific, informative places.

The failure mode this guards against is concrete. `crates/ingot-ir` is a Rust
crate whose types are the IR; `crates/ingot-runtime` reads those types directly.
Nothing has ever forced the IR to be complete, because whenever the interpreter
needed something it could reach for a Rust field rather than for a documented
one. A second consumer that cannot see any of that is the only thing that can
tell us which fields were load-bearing and which were incidental.

## Goals and non-goals

**Goals**

1. A second backend that executes an Agent IR document and **shares no code with
   the reference interpreter.**
2. A **differential test**: the same artifact and the same cassette through both
   implementations, compared byte for byte. A disagreement is a finding, not a
   flake.
3. A **portability report** naming every IR construct a target does not
   implement — before deployment, not during it.
4. **Refuse rather than degrade.** A backend asked to emit something it cannot
   express fails the build. [Runtime 0.1 §2](../specs/runtime/v0.1.md) applied to
   a compiler backend.
5. It runs in CI, on every platform, with no API key.

**Non-goals**

* **Feature parity.** The second backend will not implement everything, and what
  it does not implement is the report's content rather than a defect.
* **A general transpiler.** This emits one target. A plugin system for targets is
  a different problem and needs its own RFC.
* **Integrating an agent framework.** [RFC-0002](0002-runtime-execution-model.md)
  already rejected that as the *interpreter*, for a reason that applies here too:
  it would import the semantics the IR is trying to define. See *Alternatives*.
* **Replacing the reference interpreter.** It stays the oracle. Where the two
  disagree, the specification decides, and if the specification is silent that is
  the finding.

## The target

**Python 3, standard library only, one self-contained file.**

```bash
$ ingot build --target python
heptapus.examples.summarizer.DocumentSummarizer -> target/ingot/DocumentSummarizer.py

$ python3 target/ingot/DocumentSummarizer.py \
    --cassette examples/document-summarizer/tests/cassettes/brief.json \
    --input audience="engineering leads" --input document=@doc.txt
```

Four reasons, in order of how much they matter:

1. **It shares nothing.** Different language, no linked code, no shared types.
   The generated program cannot accidentally inherit a semantic decision from
   `ingot-runtime`, because it has no way to reach one.
2. **It runs in CI.** `python3` is on every runner. That turns the portability
   claim from something asserted into something a test fails over. A backend
   whose output nobody executes proves nothing about the IR —
   [RFC-0002](0002-runtime-execution-model.md) said exactly this about the
   original M3 plan, and it is still true.
3. **Nothing external moves under it.** A framework's configuration schema
   changes on its own schedule and needs that framework installed to test. The
   standard library and one provider's HTTP API are the whole dependency surface.
4. **It is honest about what it is.** One file, no runtime to install. Somebody
   who wants to see whether the IR is implementable can read it.

### What Python is a weak test of

Worth stating plainly, because it is the honest weakness of this choice.

Python can express anything, so the portability report against it will be short:
the constructs listed there are ones **this backend has not implemented**, not
ones the language cannot represent. A *restricted* target — a workflow engine, a
framework with a fixed node vocabulary — is what will exercise the report
properly, and it will need this machinery to exist first.

So: this RFC is a strong test of the **IR** and a weak test of the **report**.
The differential test is the part that closes
[GAP-018](../docs/gaps.md#gap-018); the report is scaffolding built now because
the third backend should not have to build it.

### Written from the specification

The generated program's semantics come from
[Runtime 0.1](../specs/runtime/v0.1.md) and [Agent IR 0.1](../specs/ir/v0.1.md),
**not** from reading `crates/ingot-runtime`. This is a rule about how the work is
done, and it is the whole value of the exercise: a Python file transliterated
from our Rust would agree with it by construction and demonstrate nothing.

Where the two disagree, the disagreement is triaged rather than patched:

| Cause | Fix |
|-------|-----|
| The spec is silent | Amend the spec. This is the most valuable outcome. |
| The spec is clear and Python is wrong | Fix Python. |
| The spec is clear and Rust is wrong | Fix Rust. A bug the reference interpreter has had all along. |

## Target lowering

Every node kind in Agent IR 0.1, and what the Python backend does with it:

| Node | Python |
|------|--------|
| `llm.call` | a provider request, schema-constrained per [Runtime 0.1 §6](../specs/runtime/v0.1.md) |
| `tool.call` | **not implemented** — reported; no MCP client is emitted |
| `agent.call` | **not implemented** — reported; generated programs carry one agent |
| `branch` | `if` |
| `loop` | `while`, with `maxIterations` enforced independently of the guard |
| `parallel` | sequential, like the reference interpreter, for the same reason ([GAP-010](../docs/gaps.md#gap-010)) |
| `approval` | prompt on a terminal; **deny** when there is no terminal |
| `verify` | **not implemented** — reported, not faked. See below. |
| `state.read` / `state.write` | a dict, with a read-before-write error |
| `artifact.emit` | a value in the outputs map |
| `checkpoint` | an event, no resumption ([GAP-008](../docs/gaps.md#gap-008)) |

`verify` is the interesting row. The reference interpreter emits
`Verified { passed: true }` without a verifier existing, which is
[GAP-002](../docs/gaps.md#gap-002) — a thing the register already calls the wrong
thing to say. The Python backend will **not** copy that. It reports `verify` as
unimplemented and refuses to build an artifact containing one unless the operator
passes `--allow-unimplemented`, which is the behaviour
[Runtime 0.1 §2](../specs/runtime/v0.1.md) asks for.

Consequence, and it is deliberate: the two backends **disagree** on an agent that
uses `verify`, and the differential test will say so. That disagreement is
correct. It is GAP-002 showing up as a test failure instead of as a paragraph.

### The report

```text
$ ingot build --target python

report for target `python`
  heptapus.examples.research.ResearchAgent
    verify           1 node   not implemented
                     the target has no verifier execution model; the reference
                     interpreter reports `passed: true` without running one, which
                     this target will not copy (GAP-002)
    parallel         1 node   degraded
                     executed sequentially; the result is identical because a
                     `parallel` body cannot observe another iteration (ING6005)

1 of 2 agents cannot be built for `python` without --allow-unimplemented
```

Three verdicts, and the distinction is the point:

| Verdict | Meaning | Build |
|---------|---------|-------|
| **supported** | the target does this | proceeds |
| **degraded** | the observable result is the same; something else is weaker | proceeds, reported |
| **not implemented** | the target cannot do this | **refused** unless `--allow-unimplemented` |

`--json` emits the same thing machine-readably, so a deployment gate can be
`ingot build --target python --json | jq -e '.unimplemented == []'`.

## IR semantics

None. No new node kind, no new value form, no new document field, no version
move. This RFC adds a consumer, and a consumer that needed the IR to change
would be evidence against the IR rather than a reason to change it.

If writing the backend turns out to need an IR change, that is a finding worth an
RFC of its own — and the finding is *why the IR was incomplete*, which is exactly
what this exercise exists to surface.

## Security and policy impact

The generated program is a **new enforcement point**, which makes this the
security-relevant half of the RFC.

* **It enforces policy itself, from the artifact.** Not because it trusts our
  compiler — [Runtime 0.1 §7](../specs/runtime/v0.1.md) requires every backend to
  re-check, and the person running an artifact is frequently not the person who
  built it. Default-deny holds: an absent rule is a denial.
* **It enforces budgets itself.** Steps and tokens, per §8. Stricter is allowed,
  looser is not.
* **It carries no credential.** The key comes from the environment at call time,
  never from the IR, the generated source, or a cassette. §11.
* **The generated file contains no secret.** It is build output and may be
  committed; a build that could embed a key would make that dangerous. There is
  no code path that writes an environment value into it, and a test asserts that.
* **It is not contained.** `--contained` supervises `ingot exec`, which is our
  binary. A generated Python program is not that and gets no boundary from this
  RFC. It must therefore not be described as sandboxed anywhere, and the report
  says so.

Nothing here can widen what an agent reaches without a grant in its source. The
new risk is the opposite one — a *weaker* enforcement point — and the differential
test covers it: an agent whose policy denies an effect must fail in both
implementations, and that is a named conformance test below.

## Static bounds

Unchanged, and enforced twice rather than once. `budget.steps`, `budget.tokens`
and a loop's `maxIterations` are checked by the generated program from the
artifact's own numbers. `maxIterations` is enforced **independently of the
guard**, so a guard that never falsifies still terminates — that is a conformance
test in both implementations, not a Python detail.

## Compatibility

Additive. No language change, no IR change, no change to any existing command's
behaviour.

* `ingot build --target <name>` is new. `--target ir` is the default and is what
  `ingot build` does today, byte for byte.
* `--allow-unimplemented` and `--json` are new flags on `ingot build`.
* The new crate is `ingot-backend-python`. `ingot-runtime` is untouched, which is
  the test of whether a backend is really a separable thing.

## Alternatives

**Write a second interpreter in Rust.** Would share the `ingot-ir` crate, and
therefore share every assumption baked into those types. The bugs it could find
are a subset of the ones a different language can.

**Generate configuration for an existing agent framework.** Closer to the letter
of RFC-0002, and rejected for the reasons that RFC already gave about the
original M3: the schema is external and moving, and testing it in CI means
installing and running that framework. It also reintroduces the objection RFC-0002
raised against using a framework as the interpreter — it would import the
semantics the IR is trying to define. Worth doing as a *third* backend, once the
report machinery this RFC builds is in place and there is something to compare
against.

**Emit a JSON state machine for no particular runtime.** Nothing executes it, so
it demonstrates nothing. This is the shape of a portability claim that is not
tested, which is the situation we are in already.

**Do nothing.** Leaves the project's central claim unproven while more is built
on top of the IR. Every month of that makes an IR change more expensive, and the
whole point of finding out is to find out while changing it is still cheap.

## Conformance tests

Differential — the same artifact and cassette through both implementations:

- [x] `the_document_summarizer_produces_identical_artifacts_in_both_backends`
- [ ] `a_tool_using_agent_produces_identical_output_in_both_backends`
- [ ] `a_sub_agent_call_produces_identical_output_in_both_backends`
- [x] `the_event_streams_agree_on_kind_and_order`

Enforcement — the second backend must be no weaker than the first:

- [ ] `a_denied_capability_is_refused_by_the_generated_program`
- [ ] `an_unlisted_policy_subject_is_refused_by_the_generated_program`
- [x] `the_step_budget_is_carried_by_the_generated_program`
- [x] `the_token_budget_is_enforced_the_same_way_in_both_backends`
- [ ] `a_loop_stops_at_its_static_bound_even_when_the_guard_never_falsifies`
- [ ] `an_approval_with_no_terminal_is_denied_rather_than_assumed`
- [ ] `a_response_that_violates_the_schema_is_an_error`

The report:

- [x] `an_agent_using_an_unimplemented_construct_is_refused_by_default`
- [x] `an_unimplemented_construct_blocks_the_build_and_says_which_and_how_many`
- [x] `a_degraded_construct_does_not_block_the_build`
- [x] `the_json_report_is_usable_as_a_deployment_gate`

Hygiene:

- [x] `the_generated_program_carries_no_credential`
- [x] `the_generated_program_is_the_same_bytes_every_build`
- [x] `the_generated_program_is_valid_python_before_it_is_run`
