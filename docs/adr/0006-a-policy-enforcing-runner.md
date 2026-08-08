# ADR-0006: A policy-enforcing runner is in scope; a general runtime is not

- Status: Accepted, amended
- Date: 2026-08-07
- Amends: [ADR-0002](0002-compiler-not-runtime.md)
- Amended by: [ADR-0007](0007-containing-the-run-is-not-blocked-on-a-second-backend.md),
  which corrects the "Conditions for going further" section below. Stage 2 was
  blocked on a second backend for a reason that does not hold; the rest of this
  ADR stands.

## Context

[ADR-0002](0002-compiler-not-runtime.md) decided to build a compiler and
delegate execution, and listed five conditions under which owning a runtime
would become justifiable. That list exists so this conversation could be had
with evidence rather than enthusiasm. Taking it condition by condition, as of
2026-08-07:

| # | Condition | State |
|---|-----------|-------|
| 1 | At least two independent backends work end to end | **No.** One: our own interpreter ([GAP-018](../gaps.md#gap-018)). |
| 2 | Agent IR 0.1 is stable and has conformance fixtures | **Partly.** Stable and golden-tested; no fixture package for other implementers ([GAP-017](../gaps.md#gap-017)). |
| 3 | A user-validated need exists that no target runtime can express | **Yes.** No agent runtime derives its sandbox from a declared capability set. Every one of them takes the sandbox as separate configuration, which is exactly how a policy and a boundary drift apart. |
| 4 | The scope is genuinely narrow — a policy-enforcing runner, not a general execution engine | **Yes, if held to it.** ADR-0002 names this shape itself. |
| 5 | A team exists that can carry provider adapters and MCP lifecycle maintenance indefinitely | **No.** |

Two conditions argue for, three against, and the two arguing for are the two
about *what the thing is*. The three against are about *how much we can carry*.

There is also a real hazard ADR-0002 names directly: a project with its own
runtime has an incentive to make that runtime the good target, which is the
opposite of neutrality. Building a runner before a second backend exists would
bias the IR toward ourselves at precisely the moment nothing could detect it.

## Decision

A **policy-enforcing runner** — "Ingot Containers" — is in scope. A general
execution engine is not, and ADR-0002 remains the reference for saying so.

The narrowness is made structural rather than promised, in two ways.

**First, by what it does.** The runner's only job is to turn an artifact's
`policy` block into an enforced boundary. It adds no orchestration, no context
management, no session state, no provider routing, no scheduler. If a feature
does not follow from a policy declaration, it does not belong in the runner.

**Second, by where it sits.** It is delivered in two stages, and the first stage
is deliberately built so that it *cannot* bias the IR:

* **Stage 1 — tool servers are contained.** The interpreter stays where it is,
  on the host, holding the model credential. Each MCP tool server runs inside a
  boundary derived from the artifact's policy. This adds **no new consumer of
  the IR**, so the neutrality hazard does not arise, and it addresses the part
  that actually carries risk: a tool server is somebody else's program doing
  effectful work, while the interpreter is ours following a checked artifact.
  This also satisfies `secrets deny export` structurally — the credential is
  outside the boundary because the model call is.

* **Stage 2 — the run is contained.** The whole run moves inside, with model
  calls proxied out by a supervisor. Stage 2 is **blocked on condition 1**: it
  makes the runner an IR consumer, and that must not happen while ours is the
  only one.

  > **Amended.** This is wrong, and
  > [ADR-0007](0007-containing-the-run-is-not-blocked-on-a-second-backend.md)
  > says why: the interpreter has been an IR consumer since M3, so containing it
  > adds no consumer. Stage 2 is unblocked and delivered in
  > [RFC-0005](../../rfcs/0005-the-contained-run.md).

## Consequences

**Good.** `network allow ["arxiv.org"]` and `filesystem_read allow ["src"]` stop
being statements of intent. The declaration a reader sees in the source is the
one the kernel enforces, which is the whole argument for having written it
down. [GAP-001](../gaps.md#gap-001) becomes closable.

**Good.** Stage 1 is incremental. `ingot-mcp` already spawns tool servers as
child processes with a minimal environment; giving those processes a boundary is
a bounded change in one crate, behind the same `ToolHost` trait.

**Bad.** Condition 5 is still unmet and this ADR does not change that. A runner
is a permanent maintenance obligation: container runtimes change, and a sandbox
that is subtly wrong is worse than no sandbox because it is trusted. If the
maintenance cannot be carried, the honest move is to withdraw the feature rather
than let it rot — and to say so in [the gap register](../gaps.md).

**Bad.** Containers are not available everywhere Ingot runs, so the runner is
optional and `ingot run` without it must keep working exactly as it does now,
with the policy checked but not enforced. Two paths with different guarantees is
a documentation burden and a source of false confidence; the report the runner
prints must make plain which one is in effect.

**Neutral.** ADR-0002's positions on tool protocol, messaging and distribution
are unchanged. This ADR moves one line of its table — sandboxing — from "the
runtime's, initially" to ours, and nothing else.

## Conditions for going further

> **Amended by [ADR-0007](0007-containing-the-run-is-not-blocked-on-a-second-backend.md).**
> The paragraph below applies to making the runner a general execution engine. It
> does **not** apply to stage 2, which was gated here by mistake.

Stage 2, and anything that would make the runner a general execution engine,
still needs ADR-0002 condition 1: **a second, independent backend, working end
to end.** That is not a formality. It is the only thing that can tell us whether
the IR describes an agent or describes our interpreter.
