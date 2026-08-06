# ADR-0003: Capabilities are default-deny

- Status: Accepted
- Date: 2026-08-06

## Context

An agent's `policy` block states what it may do. The question is what happens
when the block says nothing about an effect a tool needs.

Two options:

1. **Default-allow.** An unmentioned effect is permitted. The policy block is a
   list of restrictions.
2. **Default-deny.** An unmentioned effect is denied. The policy block is a list
   of grants.

## Decision

Default-deny. An effect is available only if a policy rule grants it. A missing
rule is reported as its own diagnostic (`ING4007`), distinct from an explicit
`deny` (`ING4001`).

`model_access` is the single exception: it is implicit on every `ask` and always
granted.

## Rationale

**The failure modes are not symmetric.** Under default-allow, forgetting a rule
produces an agent with more reach than intended, and nothing says so. Under
default-deny, forgetting a rule produces a build error with a message naming the
missing rule. One failure is silent and in production; the other is loud and at
compile time.

**The artifact should be readable.** A reviewer looking at a compiled agent
should be able to read its reach off the policy block. That only works if the
block is exhaustive, which only holds under default-deny.

**Two diagnostics, not one.** An explicit `deny` and an absent rule are
different mistakes. The first means "you wrote that this is forbidden"; the
second means "you have not said". They get different codes and different help
text, because the fix differs: change the rule, versus add one.

**`model_access` has to be exempt.** Requiring `network allow` for the model
provider would force every allowlist to name it, which would make allowlists
meaningless. An agent that may not call a model is not an agent.

## Consequences

- Simple agents need more ceremony. The document summariser example spells out
  five denials it does not need. That is the cost, and the examples show it
  honestly rather than hiding it.
- A future RFC may add policy presets or organisation-level composition to
  reduce the boilerplate. Any such mechanism must preserve the invariant: the
  effective policy is derivable from the source without running anything.
- Backends inherit the rule. A subject absent from the IR's `policy` object is
  denied, and a backend that cannot enforce a decision must reject the artifact
  rather than proceed.
- Adding a new effect is a breaking change for programs that call a tool which
  gains it: they will fail to build until the policy is updated. This is the
  intended behaviour, and it is why the effect set is small and closed within a
  language version.
