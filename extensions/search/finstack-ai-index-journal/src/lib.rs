//! Scoped lexical search over committed journal entries and tool results.
//! Observers enqueue identifiers only; bounded maintenance reads the journal.
//! Explicit authorized session IDs are required. There is no global discovery.
#![warn(missing_docs)]
#![forbid(unsafe_code)]
#![warn(clippy::float_cmp)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable
)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]
#![doc(test(attr(allow(clippy::expect_used))))]

mod catalog;
mod database;
mod extract;
mod indexing;
mod observer;
mod retrieval;
mod source;
pub use indexing::JournalIndexReport;
pub use observer::JournalIndexObserver;
pub use source::{JournalIndexConfig, JournalSearchSource};
