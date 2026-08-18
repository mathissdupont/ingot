# Specifications

Normative behaviour lives here. Implementation detail does not.

| Document | Status | Covers |
|----------|--------|--------|
| [`language/v0.1.md`](language/v0.1.md) | Draft, implemented | Syntax and static semantics |
| [`language/v0.2.md`](language/v0.2.md) | Draft, partially implemented | Language 0.1 plus project-local imports and verifier bodies |
| [`ir/v0.1.md`](ir/v0.1.md) | Draft, implemented | The original Agent IR backend contract |
| [`ir/v0.2.md`](ir/v0.2.md) | Draft, implemented | Agent IR 0.1 plus portable node source spans, a `verify` condition and persistent memory |
| [`ir/agent-ir.schema.json`](ir/agent-ir.schema.json) | Draft, implemented | Machine-readable IR schema |
| [`image/v0.1.md`](image/v0.1.md) | Draft, implemented | OCI artifact profile, media types, lockfile, digest |
| [`runtime/v0.1.md`](runtime/v0.1.md) | Draft, superseded in part | Execution model and the backend interface |
| [`runtime/v0.2.md`](runtime/v0.2.md) | Draft, implemented | Runtime 0.1 plus a three-state `verify` outcome |
| [`runtime/v0.3.md`](runtime/v0.3.md) | Draft, implemented | Runtime 0.2 plus streaming, a live channel and transport-decided ceilings |
| [`runtime/v0.4.md`](runtime/v0.4.md) | Draft, implemented | Runtime 0.3 plus a `verify` that runs, and a failed check that stops the run |
| [`runtime/v0.5.md`](runtime/v0.5.md) | Draft, implemented | Runtime 0.4 plus persistent memory and a resumable checkpoint |
| [`runtime/v0.6.md`](runtime/v0.6.md) | Draft, implemented | Runtime 0.5 plus a fan-out that overlaps, and what overlapping may not change |
| [`tools/mcp-v0.2.md`](tools/mcp-v0.2.md) | Draft, implemented | MCP binding 0.1 plus Streamable HTTP and the policy check that governs it |
| [`ingot-conformance`](../crates/ingot-conformance/README.md) | Draft, implemented | Normative tests a backend must pass. A crate rather than a directory here, because it ships inside the `ingot` binary and a package carries only its own files |

Where a specification and the implementation disagree, the specification is
authoritative and the implementation has a bug.

A rule without a test is not considered specified. Tests that pin down the
current rules live in the crate that implements them, plus `tests/golden-ir/`
for the compiled output of the reference examples. Cross-backend differential
tests live in `crates/ingot-cli/tests/differential.rs`.
