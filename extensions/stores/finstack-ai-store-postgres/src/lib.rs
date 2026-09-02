//! Durable multi-writer `PostgreSQL` [`JournalStore`](finstack_ai_runtime::ports::journal::JournalStore)
//! implementation (docs/superpowers/specs/2026-08-20-postgres-journal-design.md).
//!
//! Unlike sqlite's single ordered worker, operations run natively async on
//! tokio through a small hand-rolled connection pool; per-session write
//! serialization is delegated to Postgres row locks rather than client-side
//! ordering. Applications inject this crate explicitly. It is not the Agent,
//! Python, or WASM default (no WASM support: native-tokio only).
//!
//! This crate provides the complete [`JournalStore`](finstack_ai_runtime::ports::journal::JournalStore)
//! surface over `PostgreSQL`: configuration, error mapping, schema
//! management, the connection pool, an open path with `health()`, the
//! multi-writer `append` protocol, chain-verified `load`/`load_from`,
//! snapshot writes, `scan`, the `write_metadata` CAS (see
//! `src/snapshot.rs`), and the snapshot-aligned prefix `prune` (see
//! `src/prune.rs`).
//!
//! TLS (rustls, hostname-verified against the bundled `WebPKI` roots plus any
//! configured PEM anchors) is required by default; plaintext transport needs
//! the explicit `PostgresTlsMode::Disable` setting (see the TLS note on
//! `src/store.rs`). Multiple writers, including across processes, may append
//! to the same session concurrently: per-session ordering is enforced by
//! Postgres row locks rather than client-side serialization.

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

/// The `records` column list, in the order `load::reconstruct_envelope`
/// reads it.
///
/// A macro rather than a `const` so the three record-selecting statements
/// can be assembled with `concat!` into `&'static str` literals: the
/// per-connection statement cache (`pool::PooledClient::prepared`) is keyed
/// by the SQL literal, and a `format!`ed `String` could never be that key.
/// Defined in the crate root, before the `mod` declarations, so textual
/// macro scoping makes it visible to every module below.
macro_rules! record_columns {
    () => {
        "session_id, sequence, record_id, lane_id, run_id, kind, format_version, kind_version, \
         payload_cbor, timestamp, payload_digest, previous_checksum, envelope_checksum, \
         derived_event_ids"
    };
}

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

pub use config::{
    DEFAULT_CHECKOUT_TIMEOUT, DEFAULT_CONNECT_TIMEOUT, DEFAULT_OPERATION_TIMEOUT,
    DEFAULT_POOL_SIZE, DEFAULT_SCHEMA, PostgresDurability, PostgresStoreConfig, PostgresTlsMode,
    SchemaPolicy,
};
pub use store::PostgresJournalStore;
