# Governance

Ingot is maintained by Heptapus Group and developed in the open under
Apache-2.0.

## Roles

**Contributor** — anyone who opens an issue or a pull request.

**Maintainer** — has merge rights in one or more ownership areas, reviews
contributions there, and is accountable for its tests and documentation.

**Ownership areas** — each has a primary and at least one reviewer. Roles follow
the work, not job titles, and shift between milestones.

| Area | Responsibility |
|------|----------------|
| Language & Spec | grammar, type and effect semantics, RFC process |
| Compiler | lexer, parser, resolution, checking, lowering |
| Backends | target code generation and capability profiles |
| Build & Distribution | lockfile, reproducible builds, OCI packaging |
| Runtime & Security | backend lifecycle, policy enforcement, sandboxing |
| Test & Conformance | golden files, replay fixtures, the conformance suite |
| Developer Experience | CLI, language server, editors, diagnostics |
| Docs & Community | tutorials, examples, website, onboarding |

## Decisions

Ordinary changes are decided by review: one maintainer approval in the relevant
area.

Changes to the language, the IR or the artifact format go through the
[RFC process](rfcs/) and need approval from the Language & Spec area plus at
least one other maintainer. Anything touching effects, policy, secrets or the
artifact format also needs Runtime & Security.

Disagreement is resolved by discussion on the RFC. If it cannot be, the Language
& Spec primary decides and records the reasoning in the RFC — recorded and
disliked beats unrecorded.

**Decisions do not live in chat.** A decision that matters becomes an RFC (for
things not yet decided) or an ADR in [`docs/adr/`](docs/adr/) (for things now
decided). If it is not written down, it is not a decision.

## Rhythm

* A design review is mandatory at the start of each milestone, not weekly by
  default. Meetings are scheduled when there is something to decide.
* Every milestone closes with a working demo, a compatibility report and a
  retrospective. "The code is written" does not close a milestone.
* Language and IR changes are discussed with a concrete source example and its
  expected lowered output, never in the abstract.

## Versioning

Three versions move independently:

| Version | Scheme | Meaning |
|---------|--------|---------|
| Language | `MAJOR.MINOR`, declared in every source file | source-level semantics |
| Agent IR | `MAJOR.MINOR` in `irVersion` | the backend contract |
| CLI | SemVer | the `ingot` binary |

Adding an optional IR field is a minor change. Changing the meaning of a field,
or adding a required one, is major. A source file pins its language version so
the language can evolve without changing the meaning of code already written.

The compiler may provide migrations for older IR versions, but unlimited
backward support is not promised. A release note states exactly which language
and IR versions a CLI release implements.

## Becoming a maintainer

Sustained, high-quality contribution in an area, plus a nomination from an
existing maintainer and no objection from the others. Maintainers who have been
inactive for six months move to emeritus and can return on request.
