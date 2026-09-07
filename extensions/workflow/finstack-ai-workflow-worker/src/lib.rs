//! Leased worker over the local workflow driver.
//!
//! Owns three adapter tables — a wake index, cron-fire records, and a
//! response inbox — and a tick loop that claims due work with CAS leases.
//! Every table is a hint; the kernel journal stays authoritative.

mod error;
mod fires;
mod inbox;
mod memory;
mod park;
mod recovery;
mod recovery_sqlite;
mod sqlite;
mod wake;
mod worker;

pub use error::WorkerError;
pub use fires::{
    FireIdempotencyKey, FireRow, FireStartOutcome, FireStatus, FireStore, idempotency_key,
};
pub use inbox::{
    DeadLetterRow, InboxInsertOutcome, InboxKind, InboxRow, InboxStore, MAX_INBOX_PAYLOAD_BYTES,
};
pub use memory::MemoryWorkerStore;
pub use park::park_for_wake;
pub use recovery::{
    MAX_RECOVERY_DESCRIPTOR_BYTES, MAX_RECOVERY_SCAN, RecoveryRegistration, RecoveryStore,
};
pub use sqlite::{SqliteWorkerStore, is_memory_sqlite_path};
pub use wake::{WakeIndexStore, WakeReason, WakeRow, lease_deadline, lease_open, wake_due};
pub use worker::{
    InteractionDeliveryOutcome, InteractionLifecycle, PortsFactory, RunStarter, StartedRun,
    TickReport, WorkerBuilder, WorkerHandle, WorkflowExecution, WorkflowWorker,
};
