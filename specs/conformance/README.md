# Conformance suite

Not started. Planned for milestone M8.

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
