# Runtime and backend interface

Not started. Planned for milestone M3.

Will define what a backend is: how it declares the IR features it supports, how
it lowers each node kind, how it reports what it cannot represent, and how the
driver starts it and normalises its event stream.

Constraints already fixed by the IR specification:

- A backend must reject an `irVersion` whose major component it does not
  implement, rather than ignoring unknown fields.
- A backend must reject, not skip, an unknown node kind, an unknown value kind,
  or a policy decision it cannot enforce. Skipping a node is how a capability
  restriction gets lost.
- A backend that cannot pause for approval must refuse an artifact containing an
  `approval` node.

See [`../ir/v0.1.md`](../ir/v0.1.md).
