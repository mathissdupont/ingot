# RFC-0004: Ingot Containers — the policy block as a boundary

- Status: **Accepted**
- Created: 2026-08-07
- Affects: language spec (path semantics), runtime spec, CLI, manifest
- Implements: [ADR-0006](../docs/adr/0006-a-policy-enforcing-runner.md), stage 1
- Closes: part of [GAP-001](../docs/gaps.md#gap-001)

## Problem

An agent's `policy` block reads like a boundary and behaves like a checklist.

```ingot
policy {
  filesystem_read  allow ["src", "crates"]
  filesystem_write allow ["target/review"]
  network deny
}
```

At run time the interpreter asks, before each call: *does this node need an
effect the policy grants?* It answers yes or no. Then it hands the call to a
tool server — a separate process, running as the operator, with the operator's
entire filesystem and the operator's network. Whether that server reads `src` or
`~/.ssh` is not a question anything asks.

So the three lines above establish that the agent *may* read and *may not* reach
the network, and establish nothing at all about `src`, `crates`,
`target/review`, or the absence of a network. That is
[GAP-001](../docs/gaps.md#gap-001), and it is the gap most likely to be mistaken
for a guarantee.

## Goals and non-goals

**Goals**

1. A tool server runs inside a boundary derived from the artifact's own policy.
2. What the boundary will be is **inspectable before anything runs**.
3. What the boundary **cannot** enforce is named, and refused by default rather
   than glossed over.
4. Policy paths get a defined frame of reference, so that an artifact means the
   same thing on two machines.
5. Nothing changes for anyone who does not opt in. `ingot run` without a sandbox
   behaves exactly as it does today.

**Non-goals for this RFC**

* **Containing the run itself.** The interpreter stays on the host. That is
  stage 2, and [ADR-0006](../docs/adr/0006-a-policy-enforcing-runner.md) blocks
  it on a second backend existing.
* **Enforcing a host allowlist.** `network allow ["arxiv.org"]` needs an egress
  proxy. This RFC reports it as unenforceable rather than pretending.
* **Replacing the tool server's own bound.** `ingot-mcp-fs --root` still
  applies. Two independent limits is the design, not an accident.

## What a policy path is relative to

The language never said, and until now nothing needed it to. Both shipped
examples turn out to write policy paths relative to the **tool server's root** —
`code-review-team` says `["src", "crates"]` with a server rooted at the
repository, `repo-digest` says `["."]` with a server rooted at a subdirectory.
Neither is interpretable from the artifact alone, because the artifact cannot
see the manifest.

That has to be settled before a path can be enforced, and the answer cannot be
"the project directory": an artifact pulled from a registry has no project.

**Decision: policy paths are relative to the *workspace*, an abstract root the
operator binds at run time.**

```
ingot run --workspace /srv/checkouts/api        # explicit
ingot run                                       # defaults to the project directory
```

The artifact says `src`; the operator says where `src` lives. That is portable,
it is the same shape as every other operator-side binding in the design, and
inside a boundary the workspace has one fixed location, so a tool sees the same
paths wherever it runs.

Consequence: both examples' policies were wrong and are corrected. That they
were wrong for a year of design and nobody noticed is the argument for
enforcement.

## The plan

Enforcement splits in two, and the split is where the testing leverage is.

**A plan** is a pure function of the artifact's policy, the workspace and the
manifest: which paths are mounted and how, what the network is, which
environment variables cross, and what could not be honoured. It runs anywhere,
needs no container runtime, and is worth having on its own — it answers "what
box would this agent get?" before there is a box.

**An executor** turns a plan into a running boundary. It needs a container
runtime, so it is where portability and testability get hard.

```
$ ingot sandbox examples/code-review-team

server `repo`  (for agent CodeReviewTeam)
  mount  ./src            -> /workspace/src            ro    filesystem_read
  mount  ./crates         -> /workspace/crates         ro    filesystem_read
  mount  ./target/review  -> /workspace/target/review  rw    filesystem_write
  network  none                                              network deny
  env      (none)
  workdir  /workspace

  cannot enforce:
    external_write require approval
      a boundary cannot distinguish an intended external write from any other;
      the effect check and the approval gate still apply
```

### Derivation

| Policy | Boundary |
|--------|----------|
| `filesystem_read allow [p…]` | each `p` mounted read-only at `/workspace/p` |
| `filesystem_write allow [p…]` | each `p` mounted read-write; read-write wins if a path appears in both |
| `filesystem_* deny`, or absent | not mounted; the path does not exist inside |
| `network deny`, or absent | no network |
| `network allow [h…]` | network on, allowlist **not enforced** — recorded as unenforceable |
| `network allow []` | network on, unrestricted — recorded as unenforceable |
| `secrets deny export` | satisfied structurally: no credential is placed inside |
| `external_write allow`/`require approval` | recorded as unenforceable |

Two refusals happen at plan time, before anything starts:

* a read mount whose path does not exist — mounting an empty directory would
  make a missing checkout look like an empty repository;
* a policy path that is absolute or climbs out of the workspace.

A write mount whose path does not exist is created, because
`filesystem_write allow ["target/review"]` is how you say "put the report here".

### Whose policy

One agent's, not the union of several. A program's agents deliberately differ:
in `code-review-team` the sub-agent may read and the coordinator may write, and
a shared box wide enough for both would hand the sub-agent a write mount its own
policy denies.

So a plan is per **(server, agent)** pair, and the executor starts one server
instance per agent that needs it. `ToolInvocation` gains the calling agent's
name, because a host cannot apply the right policy without knowing whose call it
is.

### Refusing rather than glossing

`ingot run --sandbox` **refuses to start** when a plan has unenforceable
entries, and names them. `--sandbox-allow-unenforced` proceeds anyway, and the
run's first event says which limits are advisory.

This is [Runtime 0.1 §2](../specs/runtime/v0.1.md) applied to ourselves: a
restriction that is silently dropped is worse than a build that fails. An
operator who thinks `network allow ["arxiv.org"]` is enforced, because a
sandbox was switched on, is worse off than one who knows it is not.

### Approvals still work

Worth stating because it is not obvious: in stage 1 the interpreter is on the
host, so an `approval` node still reaches a human. It is stage 2, where the run
itself moves inside, that has to answer this.

## What this does not fix

The boundary constrains a **tool server**. It does not constrain the
interpreter, which is ours and which follows a checked artifact, and it does not
constrain the model. If a tool server is a proxy for something else — a shell,
an interpreter, a remote API — then the boundary is the reach of that thing, and
`--network none` is doing more work than the mount list.

[GAP-001](../docs/gaps.md#gap-001) therefore becomes *partly* closed: the
filesystem half is enforced, the network half is enforced only as
present-or-absent. The register entry says so rather than being ticked off.

## Compatibility

Additive. No language syntax changes, no IR change, no new node kind. An
artifact built before this RFC produces the same plan as one built after.

Two behaviour changes, both opt-in:

* `--workspace` and `--sandbox` are new flags; without them nothing differs.
* The shipped examples' policy paths are corrected to the workspace frame. Their
  IR changes, so their golden files change.
