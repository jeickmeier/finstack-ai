//! Human-in-the-loop router battery over the local workflow worker.
//!
//! # Expiry, in one paragraph
//!
//! An unanswered interaction expires whether or not this battery does
//! anything. The workflow worker's tick drives the kernel's own
//! credential-free expiry (`InteractionResumeAction::ExpireIfDue`) for a
//! parked interaction whose committed `expires_at` has passed: `park` indexes
//! the deadline on `WakeRow::expires_at`, the tick claims the row on the clock
//! alone, and attaching the session commits the expiry — no principal, no
//! authorization evidence, counted in `TickReport::sessions_expired`. What
//! this battery adds is optional: the fail-closed [`ApprovalExpiry`] default
//! and the host-installed [`ExpiryPolicy`] hook let a host that holds the
//! run's own credentials hand the run an *authored refusal payload* instead
//! of a plain expiry. After a worker-driven expiry, [`HitlRouter::sweep`]
//! reconciles the inbox row to [`InteractionStatus::Closed`]; the row's
//! `Expired` status only ever comes from a host policy's delivered refusal.
//! The journal remains the authority in every case.

mod authorize;
mod capture;
mod error;
mod expiry;
mod memory;
mod router;
mod row;
mod sqlite;
mod store;

pub use authorize::{ResolveAuthorizer, TenantAuthorizer};
pub use capture::{capture, park};
pub use error::HitlError;
pub use expiry::{ApprovalExpiry, ExpiryPolicy, ExpiryResolution, SweepReport};
pub use memory::MemoryHitlStore;
pub use router::HitlRouter;
pub use row::{InteractionRow, InteractionStatus};
pub use sqlite::SqliteHitlStore;
pub use store::HitlInboxStore;
