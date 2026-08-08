# Conformance suite

Started in milestone M5. The first executable evidence is
`crates/ingot-cli/tests/differential.rs`: it runs one Agent IR artifact and one
cassette through the reference interpreter and an independently generated Python
program, then compares their artifact bytes and event order. Milestone M8 still
has to package these checks for third-party backends and write the backend guide.

Will define the normative tests a backend must pass to claim conformance with a
given Agent IR version, plus the portability levels a report is expressed in:

| Level | Guarantee |
|-------|-----------|
| P0 Parse | the source is a valid program |
| P1 Structural | the target can represent the agent's structure without loss |
| P2 Operational | the required model and tool capabilities are available |
| P3 Policy | the target can enforce the declared permissions and budgets |
| P4 Conformance | the defined behaviour tests pass |

Conformance is about defined behaviour, not identical output. Two runtimes given
the same agent will not produce the same text, and the suite must not pretend
otherwise.
