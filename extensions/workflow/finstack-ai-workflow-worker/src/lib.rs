//! Leased worker over the local workflow driver.
//!
//! Owns three adapter tables — a wake index, cron-fire records, and a
//! response inbox — and a tick loop that claims due work with CAS leases.
//! Every table is a hint; the kernel journal stays authoritative.

mod error;
mod fires;
mod memory;
mod park;
mod sqlite;
mod wake;

pub use error::WorkerError;
pub use fires::{FireRow, FireStatus, FireStore, idempotency_key};
pub use memory::MemoryWorkerStore;
pub use park::park;
pub use sqlite::SqliteWorkerStore;
pub use wake::{WakeIndexStore, WakeReason, WakeRow, lease_deadline, lease_open, wake_due};
