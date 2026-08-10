//! Runtime ports and effect execution for `finstack-ai`.
//!
//! Owns the six primary port contracts and effect execution. Target drivers
//! are selected by the non-default `native-tokio` and `wasm-host` features.
//!
//! PR-014 adds the journal-store boundary and the authoritative commit loop.

#![warn(missing_docs)]

mod audit;
mod coordinator;
mod error;
mod id_generation;
mod journal;
mod ports;

#[cfg(feature = "native-tokio")]
mod task;

#[cfg(feature = "native-tokio")]
mod ingress;

pub use audit::{
    SecurityAuditCategory, SecurityAuditError, SecurityAuditEvent, SecurityAuditHealth,
    SecurityAuditReceipt, SecurityAuditSink,
};

pub use coordinator::{CommitCoordinator, CommitCoordinatorError, CommitOutcome, RunFault};

pub use error::FrameworkError;
pub use id_generation::{Clock, IdGenerationError, RandomSource, UuidV7Generator};
pub use journal::{
    JournalStore, LoadRequest, LoadedSession, OpaqueSnapshot, SnapshotReceipt, SnapshotRequest,
    StoreCommitTimestamp, StoreError, StoreHealth,
};
pub use ports::{PortFuture, PortObject};

#[cfg(feature = "native-tokio")]
pub use task::{RunHandle, RunHandleError, RunStatus, RunTaskConfig, RunTaskOwner};

#[cfg(feature = "native-tokio")]
pub use audit::{SecurityAuditGate, SecurityAuditGateError};

#[cfg(feature = "native-tokio")]
pub use ingress::{
    ExternalCompletionRouter, ExternalRouteError, ExternalRouteOutcome, InteractionRouter,
};

#[cfg(feature = "native-tokio")]
pub use id_generation::{OsRandomSource, SystemClock};
