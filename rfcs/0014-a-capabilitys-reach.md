# RFC-0014: A capability's reach

- Status: **Accepted**
- Created: 2026-08-10
- Affects: language, IR, CLI
- Closes: [GAP-013](../docs/gaps.md#gap-013)
- Narrows: [GAP-001](../docs/gaps.md#gap-001)
- Specified in: [Language 0.2 §9](../specs/language/v0.2.md)

## Problem

The effect system says what *kind* of thing a call does. It cannot say where.

```ingot
tool web.search(query: string) -> search_result[] !network

policy {
  network allow ["arxiv.org", "github.com"]
}
```

Two statements, and nothing connects them. `!network` says this tool goes
somewhere on the network. `network allow [...]` says this agent may reach two
hosts. The effect check at the call site reads the first and consults the
decision on the second — allow, deny, require approval — and never looks at the
values at all.

So the values are a claim with no reader. A tool declared `!network` may contact
anything; adding a host to the list changes nothing a compiler can see; and
removing one changes nothing either. [Language 0.1 §7.1](../specs/language/v0.1.md)
says as much — "values constrain reach, and reach is enforced by whatever the
artifact runs inside" — and [GAP-001](../docs/gaps.md#gap-001) records that for
the network, nothing does.

There are two separate failures here and they are usually confused. One is that
no runner bounds egress to a host list; that needs an egress proxy and is
GAP-001. The other is that the *language* cannot express a bound per call in the
first place, so even a runner that could enforce one would not know what to
enforce. This RFC is the second.

It is worth doing first, and not only because it is the cheaper half. A policy
that lists hosts today reads like a bound and is a wish. Making the language
able to state a bound, and making the compiler check it, converts the most
common lie in an Ingot artifact into a compile error.

## Goals and non-goals

**Goals.** State what a tool reaches. Check at compile time that it is inside
what the agent granted. Carry it into the IR so a runner knows what to enforce.
Refuse to run when a stated reach cannot be enforced.

**Non-goals.** The egress proxy (GAP-001). Wildcards, ports, URL paths, or CIDR
ranges — 0.1 matches a host exactly and this RFC does not widen that. A scope
written inside the flow, per call rather than per tool; see *Alternatives*.

## Proposed syntax

An effect on a tool declaration may carry the values it reaches:

```ingot
tool web.search(query: string) -> search_result[] !network("arxiv.org")
tool repo.read(path: string)   -> string          !filesystem_read("src", "docs")
tool page.fetch(url: string)   -> string          !network
```

The parenthesised list is the effect's **reach**. It uses the same values
[Language 0.1 §7.1](../specs/language/v0.1.md) already defines for a policy of
that subject — a host name for `network`, a workspace-relative path for
`filesystem_read` and `filesystem_write` — with the same rules. A host is
matched exactly and carries no wildcard. A path may not be absolute and may not
contain `..`.

An effect with no parentheses, as in `page.fetch` above, declares no reach. That
is the existing syntax and it keeps its existing meaning: this tool performs
this kind of effect, and how far it goes is not stated.

`!network()` — an empty list — is refused. It reads like "reaches nothing",
which would be a tool that does not need the effect, and a tool that does not
need an effect should not declare it.

Effects that name no resource take no reach. `secret_access`, `external_write`
and `model_access` are refused with parentheses, because there is no value
vocabulary for them and inventing one here would be a second, undocumented
policy language.

## IR semantics

`ToolBinding` gains one field:

```json
{
  "ref": "mcp:web.search",
  "name": "web.search",
  "transport": "mcp",
  "effects": ["network"],
  "scopes": { "network": ["arxiv.org"] },
  "signature": { }
}
```

`scopes` maps an effect name to its sorted, deduplicated values. Absent when the
tool declares no reach, so an artifact that uses none is byte-identical to one
compiled before this change — which matters, because a digest over the IR is how
a package is identified ([RFC-0012](0012-the-ingot-package.md)).

Every key of `scopes` is also present in `effects`. The two are not merged into
one field: `effects` is a set a backend already iterates to check the policy, and
an entry that is sometimes a string and sometimes an object is the kind of shape
that makes a second implementation guess.

Sorted and deduplicated at compile time rather than at use, so the canonical
encoding is a property of the document rather than of whoever wrote it.

## Static bounds

The compiler checks **containment**: what a tool reaches must be inside what the
agent granted.

For each tool an agent grants, for each scoped effect of that tool:

| Policy for that effect | Rule |
|---|---|
| `allow` with values | every declared value must appear in the policy's list, or `ING4009` |
| `allow` with no values | the grant is unbounded; any reach is contained |
| `require approval` with values | same as `allow` with values; the gate is about *whether*, not *where* |
| `deny`, or no rule | already `ING4001` or `ING4007` on the effect kind, before reach is considered |

A tool that declares no reach is not checked, and is not an error. The policy's
values still bound it at run time wherever a runner can bound anything; what is
absent is only the earlier, cheaper check.

```
error[ING4009]: `web.search` reaches `github.com`, which this agent's policy
                does not grant
  --> main.ing:12:5
   |
 8 | tool web.search(query: string) -> search_result[] !network("arxiv.org", "github.com")
   |                                                                         ------------ declared here
...
12 |     network allow ["arxiv.org"]
   |                   ^^^^^^^^^^^^ granted here
   |
   = add "github.com" to this policy, or use a tool that does not need it
```

Naming both spans is the point. The two halves of the mistake are in different
declarations and either one may be the wrong one.

**Sub-agents.** A sub-agent's tools are checked against the sub-agent's own
policy, and the parent's grant must contain the union of what its sub-agents
reach — the same rule [Language 0.1 §7](../specs/language/v0.1.md) already
applies to effect kinds, extended to values. A parent cannot widen a child's
reach by granting more, and cannot narrow it by granting less: the child is
checked against its own policy first, and the parent is then checked against
what it delegates.

## Security and policy impact

**A declared reach is a stronger statement than a policy value**, and this is
the load-bearing distinction in this RFC.

A policy's value list has always been advisory in arrangements that cannot bound
anything, and Ingot has been explicit about that:
`--sandbox-allow-unenforced` exists, `ingot sandbox` reports what a boundary
cannot deliver, and passing that flag without a boundary is itself an error
because "without a boundary there is nothing to leave unenforced".

`!network("arxiv.org")` is not advisory. It says this tool must be bounded to
that host. So:

> A run **must** refuse to start when an artifact declares a reach the
> arrangement cannot enforce, unless the operator passes
> `--allow-unenforced-scopes`.

This is opt-in strictness rather than a change to existing programs. An artifact
written before this RFC declares no reach, so nothing about it changes. An
artifact that adopts the syntax gets a guarantee that it will not be run by
something that cannot keep it.

The refusal reuses the machinery that already exists: a declared reach a
boundary cannot deliver becomes an `Unenforceable` note on the sandbox plan,
alongside the host-allowlist note that is already produced there, and the
existing refusal reports both together. A plain `ingot run` bounds nothing and
therefore refuses any declared reach outright.

**What this does not make true.** No runner bounds egress to a host yet. A
declared `!network("arxiv.org")` is checked against the policy, recorded in the
IR, and refused where it cannot be honoured — and it is still not *enforced*
anywhere. GAP-001 stays open until the proxy exists. What changes is that the
gap can no longer be reached by accident: before this RFC an unenforced host
list ran silently; after it, stating a reach and running anyway takes a flag
with "unenforced" in its name.

## Compatibility

Additive in every direction.

Source written before this RFC parses and compiles unchanged, because an effect
without parentheses keeps its meaning. An artifact with no scoped effect
produces byte-identical IR, so no package digest moves and no cassette is
invalidated. A backend that ignores `scopes` behaves exactly as it did — which
is not permission to ignore it, only an observation that nothing breaks: per
[Runtime 0.1 §2](../specs/runtime/v0.1.md) a backend that cannot honour a scope
must refuse the run, and a conformance test says so.

`language 0.1` sources may use the syntax. Ingot does not gate syntax on the
declared language version today — project-local imports do not — and introducing
a gate for this one feature would be a rule nobody could predict from the others.

## Alternatives

**A scope in the policy, per tool** — `network allow ["arxiv.org"] for web.search`.
Rejected: it keeps one security surface, which is genuinely attractive, but it
puts a claim about a foreign server's behaviour in the block that grants
permission. The two are different facts with different authors, and merging them
means the compiler has nothing to compare. Containment needs two statements.

**A scope at the call site** — `web.search(query: q) with network to "arxiv.org"`.
Rejected, though it is the most precise of the three, and the phrasing GAP-013
itself used. Two objections. It scatters security decisions through the flow, so
"what can this agent reach" stops being answerable by reading one block. And a
backend that cannot honour a scope discovers it mid-run, which makes the refusal
in *Security and policy impact* arrive after work has already happened — the
opposite of refusing before starting anything.

**Inferring reach from the tool server.** Rejected: an MCP server is not obliged
to describe its egress, and a value Ingot inferred would be a guarantee derived
from an unverified claim by the very thing being constrained.

## Conformance tests

- [ ] `a_tool_that_reaches_beyond_the_policy_is_a_compile_error`
- [ ] `a_tool_within_the_policy_compiles`
- [ ] `an_unbounded_grant_contains_any_reach`
- [ ] `a_tool_with_no_declared_reach_is_not_an_error`
- [ ] `an_empty_reach_is_refused`
- [ ] `an_effect_with_no_value_vocabulary_takes_no_reach`
- [ ] `a_reach_path_may_not_leave_the_workspace`
- [ ] `a_sub_agents_reach_is_contained_by_its_own_policy`
- [ ] `scopes_reach_the_ir_sorted_and_deduplicated`
- [ ] `an_artifact_with_no_scope_produces_identical_ir`
- [ ] `a_run_that_cannot_enforce_a_declared_reach_refuses`
- [ ] `the_refusal_can_be_acknowledged_and_names_what_is_unenforced`
