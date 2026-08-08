//! A second Agent IR backend: one self-contained Python 3 file per agent.
//!
//! The project's central claim is that a typed source compiles to something
//! portable. Until this crate existed, exactly one program read an Agent IR
//! document — the reference interpreter — and a format with one consumer is
//! indistinguishable from that consumer's internal representation. This crate is
//! the falsification test, not a feature. See [RFC-0006] and
//! [GAP-018](https://github.com/mathissdupont/ingot/blob/main/docs/gaps.md#gap-018).
//!
//! ```no_run
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let ir = ingot_ir::AgentIr::from_json(&std::fs::read_to_string("Brief.ir.json")?)?;
//!
//! // What this target would make of the agent, before anything is written.
//! let report = ingot_backend_python::analyse(ingot_backend_python::TARGET, &[ir.clone()]);
//! println!("{}", report.render());
//!
//! if report.buildable() {
//!     std::fs::write("Brief.py", ingot_backend_python::emit(&ir)?)?;
//! }
//! # Ok(())
//! # }
//! ```
//!
//! # Two rules this crate is built to obey
//!
//! **It does not depend on `ingot-runtime`, and must not.** A backend that could
//! reach into the reference interpreter would inherit the semantics it is
//! supposed to be an independent check on. `Cargo.toml` is the enforcement:
//! `ingot-ir` and `serde` are the whole dependency list.
//!
//! **The emitted semantics come from the specification.** [`emit`] lowers to
//! Python, and the runtime the output carries was written from
//! [Runtime 0.1] and [Agent IR 0.1] rather than from reading the Rust. A Python
//! file transliterated from `ingot-runtime` would agree with it by construction
//! and demonstrate nothing; the point is that it *can* disagree, and that a
//! disagreement is triaged rather than papered over.
//!
//! # A compiler backend, not a Python interpreter
//!
//! The output contains no IR document. A `branch` is an `if`, a `loop` is a
//! `for` with a static bound, a binding is a local variable. Embedding the JSON
//! and interpreting it at run time would let every field this backend does not
//! understand pass through untouched — which is the exact failure a second
//! consumer exists to catch.
//!
//! [RFC-0006]: https://github.com/mathissdupont/ingot/blob/main/rfcs/0006-a-second-backend.md
//! [Runtime 0.1]: https://github.com/mathissdupont/ingot/blob/main/specs/runtime/v0.1.md
//! [Agent IR 0.1]: https://github.com/mathissdupont/ingot/blob/main/specs/ir/v0.1.md

pub mod emit;
pub mod report;

pub use emit::{emit, EmitError};
pub use report::{analyse, analyse_agent, AgentReport, Finding, Report, Support};

/// How `--target` spells this backend.
pub const TARGET: &str = "python";

/// The conventional file extension for its output.
pub const EXTENSION: &str = "py";
