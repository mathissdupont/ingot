# Specifications

Normative behaviour lives here. Implementation detail does not.

| Document | Status | Covers |
|----------|--------|--------|
| [`language/v0.1.md`](language/v0.1.md) | Draft, implemented | Syntax and static semantics |
| [`language/v0.2.md`](language/v0.2.md) | Draft, partially implemented | Language 0.1 plus project-local imports |
| [`ir/v0.1.md`](ir/v0.1.md) | Draft, implemented | The original Agent IR backend contract |
| [`ir/v0.2.md`](ir/v0.2.md) | Draft, implemented | Agent IR 0.1 plus portable node source spans |
| [`ir/agent-ir.schema.json`](ir/agent-ir.schema.json) | Draft, implemented | Machine-readable IR schema |
| [`image/`](image/) | Not started (M6) | OCI artifact profile, media types, lockfile |
| [`runtime/v0.1.md`](runtime/v0.1.md) | Draft, implemented | Execution model and the backend interface |
| [`conformance/`](conformance/) | Started (M5); packaging planned for M8 | Normative tests a backend must pass |

Where a specification and the implementation disagree, the specification is
authoritative and the implementation has a bug.

A rule without a test is not considered specified. Tests that pin down the
current rules live in the crate that implements them, plus `tests/golden-ir/`
for the compiled output of the reference examples. Cross-backend differential
tests live in `crates/ingot-cli/tests/differential.rs`.
