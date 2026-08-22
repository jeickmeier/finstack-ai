//! Durable local `SQLite` [`JournalStore`](finstack_ai_runtime::ports::journal::JournalStore) implementation.
//!
//! WAL plus `synchronous=FULL` is the only acknowledged durable mode.
//! Records stay append-only; snapshots are a disposable cache. Applications
//! inject this crate explicitly. It is not the Agent, Python, or WASM default.

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

mod append;
mod config;
mod error;
mod journal_store;
mod load;
mod schema;
mod store;
mod worker;

#[cfg(test)]
mod tests;

pub use config::{
    DEFAULT_BUSY_TIMEOUT, SqliteDurability, SqliteStoreConfig, SqliteStoreLimits, SqliteSynchronous,
};
pub use schema::SCHEMA_USER_VERSION;
pub use store::SqliteJournalStore;
