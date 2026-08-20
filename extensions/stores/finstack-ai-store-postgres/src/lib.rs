//! Durable multi-writer `PostgreSQL` [`JournalStore`](finstack_ai_runtime::JournalStore)
//! implementation (docs/superpowers/specs/2026-08-20-postgres-journal-design.md).
//!
//! Unlike sqlite's single ordered worker, operations run natively async on
//! tokio through a small hand-rolled connection pool; per-session write
//! serialization is delegated to Postgres row locks rather than client-side
//! ordering. Applications inject this crate explicitly. It is not the Agent,
//! Python, or WASM default (no WASM support: native-tokio only).
//!
//! This module currently provides the crate scaffold: configuration and
//! error mapping. Schema management, the connection pool, and the
//! `JournalStore` implementation land in subsequent changes.

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

mod config;
mod error;

// `schema` is temporarily `pub` (rather than a private `mod` with selective
// `pub(crate)` re-exports) purely so the integration tests in `tests/` —
// which compile as a separate crate — can drive `ensure_schema` directly
// against a real server ahead of the connection pool landing in a later
// task. See the module doc comment in `src/schema.rs` for the full
// rationale; this should narrow back down once that pool exists.
#[doc(hidden)]
pub mod schema;

pub use config::{
    DEFAULT_CONNECT_TIMEOUT, DEFAULT_POOL_SIZE, DEFAULT_SCHEMA, PostgresDurability,
    PostgresStoreConfig, SchemaPolicy,
};
