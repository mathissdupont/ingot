# ADR-0007: Containing the run is not blocked on a second backend

- Status: Accepted
- Date: 2026-08-08
- Amends: [ADR-0006](0006-a-policy-enforcing-runner.md)

## Context

[ADR-0006](0006-a-policy-enforcing-runner.md) split the policy-enforcing runner
into two stages and made the second conditional:

> **Stage 2 — the run is contained.** The whole run moves inside, with model
> calls proxied out by a supervisor. Stage 2 is **blocked on condition 1**: it
> makes the runner an IR consumer, and that must not happen while ours is the
> only one.

That reasoning does not hold, and the error is worth recording rather than
quietly editing out of ADR-0006.

**The interpreter has been an IR consumer since M3.** Putting it inside a
container does not create a consumer, does not change the IR, and does not give
us a second implementation to bias against. It runs the same program in a
different place. Whatever pull toward self-favouring exists, it has existed since
the reference interpreter was written, and stage 2 neither adds to it nor
relieves it.

The confusion was between two different things ADR-0002 discusses:

| | |
|---|---|
| **Owning a general execution engine** | ADR-0002's real warning. Orchestration, context management, session state, scheduling. Still out of scope, still governed by ADR-0002's five conditions. |
| **Running the existing interpreter inside a boundary** | Packaging. Changes where a process runs, not what the language means. |

ADR-0006 applied the first one's gate to the second one's work.

There is a second, independent argument that stage 2 was blocked for the wrong
reason: **stage 1 leaves the larger hole open.** In stage 1 the tool servers are
contained and the interpreter is not, so the interpreter — and therefore every
model call, every artifact write, and the whole process the agent runs in — holds
the operator's entire filesystem and the operator's entire network. An artifact
that says `network deny` gets a tool server with no network and an interpreter
with all of it. Whichever way the neutrality question falls, that asymmetry is
not a good place to stop.

## Decision

**Stage 2 is unblocked.** Containing the run needs no second backend.

What remains gated by ADR-0002 condition 1 is what ADR-0006 meant to gate:
extending the runner beyond policy enforcement. The line is unchanged and is
still worth stating plainly — if a feature does not follow from a policy
declaration, it does not belong in the runner, whether the run is contained or
not.

Stage 2's actual prerequisites are engineering, not evidence:

1. **The model call must leave the boundary.** A contained run with
   `network deny` cannot reach a provider, so the host serves completions on the
   run's behalf. The credential therefore stays outside, which is a stronger
   reading of `secrets deny export` than stage 1 achieved: in stage 1 the key is
   merely in a different process, in stage 2 it is on the other side of a
   kernel boundary.
2. **An approval gate must reach a human.** There is nobody inside the box to
   ask, so the gate crosses the same channel and is decided by the operator.
3. **The image must contain `ingot`** and whatever tool servers the manifest
   names, because the tool servers are now children of the contained
   interpreter rather than of the host.

These are settled in [RFC-0005](../../rfcs/0005-the-contained-run.md).

## Consequences

**Good.** `network deny` starts meaning what it says for the agent, not only for
its tools. This is the half of [GAP-001](../gaps.md#gap-001) that stage 1 could
not touch.

**Good.** The credential is outside the boundary as a matter of topology rather
than of discipline. Nothing inside can read `ANTHROPIC_API_KEY` because nothing
inside has an environment containing it, and the process that does cannot be
reached from inside.

**Good.** The supervisor channel is where recording, replay and streaming would
attach later, without the interpreter learning about any of them.

**Bad.** A contained run is a second execution path with different failure modes
— an image that lacks a binary, a protocol version mismatch, a guest that dies
mid-run. ADR-0006 already noted that two paths with different guarantees is a
documentation burden; this makes three, counting `--supervised`.

**Bad.** Sub-agent calls are the open question. Two agents in one program hold
different policies, and one box cannot serve both without widening one of them.
RFC-0005 refuses that case rather than papering over it, which means the
two-agent example does not run contained yet. That is a real limitation and it is
recorded as [GAP-023](../gaps.md#gap-023) rather than described as a design
choice.

**Neutral.** ADR-0002's five conditions are untouched, and three of them are
still unmet. This ADR narrows what condition 1 gates; it does not claim the
condition is satisfied.

## What this does not license

Being wrong about one blocker is not a reason to stop having blockers. The
following still need ADR-0002 condition 1, for the reason ADR-0006 gave and this
ADR does not dispute:

* a scheduler, a queue, or anything that runs an agent on a trigger;
* session or conversation state held between runs;
* context management, retrieval, or memory;
* provider routing policy beyond what the artifact pinned;
* any IR extension whose first consumer would be our own runner.
