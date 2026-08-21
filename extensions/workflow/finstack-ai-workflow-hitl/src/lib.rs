//! Human-in-the-loop inbox and router over the local workflow worker.
//!
//! Capture stores the run's exact accepted principal and authorization
//! evidence with each request. Resolution rejects any mismatch before it can
//! enter the worker inbox. Rows then distinguish `Buffered`, runtime-admitted
//! `Accepted`, durable-ingress `Rejected`, and reconciled `Closed` states.
//! Register [`HitlLifecycle`] on the worker to capture interactions created
//! during worker re-park and to receive authoritative ingress outcomes.
//!
//! The kernel journal remains authoritative. Interaction deadlines are
//! applied by the runtime's existing credential-free expiry path; this crate
//! does not expose a second expiry policy.

mod authorize;
mod capture;
mod error;
mod lifecycle;
mod memory;
mod router;
mod row;
mod sqlite;
mod store;

pub use authorize::{ResolveAuthorizer, TenantAuthorizer};
pub use capture::{MAX_HITL_REQUEST_BYTES, capture, park};
pub use error::HitlError;
pub use lifecycle::HitlLifecycle;
pub use memory::MemoryHitlStore;
pub use router::{HitlRouter, ResolutionInput, SweepReport};
pub use row::{InteractionRow, InteractionStatus, InteractionSummary};
pub use sqlite::SqliteHitlStore;
pub use store::{HitlInboxStore, InteractionTransition};
