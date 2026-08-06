# ADR-0004: One canonical IR encoding

- Status: Accepted
- Date: 2026-08-06

## Context

The IR will eventually be packaged as a layer in an OCI artifact, and the
artifact will be addressed and signed by digest. A digest is only meaningful if
the bytes are reproducible: the same source and the same compiler must produce
the same file, on any machine, on any run.

JSON makes that easy to get wrong. Map iteration order, float formatting,
indentation and trailing newlines all vary by default.

## Decision

Exactly one encoding is valid, and the compiler emits only that one:

- two-space indentation, pretty-printed
- object keys sorted (every map is a `BTreeMap`; struct field order is fixed by
  declaration)
- exactly one trailing newline
- empty optional collections omitted rather than written as `[]` or `{}`
- monetary amounts as decimal **strings**, never floats
- effects, capabilities and policy values sorted
- node ids assigned in creation order, which follows source order

This is what `AgentIr::to_canonical_json` produces, and the golden tests compare
against it byte for byte.

## Rationale

**Floats are the trap.** `5.0` serialises as `5.0`, `5`, or `5.000000000000001`
depending on the library and the platform. A cost budget is a decimal quantity,
not a binary one, so it is stored as a decimal string. This costs backends one
parse and buys byte-stability.

**Sorted keys beat insertion order.** Insertion order is stable only if every
producer inserts in the same order, which no one can enforce across future
backends and tools. Sorting is enforced by the type system: using a `BTreeMap`
makes unsorted output impossible rather than merely discouraged.

**Pretty-printed, not minified.** The IR is reviewed by humans during
development and diffed in golden tests. A minified file would make every change
a single unreadable line. Size matters far less than reviewability at this
stage, and compression handles it at the packaging layer.

**Golden files, not just a property test.** A determinism property test proves
the compiler agrees with itself. Checked-in golden files prove the compiler
agrees with what was reviewed, and make a lowering change show up as a diff a
person reads before accepting.

## Consequences

- Any change to lowering shows up as a golden diff. `INGOT_UPDATE_GOLDEN=1`
  regenerates them; the pull request must explain the change. An unexplained
  golden diff means the compiled meaning of every existing agent moved.
- Adding a field to the IR model changes every golden file. This is a feature:
  it forces the author to look at the effect on real programs.
- Backends must not assume key order carries meaning. Arguments are named and
  ordered by the callee's declaration, so nothing depends on JSON ordering.
- A digest and signing story (M6) can be built on top without revisiting the
  encoding.
