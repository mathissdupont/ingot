# What Ingot is for

The README says what Ingot *is*. This says what it is **for**, end to end, so
that a decision about scope has something to be measured against.

## The problem

There are two ways to write an AI agent today and both break.

**In YAML.** It starts well. Then the agent needs a condition, then a loop, then
a type, then an answer to "which files may this touch". YAML carries none of
that, so it grows a programming language inside itself — badly, in a format with
no compiler.

**In Python, against a framework.** Powerful, and it marries the agent to one
vendor's execution loop. An agent written against one framework does not move to
another. Worse, nothing tells you *before it runs* that it needs a permission
nobody granted. You find out in production, and the way you find out is that it
did something.

Neither gives one coherent path from an idea to something a person can keep:
source they can read, errors they can act on, a deterministic test, a safe place
to run, and an artifact that is not married to the framework that created it.

Permissions are a necessary part of that path, not the whole product. An agent
that is safe but difficult to create will not be used; an agent that is easy to
generate but impossible to inspect, test or constrain will not be trusted.

## The claim

A small, statically typed language sits between those two, and it can answer
questions before the agent runs.

```
$ ingot check
error[ING4007]: `web.search` requires the `network` effect, which no policy rule grants
  --> main.ing:31:12
   |
31 |     hits = call web.search(topic)
   |            ^^^^^^^^^^^^^^^^^^^^^^ needs `network`
   |
   = note: Ingot is default-deny: an effect with no rule is denied
```

An agent that compiles is an agent whose permissions have been stated and
checked, whose types line up, whose budget is satisfiable, and whose loops
terminate. Not an agent that is *correct* — no compiler promises that — but one
whose reach is known.

The user-facing claim is broader: **write an agent in a small, readable language;
Ingot helps you understand it, test it, run it inside the boundary its own policy
describes, and move the same artifact elsewhere.** Security is the trust layer
under that workflow. It is not a separate destination.

## The four parts

Ingot is a toolchain, and each part exists because the one before it is not
enough on its own.

**1. A language.** One file holds the whole agent, and "the whole agent" is four
things that are usually scattered across a codebase, a config file, a deployment
manifest and somebody's memory:

| | |
|---|---|
| **which model** | `model exact "openai/gpt-5.1"` — vendor and model, in the source |
| **which tools** | `tools { mcp repo.read_file }` — typed signatures, checked at each call |
| **what it may touch** | `policy { filesystem_read allow ["src"] }` |
| **what it may spend** | `budget { steps <= 40  tokens <= 200000 }` |

Small on purpose does not mean frozen. Language 0.2 adds project-local imports,
optional and union type expressions, and expression-only pure helper functions
because those remove repeated authoring friction while staying checkable.
Generics remain deliberately deferred until repeated real source shows the need.
The value is not maximum expressiveness; it is that everything expressible is
checkable.

**2. A compiler.** Types, effects, capabilities, budgets and static bounds, all
before execution. Default-deny throughout: an effect with no policy rule is
denied, with its own diagnostic, never a permission.

**3. A portable artifact.** The Agent IR: the whole agent as canonical JSON,
byte-reproducible, naming no vendor and no technology. This is what moves
between machines, gets signed, and gets shipped.

**4. Somewhere to run it.** Backends that take the artifact and execute it in a
real environment.

## What we do not build

| Concern | Position |
|---------|----------|
| Tool protocol | MCP. No new tool protocol. |
| Agent-to-agent messaging | A2A. No new messaging protocol. |
| Distribution | OCI registries. No new registry. |
| General execution | Existing runtimes, through backends. |

The differentiators are compile-time portability, typed effects and
capabilities, a machine-readable target compatibility report, reproducible
artifacts, and a conformance suite backends can be tested against.

## Where it is going

Four directions beyond the toolchain, each following from the same idea: a
declaration is only worth writing if something enforces it.

### One product loop around the language

The compiler, cassette system, runtime, containers and backends exist today, but
the user has to operate them as separate systems. The next product milestone is
to join them without hiding the language:

```text
create or write .ing
        ↓
develop with immediate diagnostics and traces
        ↓
replay a deterministic test
        ↓
run inside a policy-derived boundary
        ↓
package the same checked artifact
```

`.ing` remains the source of truth at every step. Templates produce it, the
editor understands it, a model may propose a diff to it, and build/test/run all
consume what it compiles to. There is no hidden workflow representation behind
the language.

[RFC-0007](../rfcs/0007-the-ingot-product-loop.md) defines this product loop,
the delivery order and the work packages that will become implementation issues.

### Ingot Containers: the safe-run implementation

Today an agent's `policy` block is a **checklist**. The runtime asks "does this
call need an effect the policy grants?", answers yes, and calls the tool. The
tool then runs as the operator's user, with the operator's filesystem and the
operator's network. Where it actually goes, nobody checks
([GAP-001](gaps.md#gap-001)).

An Ingot Container is the same declaration used as a **boundary** rather than a
checklist. The policy configures the environment:

| Policy | Boundary |
|--------|----------|
| `filesystem_read allow ["src"]` | only `src/` is mounted, read-only; nothing else exists |
| `filesystem_write allow ["out"]` | one writable mount; anything else fails at the kernel |
| `network deny` | no network |
| `network allow ["arxiv.org"]` | egress to that host, enforced at the network layer |
| `secrets deny export` | the model credential never enters the environment |

The difference is the difference between an agent that *knows* it should not,
and an agent that *cannot*. It is also the first place `network allow [host]`
can stop being a wish.

Scope is deliberately narrow: **a policy-enforcing runner, not a general
execution engine.** That is the shape
[ADR-0002](adr/0002-compiler-not-runtime.md) named as the only acceptable one,
and [ADR-0006](adr/0006-a-policy-enforcing-runner.md) is where it was accepted.

Where this stands, as of 2026-08-08: four of the five rows above are real.
`ingot run --sandbox` contains an agent's tool servers
([RFC-0004](../rfcs/0004-ingot-containers.md)) and `ingot run --contained`
contains the agent itself ([RFC-0005](../rfcs/0005-the-contained-run.md)), which
is what makes `network deny` and `secrets deny export` true of the agent rather
than only of its tools. The remaining row is the host allowlist, which needs an
egress proxy: [GAP-001](gaps.md#gap-001). One limitation is worth knowing —
sub-agents cannot yet cross into their own boundary, so a program whose agents
need different ones is refused: [GAP-023](gaps.md#gap-023).

This system is not discarded as authoring becomes easier. It moves behind the
safe-run step. A user should benefit from the policy-derived mounts, network and
credential topology without reconstructing Docker build commands or
understanding the supervisor protocol. An unavailable boundary still refuses;
ease of use never means silently running with weaker isolation.

### Authoring with a model, without replacing the language

Writing a first agent should not require learning the whole language first.
`ingot new "review pull requests for security problems"` should produce a
working, ordinary `.ing` file. It must not produce a hidden graph that happens
to export Ingot.

What makes this more than a code-generation gimmick is that Ingot has a
**verifier**: the generated source either passes `ingot check` or comes back
with a precise diagnostic the model can act on. A model writing YAML produces
something that fails in production; a model writing Ingot produces something the
compiler accepts or rejects before anything runs.

One rule constrains the whole feature: **the generator may never widen a policy
to make code compile.** If the agent it wrote needs `network`, it says so and
asks. A generator that silences its own safety checks is worse than no
generator.

The language remains fully usable without a model. Templates, `ingot dev`, the
editor, compiler, tests and contained runs cannot depend on an authoring API key.
Model assistance accelerates the same workflow and leaves a project that still
works when it is gone.

### More than one place to run

Portability is the central claim. It is now observed by running the same artifact
and cassette through the reference interpreter and an independent generated
Python program. [GAP-018](gaps.md#gap-018) records what that falsification test
closed; the next step is packaging it as the conformance suite in GAP-017.

## How to tell whether it worked

- An agent's `policy` block is a boundary, not a comment.
- The same artifact runs in more than one place, and what a target cannot do is
  reported before deployment rather than discovered during it.
- A clean machine can create, check, replay and safely run a useful template
  without reconstructing repository-specific setup.
- Direct `.ing` authors receive immediate compiler diagnostics and a useful run
  trace instead of operating each toolchain phase by hand.
- Someone who has never written Ingot ships a working agent.
- Every limitation is in [the gap register](gaps.md) rather than in someone's
  head.
