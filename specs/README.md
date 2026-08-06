# Specifications

Normative behaviour lives here. Implementation detail does not.

| Document | Status | Covers |
|----------|--------|--------|
| [`language/v0.1.md`](language/v0.1.md) | Draft, implemented | Syntax and static semantics |
| [`ir/v0.1.md`](ir/v0.1.md) | Draft, implemented | The Agent IR, the backend contract |
| [`ir/agent-ir.schema.json`](ir/agent-ir.schema.json) | Draft, implemented | Machine-readable IR schema |
| [`image/`](image/) | Not started (M6) | OCI artifact profile, media types, lockfile |
| [`runtime/`](runtime/) | Not started (M3) | Backend interface and capability profiles |
| [`conformance/`](conformance/) | Not started (M8) | Normative tests a backend must pass |

Where a specification and the implementation disagree, the specification is
authoritative and the implementation has a bug.

A rule without a test is not considered specified. Tests that pin down the
current rules live in the crate that implements them, plus `tests/golden-ir/`
for the compiled output of the reference examples.
