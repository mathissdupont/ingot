# Research

Source material this project was derived from.

## `2026-08-06-agent-platform-research-tr.docx`

The original technical research and roadmap, in Turkish. Dated 6 August 2026.
It surveys the prior art, records the GO / NO-GO decisions, and proposes the
architecture, repository layout and milestone plan that this repository
implements.

It is kept for provenance. It is **not** normative: where it and
[`specs/`](../../specs/) disagree, the specifications win.

Places where the implementation deliberately departs from the document:

| Document | Implementation | Reason |
|----------|----------------|--------|
| Working name "Project AX", CLI `agx` | `Ingot`, CLI `ingot`, sources `.ing` | The document flags its own name as provisional and requires clearance; `AGX` collides with an established hardware brand in the same sector. |
| `verify CitationCheck(...)` used undeclared | `verifier` declarations | A `verify` call has to be type-checkable, which needs a signature. |
| Policy unstated when a rule is absent | Default-deny, with a distinct diagnostic | See [ADR-0003](../adr/0003-default-deny-capabilities.md). |
| `state.read` unspecified | An explicit node, one per field per statement | Makes state access auditable in the artifact. |

Decisions taken since are recorded in [`docs/adr/`](../adr/), and proposals in
[`rfcs/`](../../rfcs/).
