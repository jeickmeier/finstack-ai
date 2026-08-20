//! Durable multi-writer `PostgreSQL` [`JournalStore`](finstack_ai_runtime::JournalStore)
//! implementation (docs/superpowers/specs/2026-08-20-postgres-journal-design.md).
//!
//! Unlike sqlite's single ordered worker, operations run natively async on
//! tokio through a small hand-rolled connection pool; per-session write
//! serialization is delegated to Postgres row locks rather than client-side
//! ordering. Applications inject this crate explicitly. It is not the Agent,
//! Python, or WASM default (no WASM support: native-tokio only).
//!
//! This crate currently provides configuration, error mapping, schema
//! management, the connection pool, an open path with `health()`, the
//! multi-writer `append` protocol, chain-verified `load`/`load_from`, and
//! snapshot writes, `scan`, and the `write_metadata` CAS (see
//! `src/snapshot.rs`).

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
mod pool;
mod prune;
mod schema;
mod session;
mod snapshot;
mod store;

// Task 2 temporarily widened `schema` to `#[doc(hidden)] pub mod schema;` so
// its integration tests (a separate crate) could drive `ensure_schema`
// directly ahead of `PostgresJournalStore` existing. Now that
// `PostgresJournalStore::try_open` calls `ensure_schema` itself, `schema` is
// back to a private `mod` (`pub(crate)` within, per the brief) and its
// former integration tests moved to in-crate unit tests in `src/schema.rs`,
// which can see `pub(crate)` items.

pub use config::{
    DEFAULT_CONNECT_TIMEOUT, DEFAULT_POOL_SIZE, DEFAULT_SCHEMA, PostgresDurability,
    PostgresStoreConfig, SchemaPolicy,
};
pub use store::PostgresJournalStore;
