//! Host-facing router over the HITL inbox and the workflow worker.

use std::sync::Arc;

use finstack_ai_kernel::{
    AuthorizationEvidence, InteractionRequest, InteractionResolution, InteractionResolutionCommand,
    OperationLocator, PrincipalRef, RawJson, Timestamp,
};
use finstack_ai_workflow_worker::{WakeIndexStore, WorkflowWorker};

use crate::authorize::{ResolveAuthorizer, TenantAuthorizer};
use crate::error::HitlError;
use crate::row::{InteractionRow, InteractionStatus};
use crate::store::{HitlInboxStore, InteractionTransition};

/// One authorized decision for [`HitlRouter::resolve`]: everything the
/// resolution carries besides the row key, which the router looks up itself.
///
/// Grouping these ends the run of same-typed positional parameters a caller
/// could silently transpose; the fields travel unchanged from `resolve`
/// through delivery into the kernel's `InteractionResolution`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolutionInput {
    /// Idempotent resolution identity.
    pub resolution_id: Arc<str>,
    /// Resolving principal; must equal the run's accepted principal.
    pub principal: PrincipalRef,
    /// Authorization evidence; must equal the run's accepted evidence.
    pub evidence: AuthorizationEvidence,
    /// Response payload conforming to the request's schema.
    pub payload: RawJson,
    /// Optional operator note.
    pub note: Option<Arc<str>>,
}

/// Reads the HITL inbox and turns an operator's decision into a durable
/// interaction resolution delivered to the worker.
///
/// The inbox is a hint; the kernel journal stays authoritative. Delivery is
/// attempted before the row is marked [`InteractionStatus::Buffered`], so a
/// failed delivery leaves the row `Open` and retryable.
pub struct HitlRouter {
    /// Adapter-owned inbox this router reads and settles.
    store: Arc<dyn HitlInboxStore>,
    /// Worker that durably buffers the resolution for a later tick.
    worker: Arc<WorkflowWorker>,
    /// Wake index, used by reconciliation.
    wake: Arc<dyn WakeIndexStore>,
    /// Authorization hook consulted before every resolution.
    authorizer: Arc<dyn ResolveAuthorizer>,
}

const DEFAULT_QUERY_LIMIT: usize = 100;

/// What one [`HitlRouter::sweep`] pass changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SweepReport {
    /// Active rows closed because their interaction wake was absent.
    pub reconciled: usize,
}

impl HitlRouter {
    /// Router over an inbox, a worker, and the worker's wake index, using the
    /// default [`TenantAuthorizer`].
    #[must_use]
    pub fn new(
        store: Arc<dyn HitlInboxStore>,
        worker: Arc<WorkflowWorker>,
        wake: Arc<dyn WakeIndexStore>,
    ) -> Self {
        Self {
            store,
            worker,
            wake,
            authorizer: Arc::new(TenantAuthorizer),
        }
    }

    /// Replace the authorization hook.
    #[must_use]
    pub fn with_authorizer(mut self, authorizer: Arc<dyn ResolveAuthorizer>) -> Self {
        self.authorizer = authorizer;
        self
    }

    /// Open interactions for one tenant, oldest first (host inbox view).
    ///
    /// # Errors
    ///
    /// Returns store-unavailable or integrity failures from
    /// [`HitlInboxStore::load_open`].
    pub fn pending(&self, tenant_scope: &str) -> Result<Vec<InteractionRow>, HitlError> {
        self.store.load_open(tenant_scope, DEFAULT_QUERY_LIMIT)
    }

    /// Authorize a decision, deliver it durably, and settle the inbox row.
    ///
    /// Delivery happens before the status transition: if the worker rejects
    /// the command the row stays `Open` and the call can be retried.
    ///
    /// # `Buffered` means durable, not yet accepted
    ///
    /// This router hands the command to the worker's inbox and marks the row
    /// [`crate::InteractionStatus::Buffered`]. Whether the journal accepts
    /// it is decided later, by the runtime's interaction ingress, which
    /// admits a resolution only when its principal **and** its authorization
    /// evidence exactly equal the run's own `RunAccepted` security context —
    /// the credentials the host presented when it accepted the run
    /// (`authorization_matches`,
    /// `crates/finstack-ai-runtime/src/driver/ingress/shared.rs`).
    ///
    /// The router compares both values with the accepted context captured on
    /// the row and rejects mismatches before delivery. With [`crate::HitlLifecycle`]
    /// registered, the worker moves the row to `Accepted` or `Rejected` after
    /// the runtime ingress returns its durable outcome.
    ///
    /// Note also that a resolution submitted at or after the request's
    /// `expires_at` is rewritten into an expiry by that same ingress, so a
    /// late answer settles the interaction without its payload.
    ///
    /// If the final [`HitlInboxStore::transition`] fails after a successful
    /// delivery the row stays `Open` and visible in [`HitlRouter::pending`]
    /// even though the command is already buffered; the worker inbox's keyed
    /// upsert makes the retry harmless.
    ///
    /// Once-only resolution is enforced by the journal's settlement, not by
    /// this inbox: [`HitlError::NotOpen`] is a serial-use guard against
    /// double submission, not a concurrency guarantee.
    ///
    /// # Errors
    ///
    /// Returns [`HitlError::UnknownInteraction`] when no row exists,
    /// [`HitlError::NotOpen`] when the row is already settled,
    /// [`HitlError::StoreIntegrity`] with code `"hitl_request_decode"` when
    /// the stored envelope cannot be decoded or `"hitl_locator"` when the
    /// row's coordinates do not form a locator, the authorizer's
    /// [`HitlError::Unauthorized`], [`HitlError::InvalidResolution`] with
    /// code `"hitl_resolution"` when the kernel rejects the resolution
    /// inputs, and [`HitlError::Worker`] when delivery fails.
    pub fn resolve(
        &self,
        tenant_scope: &str,
        interaction_id: &str,
        input: ResolutionInput,
        now: Timestamp,
    ) -> Result<(), HitlError> {
        let row = self
            .store
            .load(tenant_scope, interaction_id)?
            .ok_or(HitlError::UnknownInteraction)?;
        if !matches!(
            row.status,
            InteractionStatus::Open | InteractionStatus::Rejected
        ) {
            return Err(HitlError::NotOpen);
        }
        let request: InteractionRequest =
            serde_json::from_slice(row.request.as_ref()).map_err(|_| {
                HitlError::StoreIntegrity {
                    code: "hitl_request_decode",
                }
            })?;
        if input.principal != row.accepted_principal || input.evidence != row.accepted_evidence {
            return Err(HitlError::Unauthorized {
                code: "accepted_context_mismatch",
            });
        }
        self.authorizer
            .authorize(&row, &request, &input.principal)?;

        let subject = self.deliver(&row, &request, input, now)?;
        if !self.store.transition(
            tenant_scope,
            interaction_id,
            InteractionTransition {
                expected: row.status,
                next: InteractionStatus::Buffered,
                resolved_by: Some(subject.as_ref()),
                outcome_code: Some("buffered"),
                updated_at: now,
            },
        )? {
            return Err(HitlError::NotOpen);
        }
        Ok(())
    }

    /// Reconcile active inbox rows against the worker's wake index.
    ///
    /// Any active (`Open` or `Buffered`) row
    /// with no matching `Interaction` wake row was already settled out of
    /// band, including through kernel deadline expiry. It is closed without
    /// authoring another resolution. The journal remains authoritative.
    ///
    /// The first row that errors aborts the pass. Each transition is
    /// independently durable, so rows already transitioned stay that way and
    /// re-running the sweep is safe.
    ///
    /// # Errors
    ///
    /// Returns store failures from [`HitlInboxStore::load_active_summaries`]
    /// or [`HitlInboxStore::transition`], and [`HitlError::Worker`] when the
    /// wake index query fails.
    pub fn sweep(&self, now: Timestamp) -> Result<SweepReport, HitlError> {
        let mut report = SweepReport::default();

        for row in self.store.load_active_summaries(DEFAULT_QUERY_LIMIT)? {
            let awaited = self
                .wake
                .contains_interaction(row.tenant_scope.as_ref(), row.interaction_id.as_ref())?;

            if !awaited
                && self.store.transition(
                    row.tenant_scope.as_ref(),
                    row.interaction_id.as_ref(),
                    InteractionTransition {
                        expected: row.status,
                        next: InteractionStatus::Closed,
                        resolved_by: row.resolved_by.as_deref(),
                        outcome_code: Some("wake_reconciled"),
                        updated_at: now,
                    },
                )?
            {
                report.reconciled += 1;
            }
        }
        Ok(report)
    }

    /// Build the resolution command for `row` and buffer it in the worker
    /// inbox. Returns the resolving principal's subject, for the caller's
    /// status transition.
    fn deliver(
        &self,
        row: &InteractionRow,
        request: &InteractionRequest,
        input: ResolutionInput,
        now: Timestamp,
    ) -> Result<Arc<str>, HitlError> {
        let locator = OperationLocator::try_new(
            row.tenant_scope.as_ref(),
            row.session_id,
            row.lane_id,
            row.run_id,
        )
        .map_err(|_| HitlError::StoreIntegrity {
            code: "hitl_locator",
        })?;
        let subject: Arc<str> = Arc::from(input.principal.subject());
        let resolution = InteractionResolution::try_new(
            request.interaction_id(),
            input.resolution_id.as_ref(),
            input.principal,
            input.evidence,
            input.payload,
            input.note.as_deref(),
        )
        .map_err(|_| HitlError::InvalidResolution {
            code: "hitl_resolution",
        })?;
        let command = InteractionResolutionCommand::try_new(locator, resolution).map_err(|_| {
            HitlError::InvalidResolution {
                code: "hitl_resolution",
            }
        })?;
        self.worker.deliver_interaction(&command, now)?;
        Ok(subject)
    }
}
