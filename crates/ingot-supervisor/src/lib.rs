//! The channel between a contained run and the host that supervises it.
//!
//! [RFC-0004] put each tool server inside a boundary derived from the artifact's
//! policy and left the interpreter on the host. That contained the narrow half:
//! the process holding the API key, reading the model's answers and writing the
//! output directory still had the operator's whole machine. [RFC-0005] moves the
//! run inside, and this crate is what makes that possible.
//!
//! ```text
//! host                                  inside the boundary
//! ────                                  ───────────────────
//! ingot run --contained                   ingot exec
//!   holds the credential                    the interpreter
//!   holds the terminal                      the MCP tool servers
//!   holds the output directory              network deny still holds
//!
//!   ├── config ────────────────────────►    the IR, the inputs, the ceiling
//!   │◄─ model ─────────────────────────┤    a completion, fetched from outside
//!   │◄─ approval ──────────────────────┤    asked inside, decided outside
//!   │◄─ event ─────────────────────────┤    progress
//!   │◄─ finished ──────────────────────┤    the outputs
//! ```
//!
//! # Why this is a separate crate
//!
//! The interpreter does not know it is contained, and must not learn.
//! [`guest::GuestProvider`] is an ordinary [`ingot_runtime::ModelProvider`]; the
//! approval handler is an ordinary [`ingot_runtime::ApprovalHandler`]. Nothing in
//! `ingot-runtime` changed to make a contained run possible, which is the test of
//! whether the boundary is really a deployment concern — and a backend that
//! contains runs some other way replaces this crate and nothing else, the same
//! bargain [ADR-0005] struck for tool hosting.
//!
//! # What does not cross
//!
//! The credential and the filesystem outside the policy's mounts. Both by
//! construction rather than by discipline: the guest's environment contains
//! nothing but what `pass-env` named, and `--out-dir` is written by the host
//! after the run from the outputs the guest returned. The guest cannot write
//! there and does not know where it is.
//!
//! [RFC-0004]: https://github.com/mathissdupont/ingot/blob/main/rfcs/0004-ingot-containers.md
//! [RFC-0005]: https://github.com/mathissdupont/ingot/blob/main/rfcs/0005-the-contained-run.md
//! [ADR-0005]: https://github.com/mathissdupont/ingot/blob/main/docs/adr/0005-mcp-over-stdio-only.md

pub mod guest;
pub mod host;
pub mod protocol;

pub use guest::{Guest, GuestApprovals, GuestError, GuestEvents, GuestProvider};
pub use host::{
    serve, supervise, Deadlines, GuestLines, HostError, Outcome, ReadLines, Supervisor,
};
pub use protocol::{Failed, Finished, RunConfig, WireError, PROTOCOL_VERSION};
