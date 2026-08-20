//! Leased worker over the local workflow driver.
//!
//! Owns three adapter tables — a wake index, cron-fire records, and a
//! response inbox — and a tick loop that claims due work with CAS leases.
//! Every table is a hint; the kernel journal stays authoritative.

mod error;

pub use error::WorkerError;
