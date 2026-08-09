//! The one JSON encoding a package is allowed to use.
//!
//! Every document a package generates follows the
//! [ADR-0004](../../../docs/adr/0004-canonical-ir-encoding.md) rules the Agent
//! IR already follows: sorted object keys, two-space indentation, exactly one
//! trailing newline. A package digest is only an identity if the bytes are
//! reproducible, and JSON has four different ways to stop being reproducible by
//! accident.
//!
//! Key ordering is not enforced here but by the types: every map in this crate
//! is a `BTreeMap` and every struct's field order is fixed by declaration, so
//! unsorted output is unrepresentable rather than merely discouraged.

use serde::Serialize;

/// Serialise in the canonical encoding.
pub(crate) fn canonical<T: Serialize>(value: &T) -> String {
    let mut json =
        serde_json::to_string_pretty(value).expect("every document in this crate is serialisable");
    json.push('\n');
    json
}
