# RFC-0009: Language 0.2 optional and union values

- Status: Draft
- Author(s): Heptapus Group
- Created: 2026-08-09
- Affects: language, type checker, IR type text, editor tooling

## Problem

Real MCP tool schemas frequently contain nullable fields and small unions:

- a search result may have `snippet: string?`;
- a fetch tool may return `page?` when a URL does not resolve;
- a file reader may return `text | bytes` depending on the detected content;
- metadata from APIs often contains values like `string | int`.

Language 0.1 has no way to spell these shapes except by weakening to `json`.
That hides useful facts from the compiler. Once a declaration says `json`, the
checker cannot protect an agent from treating an absent value as a required
string, or from emitting a `file` where `markdown` is expected.

## Goals and non-goals

Goals:

- Add compact syntax for nullable values and closed unions.
- Keep assignability conservative: a maybe-value is not a required value until a
  later narrowing feature proves it.
- Let imported or local MCP tool declarations preserve common JSON-schema
  shapes without falling back to `json`.
- Keep Agent IR structural compatibility by rendering these types as canonical
  type strings.

Non-goals:

- Pattern matching or full algebraic data types.
- User-defined variants with payload constructors.
- Flow-sensitive null/variant narrowing in this first implementation slice.
- Generic type parameters.
- Changing runtime value encoding; values still travel as JSON-compatible data
  at backend boundaries.

## Proposed syntax

Optional:

```ingot
type page {
  title: string?
}

tool web.fetch(url: string) -> page? !network
```

Union:

```ingot
type tool_output {
  body: markdown | text
  attachment: file | bytes
}
```

Precedence:

```text
postfix [] and ? bind tighter than |
```

Examples:

- `string?[]` means a list of optional strings.
- `string[]?` means an optional list of strings.
- `(file | bytes)[]` means a list whose elements may be files or bytes.

`?` and `|` in type expressions require `language 0.2` or newer.

## Static semantics

`T?` means `T` or absent/null. A value of type `T` may be used where `T?` is
expected. A value of type `T?` may not be used where `T` is expected without a
future narrowing construct.

`A | B` means the value is either `A` or `B`.

Assignability is intentionally conservative:

- `actual` is assignable to `E?` when `actual` is assignable to `E`;
- `A?` is assignable to `B?` when `A` is assignable to `B`;
- `A?` is not assignable to non-optional `A`;
- an actual union is assignable to an expected type only when every alternative
  is assignable to the expected type;
- a value is assignable to an expected union when it is assignable to at least
  one alternative.

Lists remain covariant through the same rules.

## IR semantics

No new Agent IR node kind or JSON field is required. Optional and union shapes
are represented by canonical type strings wherever IR already carries a type:

- record field types: `string?`, `markdown | text`;
- tool parameter/result types: `page?`, `(file | bytes)[]`;
- ask response type strings.

The IR schema remains version `0.1`; the source language field records
`"0.2"`.

## Target lowering

Reference interpreter:

- accepts the new type strings in declarations;
- keeps runtime values as JSON-compatible data;
- does not yet perform flow-sensitive narrowing.

Python backend:

- may treat optional/union type strings as descriptive boundary metadata;
- must not erase them from signatures or portability reports;
- can continue to refuse constructs it cannot faithfully implement, but this
  RFC does not add a new node kind.

## Diagnostics and formatter

Using `?` or `|` in a type expression under `language 0.1` is `ING1020`.

Existing type mismatch diagnostics explain unsafe uses, for example assigning
`markdown?` to `markdown`.

The formatter prints canonical type text with the minimum required parentheses:

```ingot
field: string?
body: markdown | text
attachments: (file | bytes)[]
```

## Compatibility and migration

Existing Language 0.1 source is unchanged.

Migration from `json` is manual but local:

1. replace a nullable JSON field with `T?`;
2. replace small closed schema alternatives with `A | B`;
3. run `ingot check`;
4. fix any newly reported unsafe use before relying on the stronger type.

## Alternatives

**Only `json`.** This keeps the type system smaller but makes tool declarations
less useful exactly where real MCP schemas need precision.

**`optional<T>` instead of `T?`.** This is more explicit but introduces generic
syntax before the language has decided whether generics are needed.

**Tagged unions only.** Useful later, but too heavy for the common schema shapes
where the runtime data is already JSON/null or one of a few scalar/content
types.

**Immediate flow narrowing.** Necessary eventually, but separable. The first
slice is valuable even without it because declarations and type errors become
more precise.

## Conformance tests

- [ ] `optional_field_type_parses_and_formats`
- [ ] `union_field_type_parses_and_formats`
- [ ] `postfix_type_precedence_is_canonical`
- [ ] `language_0_1_rejects_optional_type`
- [ ] `language_0_1_rejects_union_type`
- [ ] `tool_result_can_be_optional_record`
- [ ] `tool_result_can_be_small_union`
- [ ] `required_value_assigns_to_optional_slot`
- [ ] `optional_value_does_not_assign_to_required_slot`
- [ ] `union_assigns_only_when_all_alternatives_are_safe`
- [ ] `optional_and_union_type_text_lowers_to_ir`
- [ ] `python_portability_report_preserves_optional_union_type_text`
