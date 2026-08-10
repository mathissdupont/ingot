//! The policy block as a boundary.
//!
//! An agent's `policy` is checked by the compiler and re-checked by the
//! runtime, and both checks answer the same shape of question: *may this call
//! have this effect?* Neither can answer *where did it go?* — a tool server is
//! a separate process with the operator's filesystem and the operator's
//! network, and nothing inside Ingot can see into it.
//!
//! This crate derives, from the same policy, the boundary that server should
//! run inside. See [RFC-0004] and [ADR-0006].
//!
//! ```no_run
//! use std::path::Path;
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let ir = ingot_ir::AgentIr::from_json(&std::fs::read_to_string("CodeReviewTeam.ir.json")?)?;
//! let plan = ingot_sandbox::plan(&ir, "repo", Path::new("/srv/checkout"), &[], false)?;
//!
//! for mount in &plan.mounts {
//!     println!("{} -> {} {}", mount.host.display(), mount.guest,
//!              if mount.writable { "rw" } else { "ro" });
//! }
//! for note in &plan.unenforceable {
//!     println!("cannot enforce {}: {}", note.policy, note.reason);
//! }
//! # Ok(())
//! # }
//! ```
//!
//! # Two halves, and why they are separate
//!
//! [`plan()`] is pure: it reads an artifact and a workspace path and produces a
//! description. It starts nothing, needs no container runtime, and runs on any
//! platform. That is what makes the interesting logic — which paths, which
//! direction, what cannot be honoured — testable everywhere.
//!
//! Executing a plan needs a container runtime, so it lives behind a separate
//! interface and is the only part that has to care what is installed.
//!
//! # What it does not contain
//!
//! The interpreter, which is ours and follows a checked artifact, and the
//! model. Stage 1 contains **tool servers** — somebody else's program, doing
//! effectful work. Containing the run itself is stage 2, and [ADR-0006] blocks
//! it on a second backend existing.
//!
//! [RFC-0004]: https://github.com/mathissdupont/ingot/blob/main/rfcs/0004-ingot-containers.md
//! [ADR-0006]: https://github.com/mathissdupont/ingot/blob/main/docs/adr/0006-a-policy-enforcing-runner.md

pub mod egress;
pub mod executor;
pub mod plan;
pub mod report;

pub use egress::{EgressBoundary, EgressRoute, DEFAULT_EGRESS_IMAGE};
pub use executor::{
    detect, image_digests, image_exists, invocation, ExecutorError, Runtime, RUNTIMES,
};
pub use plan::{
    plan, Mount, Network, PlanError, SandboxPlan, Unenforceable, GUEST_WORKSPACE, RUN_SUBJECT,
};
pub use report::render;
