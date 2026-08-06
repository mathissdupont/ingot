# ADR-0002: Build a compiler, not a runtime

- Status: Accepted
- Date: 2026-08-06

## Context

The idea this project grew from was "define an agent in YAML, run it with one
command, share it through a registry". That product now exists, shipped by a
vendor with a large team, and it covers declarative definitions, multiple model
providers, MCP tools, multi-agent orchestration and OCI sharing.

Rebuilding it would mean competing on the parts that are already solved, funded
and adopted: provider adapters, streaming, retries, context management, auth,
tool lifecycles. That work is large, permanently ongoing, and undifferentiated.

## Decision

Build the compiler and the intermediate representation. Delegate execution to
runtimes that already exist, through backends.

Explicitly:

| Concern | Position |
|---------|----------|
| Tool protocol | MCP. No new tool protocol. |
| Agent-to-agent messaging | A2A. No new messaging protocol. |
| Distribution | OCI registries. No new registry. |
| Execution | Existing runtimes, through backends. |
| Sandboxing | The runtime's, initially. WASI components later. |
| Skill packaging | The existing skill format, as an input. |

What is ours: a statically typed source language, a target-neutral IR, typed
effects and capabilities, a compile-time portability report, reproducible
artifacts, and a conformance suite backends can be tested against.

## Rationale

The defensible claim is not "run anywhere". It is:

> Compile one source to several runtime targets without rewriting it, and
> report at compile time exactly what each target cannot express.

That claim needs a compiler. It does not need a runtime, and a runtime built
first would make it harder — a project with its own runtime has an incentive to
make that runtime the good target, which is the opposite of neutrality.

Writing a runtime first would also produce, at best, another agent framework:
the thing that makes this different is the type and effect system in front of
it, not the execution loop behind it.

## Conditions for revisiting

A native runtime becomes justifiable only when all of these hold:

1. At least two independent backends work end to end.
2. Agent IR 0.1 is stable and has conformance fixtures.
3. A user-validated need exists that no target runtime can express.
4. The scope is genuinely narrow — a policy-enforcing runner, say, not a general
   execution engine.
5. A team exists that can carry provider adapters and MCP lifecycle maintenance
   indefinitely.

Until then, "we should write our own runtime" is out of scope, and this ADR is
the reference for saying so.

## Consequences

- Real value depends on a backend landing (M3). Until it does, the toolchain
  checks and compiles but nothing runs.
- Backend authors need a good contract, so the IR specification and the
  conformance suite are load-bearing deliverables, not documentation chores.
- The project is positioned as complementary to existing runtimes. Integration,
  not competition, is the working assumption.
- If a target runtime absorbs the compiler layer, the differentiators are
  multi-target lowering, the open IR and the conformance suite — which is why
  the second backend (M5) matters more than the first.
