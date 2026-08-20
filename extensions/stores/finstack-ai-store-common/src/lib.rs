//! Shared journal-store semantics for finstack-ai store backends.
//!
//! Pure functions over the kernel, protocol, and runtime port types. Backends
//! (memory, sqlite, future stores) own all storage plumbing and delegate the
//! store-contract decisions — idempotency, admission, chain verification,
//! windowing — to this crate so the semantics cannot drift between them.

#![warn(missing_docs)]
#![forbid(unsafe_code)]
#![warn(clippy::float_cmp)]
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
#![deny(clippy::panic)]
#![deny(clippy::unreachable)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::indexing_slicing,
        clippy::float_cmp,
    )
)]

mod error;

pub use error::protocol_error;
