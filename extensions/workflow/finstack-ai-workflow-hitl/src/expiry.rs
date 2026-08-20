//! Expiry policy for interactions that cross their deadline.
//!
//! The sweep never invents a decision on its own: it asks an
//! [`ExpiryPolicy`], and a policy that declines leaves the row `Open` for the
//! next sweep. [`ApprovalExpiry`] is the fail-closed default, and it declines
//! everything — see its documentation for why no router-authored credential
//! can be admitted by the runtime.
//!
//! # What a policy actually controls
//!
//! Not the run's outcome. A policy's authored payload reaches the journal
//! *as that payload* only if it is submitted **before** the deadline, and a
//! sweep cannot arrange that: it may only hand a row to a policy once
//! `now >= expires_at`. The worker then submits the buffered command with its
//! own clock, which under one shared clock — what a real host has — is also
//! `>= expires_at`. The interaction ingress is fail-closed on a late answer:
//! `interaction_settled_input`
//! (`crates/finstack-ai-runtime/src/driver/ingress/shared.rs`) rewrites any
//! resolution whose `submitted_at` is at or after the pending request's
//! `expires_at` into `InteractionSettled::Expired` before the reducer sees
//! it. The payload is discarded and the journal records a plain expiry with
//! no resolution identity. `tests/hitl/uc05.rs` asserts exactly this on a
//! single clock.
//!
//! So an [`ExpiryPolicy`] is honestly an **inbox-annotation hook**: it
//! decides this router's row disposition — its [`crate::InteractionStatus`]
//! and `resolved_by` — and nothing about what the waiting run receives. The
//! hook stays because that annotation is useful to a host's own reporting;
//! just do not read `Expired`/`resolved_by` as a claim about the run.
//!
//! # Declining is not the same as never expiring
//!
//! An unanswered interaction expires with or without this battery, and
//! without any credential at all: the workflow worker's tick drives the
//! kernel's own semantic expiry (`InteractionResumeAction::ExpireIfDue`) for
//! a parked interaction whose committed `expires_at` has passed. `park`
//! indexes that deadline on `WakeRow::expires_at`, the tick claims the row on
//! the clock alone once it is past, and attaching the session is what commits
//! `InteractionSettled::Expired` — no principal, no authorization evidence,
//! because a deadline is a semantic event rather than an authorized answer.
//! Each one is counted in `TickReport::sessions_expired`. That is the
//! effective mechanism in every configuration, which is why declining costs
//! the run nothing.
//!
//! With no policy installed, the expired interaction no longer classifies as
//! that wait, so the next [`crate::HitlRouter::sweep`] reconciles the inbox
//! row to `Closed`. The journal stays the authority throughout.
//!
//! # Why an expiry resolution needs the run's own credentials
//!
//! A resolution reaches the journal through the runtime's interaction
//! ingress, which admits it only when
//! `crates/finstack-ai-runtime/src/driver/ingress/shared.rs:85`
//! (`authorization_matches`) holds:
//!
//! ```text
//! security.principal() == principal
//!     && security.authorization_policy_version() == authorization.policy_version()
//!     && security.authorization_decision_id() == authorization.decision_id()
//! ```
//!
//! where `security` is the run's own `RunAccepted` security context. The only
//! admissible credential is therefore the exact principal and evidence the
//! host presented when it accepted the run. That is per-run data: it is not
//! on the [`InteractionRow`], not in the committed [`InteractionRequest`]
//! (the runtime's own approval request sets `assignee_hint: None` —
//! `crates/finstack-ai-runtime/src/exec/settlement/interaction.rs:130`), and
//! not reachable from this battery's synchronous [`crate::HitlRouter::sweep`].
//! This battery cannot author them, so it declines rather than forging a
//! credential the ingress would reject on every tick. A host that holds them
//! can install a policy that presents them — but note the section above: past
//! the deadline the ingress rewrites even a perfectly credentialed
//! resolution into an expiry, so what such a policy buys is the row
//! annotation, not the run's outcome.

use finstack_ai_kernel::{AuthorizationEvidence, InteractionRequest, PrincipalRef, RawJson};

use crate::error::HitlError;
use crate::row::InteractionRow;

/// What an expired interaction resolves to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpiryResolution {
    /// Principal recorded as the resolver of the expiry.
    pub principal: PrincipalRef,
    /// Authorization evidence for the expiry decision.
    pub evidence: AuthorizationEvidence,
    /// Response payload delivered to the waiting run.
    pub payload: RawJson,
}

/// Host policy for interactions that cross `expires_at`.
pub trait ExpiryPolicy: Send + Sync {
    /// Decide what `row` (whose decoded envelope is `request`) resolves to
    /// now that its deadline has passed. `Ok(None)` takes no action and
    /// leaves the row `Open`.
    ///
    /// # Errors
    ///
    /// Returns a [`HitlError`] when the policy cannot decide; the sweep
    /// aborts on the first such error.
    fn expire(
        &self,
        row: &InteractionRow,
        request: &InteractionRequest,
    ) -> Result<Option<ExpiryResolution>, HitlError>;
}

/// Default policy: decline every interaction, so a sweep never delivers a
/// resolution the runtime would reject.
///
/// This battery holds no credential the interaction ingress will admit (see
/// the module documentation), so it declines rather than forging one. The
/// row stays `Open` and visible in [`crate::HitlRouter::pending`] and an
/// operator can still resolve it, right up to the deadline.
///
/// Declining costs the run nothing. Past the deadline the worker's tick
/// expires the interaction through the kernel's own credential-free path and
/// the run proceeds; the next sweep then reconciles this row to `Closed`.
///
/// What a declining policy gives up is the *row annotation* — `Expired` plus
/// a `resolved_by` — and not an outcome for the run. An installed policy
/// cannot buy that outcome either: the sweep only offers a row once its
/// deadline has passed, and past the deadline the ingress rewrites even a
/// perfectly credentialed resolution into a plain expiry. See the module
/// documentation.
///
/// An earlier version of this policy refused expired approvals under a
/// synthetic `("finstack.workflow.hitl", "expiry", Some(tenant))` principal.
/// That resolution was durably buffered and the row was stamped `Expired`,
/// but the ingress rejected it as `scope_mismatch` on every tick: the run
/// stalled on `RunPhase::AwaitingInteraction` forever while the row had
/// already left the pending view, with no operator-visible trace. A policy
/// that declines is strictly safer than one that lies.
///
/// # Installing a policy anyway
///
/// A host that accepted the run holds the principal and evidence the ingress
/// requires, and can install a policy that presents them via
/// [`crate::HitlRouter::with_expiry_policy`]. The crate's UC-05 spec
/// (`tests/hitl/uc05.rs`) drives exactly that end to end on one clock, and
/// asserts the honest result: the row becomes `Expired` with the policy's
/// principal recorded, while the journal's terminal outcome is `Expired`
/// rather than `Denied` and no resolution identity exists — the authored
/// payload was discarded at the ingress. Install a policy for the row
/// disposition, not for the run.
#[derive(Debug, Clone, Copy, Default)]
pub struct ApprovalExpiry;

impl ExpiryPolicy for ApprovalExpiry {
    fn expire(
        &self,
        _row: &InteractionRow,
        _request: &InteractionRequest,
    ) -> Result<Option<ExpiryResolution>, HitlError> {
        Ok(None)
    }
}

/// What one [`crate::HitlRouter::sweep`] pass changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SweepReport {
    /// Rows whose deadline passed and whose policy resolution was delivered.
    pub expired: usize,
    /// Active rows closed because their wake row was already consumed.
    pub reconciled: usize,
}
