//! Memory extension composition (store, recall provider, toolset, observer)
//! for finstack-ai.
//!
//! This crate currently defines the bounded, validated memory record model.
//! Later tasks add the store trait, recall provider, tool surface, and
//! observer that compose on top of it.
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

pub mod record;

pub use record::{
    ExtractionMethod, MemoryBody, MemoryClock, MemoryError, MemoryId, MemoryProvenance,
    MemoryRecord, MemoryScope, RetentionPolicy, system_clock,
};

#[cfg(test)]
mod tests;
