//! Human-in-the-loop router battery over the local workflow worker.
//!
//! # Expiry, in one paragraph
//!
//! An unanswered interaction expires whether or not this battery does
//! anything, and the battery cannot change what the run receives. The
//! workflow worker's tick drives the kernel's own credential-free expiry
//! (`InteractionResumeAction::ExpireIfDue`) for a parked interaction whose
//! committed `expires_at` has passed: `park` indexes the deadline on
//! `WakeRow::expires_at`, the tick claims the row on the clock alone, and
//! attaching the session commits the expiry — no principal, no authorization
//! evidence, counted in `TickReport::sessions_expired`.
//!
//! The [`ExpiryPolicy`] hook does *not* substitute an authored refusal for
//! that: a sweep may only offer a row once its deadline has passed, and the
//! interaction ingress rewrites any resolution submitted at or after the
//! deadline into a plain `InteractionSettled::Expired`, discarding the
//! payload. What a policy decides is this router's **row disposition** —
//! [`InteractionStatus::Expired`] and `resolved_by` — which is bookkeeping
//! about the inbox, not a claim about the run. With no policy installed
//! (the fail-closed [`ApprovalExpiry`] default), [`HitlRouter::sweep`]
//! reconciles the row to [`InteractionStatus::Closed`] after the worker
//! expired it. The journal is the authority in every case.
//!
//! # Two things to know before delivering
//!
//! [`InteractionStatus::Delivered`] means *durably buffered*, not accepted:
//! a resolution whose principal and evidence do not exactly equal the run's
//! `RunAccepted` security context is rejected by the ingress on every tick,
//! forever. See [`HitlRouter::resolve`].
//!
//! This battery captures at its own [`park`]. An interaction a session parks
//! on inside the worker's tick is wake-indexed by the worker but never
//! captured here, so it is invisible to [`HitlRouter::pending`] and
//! unresolvable through [`HitlRouter::resolve`]; deliver those to
//! `WorkflowWorker::deliver_interaction` directly. See the crate README's
//! Limitations.

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
