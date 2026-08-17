//! Durable local sqlite [`JournalStore`](finstack_ai_runtime::JournalStore) implementation (TDD §18.2 / PR-040).
//!
//! WAL plus `synchronous=FULL` is the only acknowledged durable mode.
//! Records stay append-only; snapshots are a disposable cache. Applications
//! inject this crate explicitly. It is not the Agent, Python, or WASM default.

#![warn(missing_docs)]

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
