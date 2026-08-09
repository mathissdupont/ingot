# RFC-0011: Defer Language 0.2 generics until repeated source demands them

- Status: Draft
- Author(s): Heptapus Group
- Created: 2026-08-09
- Affects: language roadmap, type checker, editor tooling

## Decision

Language 0.2 does not add generic type parameters or generic helper functions.
Generics are deferred until repeated real Ingot source demonstrates the same
reusable type or helper pattern that cannot be expressed cleanly with imports,
optional types, union types and pure helper functions.

The threshold for reopening generics is at least three real `.ing` source
examples showing the same pressure. A single example is documented as a
workaround. Two examples are tracked. Three examples justify a new RFC.

## Terms

A generic is a declaration with a type parameter: a placeholder type supplied
when the declaration is used. For example, a future language might be able to
spell a reusable result container as:

```ingot
type result<T> {
  value: T?
  error: string?
}
```

Then `result<string>` and `result<file>` would share one declaration while
carrying different payload types.

The same idea could apply to helpers:

```ingot
fn first<T>(items: T[]) -> T? = ...
```

That helper would return `string?` for `string[]`, `file?` for `file[]`, and so
on.

## Why defer

Generics are powerful, but they are also a type-system expansion rather than a
small authoring convenience. They would require decisions about:

- syntax for type parameters and type arguments;
- type inference at call sites;
- diagnostic messages when inference fails;
- formatter and editor behaviour;
- canonical Agent IR type text;
- backend portability reporting;
- interaction with optional and union types.

The previous Language 0.2 slices remove the observed reuse pain without that
cost:

- imports share `type`, `tool` and `verifier` declarations across files;
- optional and union types describe common MCP nullable and small-union schemas;
- pure helpers extract repeated value transformations while erasing before IR.

Adding generics without repeated source evidence would move Ingot toward a
general-purpose language before agent authors have shown they need that extra
surface.

## Evidence that would reopen the question

Open a new generics RFC if at least three real Ingot sources show the same
pattern, such as:

- repeated `result_*` or `page_*` record types that differ only by payload type;
- repeated helper functions that differ only by element or payload type;
- MCP schemas that repeatedly expose the same typed container shape;
- user-authored source where imports, optionals, unions and pure helpers still
  leave visible copy/paste solely because the payload type changes.

The examples should be copied or linked from the RFC so the feature is designed
against source people actually wrote.

## Current workaround

Until then, authors should use explicit named records and helpers:

```ingot
type string_result {
  value: string?
  error: string?
}

type file_result {
  value: file?
  error: string?
}
```

This is intentionally more verbose than generics, but it keeps the source
obvious and gives future design work concrete examples if the pattern repeats.

## IR, runtime and backend impact

None. This RFC adds no syntax and no Agent IR representation. Existing Language
0.1 and 0.2 source keeps its meaning.

If generics are later specified, that RFC must define canonical source syntax,
static semantics, formatter/editor behaviour, Agent IR type text or erasure
rules, reference lowering, Python portability reporting and migration behaviour.

## Alternatives

**Specify generic records now.** This would solve hypothetical `result<T>`-style
repetition, but no repeated source currently proves that the added complexity is
worth carrying.

**Specify generic helpers now.** Pure helpers are intentionally expression-only
and inline before IR. Adding type parameters now would force inference and error
design before helper usage has accumulated.

**Never allow generics.** Too rigid. The correct stance is evidence-gated: keep
the language small until real agents show a repeated pattern the current tools
cannot express cleanly.

## Conformance tests

No compiler conformance tests are required because this RFC adds no syntax.
The acceptance test is documentary: Language 0.2 and RFC-0007 must point to
this decision so generics are visibly deferred rather than forgotten.
