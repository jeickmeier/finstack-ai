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

pub mod extract;
pub mod observer;
pub mod provider;
pub mod record;
pub mod store;
pub mod toolset;

pub use extract::{CandidateMemory, DEFAULT_MARKER, MemoryExtractor, RuleBasedExtractor};
pub use observer::MemoryObserver;
pub use provider::{MemoryContextProvider, RecallConfig};
pub use record::{
    ExtractionMethod, MemoryBody, MemoryClock, MemoryError, MemoryId, MemoryProvenance,
    MemoryRecord, MemoryScope, RetentionPolicy, system_clock,
};
pub use store::{
    InProcessArtifactStore, InProcessMemoryStore, MatchEvidence, MemoryHit, MemoryListing,
    MemoryPage, MemoryQuery, MemoryStore, MemoryStoreError, PutOutcome,
};
pub use toolset::{
    INLINE_BODY_MAX_BYTES, MEMORY_TOOL_INVALID_ARGUMENTS, MEMORY_TOOL_NOT_FOUND,
    MEMORY_TOOL_UNAVAILABLE, MemoryPolicy, MemoryToolset,
};

#[cfg(test)]
mod tests;
