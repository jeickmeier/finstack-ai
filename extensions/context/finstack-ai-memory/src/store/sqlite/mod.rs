//! SQLite-backed durable [`MemoryStore`], gated behind the `sqlite` feature.
//!
//! One bounded command queue feeds a dedicated worker that exclusively owns
//! the [`rusqlite::Connection`], so database calls never block an async
//! executor thread. WAL journaling is used where the backing file supports
//! it. Each write applies the expiry/receipt sweep plus its data and
//! idempotency receipt in one immediate transaction; reads run without a
//! transaction and filter expired rows by predicate instead of sweeping.
//!
//! Schema, worker, and query code live in sibling modules. Add behavior
//! there rather than growing this facade.

mod coverage;
mod queries;
mod schema;
mod worker;

pub use worker::SqliteMemoryStore;
