//! Memory extension composition (store, recall provider, toolset, observer)
//! for finstack-ai.
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
// Allow expect() in doc tests (they are test code)
#![doc(test(attr(allow(clippy::expect_used))))]

// Each module is one component of the composition, and is the single canonical
// path to the items it owns. Nothing is re-exported at the crate root: the 31
// bound constants under `record`, `store` and `toolset` were already reachable
// only through their module, so flattening the types alongside them produced
// two paths for some items and one for the rest.
pub mod extract;
pub mod observer;
pub mod provider;
pub mod record;
pub mod store;
pub mod toolset;

#[cfg(test)]
mod tests;
