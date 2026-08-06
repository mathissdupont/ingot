# RFC-NNNN: Feature name

- Status: Draft | Accepted | Rejected | Superseded by RFC-NNNN
- Author(s):
- Created:
- Affects: language | IR | artifact | CLI  (delete what does not apply)

## Problem

The real user problem, with a concrete example of source someone wants to write
and cannot, or a failure they hit. No proposed solution yet.

## Goals and non-goals

What this must achieve, and what it deliberately leaves out. The non-goals are
the more useful half.

## Proposed syntax

Source examples. Show the common case first, then the edge cases.

```ingot
```

## IR semantics

What this lowers to. New node kinds, new value forms, new document fields.
State the canonical encoding of anything new, since the IR must stay byte-stable.

```json
```

## Target lowering

How each existing backend represents this, and what it does when it cannot.
A feature no backend can express is a feature that produces an unbuildable
artifact — say so if that is the case.

## Security and policy impact

New effects or capabilities. Whether the default-deny rule still holds. Whether
anything here can widen what an agent reaches without an explicit grant in its
source. If the answer is "none", say so explicitly rather than omitting the
section.

## Static bounds

Whether this affects step counting, loop bounds or budget checking. Anything
that can execute an unbounded number of times needs an answer here.

## Compatibility

Effect on existing source and existing IR documents. Whether the language
version, the IR version or both need to move. If old programs change meaning,
this is a major change and needs a migration story.

## Alternatives

What else was considered, including doing nothing. Explain why the existing
language or an existing library is not sufficient — this is where most RFCs are
won or lost.

## Conformance tests

The normative tests this needs, by name. A feature is not complete until they
exist.

- [ ] ...
