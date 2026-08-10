//! Runtime ports and effect execution for `finstack-ai`.
//!
//! Owns the six primary port contracts and effect execution. Target drivers
//! are selected by the non-default `native-tokio` and `wasm-host` features.
//!
//! PR-014 adds the journal-store boundary and its first non-durable leaf.

#![warn(missing_docs)]

mod error;
mod id_generation;
mod journal;
mod ports;

pub use error::FrameworkError;
pub use id_generation::{Clock, IdGenerationError, RandomSource, UuidV7Generator};
pub use journal::{
    JournalStore, LoadRequest, LoadedSession, OpaqueSnapshot, SnapshotReceipt, SnapshotRequest,
    StoreCommitTimestamp, StoreError, StoreHealth,
};
pub use ports::{PortFuture, PortObject};

#[cfg(feature = "native-tokio")]
pub use id_generation::{OsRandomSource, SystemClock};
