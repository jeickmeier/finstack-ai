//! Host-facing router over the HITL inbox and the workflow worker.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use finstack_ai_kernel::{
    AuthorizationEvidence, InteractionRequest, InteractionResolution, InteractionResolutionCommand,
    OperationLocator, PrincipalRef, RawJson, Timestamp,
};
use finstack_ai_workflow_worker::{WakeIndexStore, WakeReason, WorkflowWorker};

use crate::authorize::{ResolveAuthorizer, TenantAuthorizer};
use crate::error::HitlError;
use crate::expiry::{ApprovalExpiry, ExpiryPolicy, SweepReport};
use crate::row::{InteractionRow, InteractionStatus, InteractionSummary};
use crate::store::HitlInboxStore;

/// One authorized decision for [`HitlRouter::resolve`]: everything the
/// resolution carries besides the row key, which the router looks up itself.
///
/// Grouping these ends the run of same-typed positional parameters a caller
/// could silently transpose; the fields travel unchanged from `resolve`
/// through delivery into the kernel's `InteractionResolution`.
#[derive(Debug, Clone)]
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
/// attempted before the row is marked [`InteractionStatus::Delivered`], so a
/// failed delivery leaves the row `Open` and retryable.
pub struct HitlRouter {
    /// Adapter-owned inbox this router reads and settles.
    store: Arc<dyn HitlInboxStore>,
    /// Worker that durably buffers the resolution for a later tick.
    worker: Arc<WorkflowWorker>,
    /// Wake index, used by the expiry sweep.
    wake: Arc<dyn WakeIndexStore>,
    /// Authorization hook consulted before every resolution.
    authorizer: Arc<dyn ResolveAuthorizer>,
    /// Policy consulted for every interaction that crosses its deadline.
    expiry: Arc<dyn ExpiryPolicy>,
}

impl HitlRouter {
    /// Router over an inbox, a worker, and the worker's wake index, using the
    /// default [`TenantAuthorizer`] and [`ApprovalExpiry`].
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
            expiry: Arc::new(ApprovalExpiry),
        }
    }

    /// Replace the authorization hook.
    #[must_use]
    pub fn with_authorizer(mut self, authorizer: Arc<dyn ResolveAuthorizer>) -> Self {
        self.authorizer = authorizer;
        self
    }

    /// Replace the expiry policy consulted by [`HitlRouter::sweep`].
    ///
    /// A policy installed here decides **this router's row disposition** —
    /// its [`crate::InteractionStatus`] and `resolved_by` — and not what the
    /// waiting run receives.
    ///
    /// The default [`ApprovalExpiry`] declines everything, so *this sweep*
    /// delivers nothing until a host installs a policy. That does not keep
    /// unanswered interactions alive: past the deadline the worker's tick
    /// expires them through the kernel's own credential-free path, whether or
    /// not a policy exists.
    ///
    /// Nor does installing a policy substitute an authored refusal payload
    /// for that expiry. A sweep may only offer a row once `now >=
    /// expires_at`, the worker submits the buffered command with the same
    /// clock, and the interaction ingress rewrites any resolution submitted
    /// at or after the deadline into `InteractionSettled::Expired` before the
    /// reducer sees it — so the payload is discarded and the journal records
    /// a plain expiry with no resolution identity. See the
    /// [`crate::ApprovalExpiry`] documentation and its module for the full
    /// chain, and `tests/hitl/uc05.rs` for it asserted end to end.
    ///
    /// Whatever the policy returns is still delivered under the host's own
    /// responsibility: it must carry the principal and authorization evidence
    /// the run was accepted with, or the delivery is refused by the ingress
    /// for a second, independent reason.
    #[must_use]
    pub fn with_expiry_policy(mut self, policy: Arc<dyn ExpiryPolicy>) -> Self {
        self.expiry = policy;
        self
    }

    /// Open interactions for one tenant, oldest first (host inbox view).
    ///
    /// # Errors
    ///
    /// Returns store-unavailable or integrity failures from
    /// [`HitlInboxStore::load_open`].
    pub fn pending(&self, tenant_scope: &str) -> Result<Vec<InteractionRow>, HitlError> {
        self.store.load_open(tenant_scope)
    }

    /// Authorize a decision, deliver it durably, and settle the inbox row.
    ///
    /// Delivery happens before the status transition: if the worker rejects
    /// the command the row stays `Open` and the call can be retried.
    ///
    /// # `Delivered` means buffered, not accepted
    ///
    /// This router hands the command to the worker's inbox and marks the row
    /// [`crate::InteractionStatus::Delivered`]. Whether the *journal* accepts
    /// it is decided later, by the runtime's interaction ingress, which
    /// admits a resolution only when its principal **and** its authorization
    /// evidence exactly equal the run's own `RunAccepted` security context —
    /// the credentials the host presented when it accepted the run
    /// (`authorization_matches`,
    /// `crates/finstack-ai-runtime/src/driver/ingress/shared.rs`).
    ///
    /// Deliver a mismatched principal, policy version, or decision id and the
    /// failure is permanent and invisible from here: every tick rejects the
    /// buffered command as `scope_mismatch`, nothing is ever settled, the run
    /// stays parked on `RunPhase::AwaitingInteraction`, and the row sits at
    /// `Delivered` — out of [`HitlRouter::pending`], so out of the operator's
    /// view. The worker's retry backoff never gives up, so this does not
    /// self-heal. Pass the accepted run's own principal and evidence through;
    /// a host that cannot should leave the row `Open` rather than deliver.
    ///
    /// Note also that a resolution submitted at or after the request's
    /// `expires_at` is rewritten into an expiry by that same ingress, so a
    /// late answer settles the interaction without its payload.
    ///
    /// If the final [`HitlInboxStore::set_status`] fails after a successful
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
        if row.status != InteractionStatus::Open {
            return Err(HitlError::NotOpen);
        }
        let request: InteractionRequest =
            serde_json::from_slice(row.request.as_ref()).map_err(|_| {
                HitlError::StoreIntegrity {
                    code: "hitl_request_decode",
                }
            })?;
        self.authorizer
            .authorize(&row, &request, &input.principal)?;

        let subject = self.deliver(&row, &request, input, now)?;
        self.store.set_status(
            tenant_scope,
            interaction_id,
            InteractionStatus::Delivered,
            Some(subject.as_ref()),
            now,
        )
    }

    /// Reconcile the inbox against the wake index, then expire what is past
    /// its deadline.
    ///
    /// Reconcile runs first and wins: any active (`Open` or `Delivered`) row
    /// with no matching `Interaction` wake row was already settled out of
    /// band — a tick consumed it — so it is closed rather than refused, and
    /// nothing is delivered for it. Every remaining `Open` row whose
    /// `expires_at` has arrived is handed to the [`ExpiryPolicy`]; a
    /// resolution is delivered to the worker under the idempotent id
    /// `"hitl-expiry-<interaction_id>"` and the row becomes `Expired`, while
    /// a policy that declines leaves the row `Open` for the next sweep.
    ///
    /// The default [`ApprovalExpiry`] declines every row, so a sweep expires
    /// nothing until a host installs a policy — see
    /// [`HitlRouter::with_expiry_policy`]. `Expired` is a statement about
    /// *this row*, not about the run: it means "a host policy authored an
    /// expiry for this row". The payload that policy authored does not reach
    /// the run, because by the time a sweep may offer the row its deadline
    /// has passed and the interaction ingress rewrites the delivered
    /// resolution into a plain `InteractionSettled::Expired`. An interaction
    /// the worker's tick expired with no policy installed instead arrives
    /// here as a reconcile and is closed by the branch above. Either way the
    /// journal — not this inbox — is the authority on what settled the run.
    ///
    /// The first row that errors aborts the pass. Each transition is
    /// independently durable, so rows already transitioned stay that way and
    /// re-running the sweep is safe.
    ///
    /// Rows come from a snapshot, so each one is re-loaded and re-checked
    /// immediately before its expiry is delivered; that narrows but cannot
    /// eliminate the window against a concurrent [`HitlRouter::resolve`], and
    /// the journal's settlement — not this inbox — remains the once-only
    /// authority.
    ///
    /// # Errors
    ///
    /// Returns store failures from [`HitlInboxStore::load_active`],
    /// [`HitlInboxStore::load`], or
    /// [`HitlInboxStore::set_status`], [`HitlError::Worker`] when the wake
    /// index or delivery fails, [`HitlError::StoreIntegrity`] with code
    /// `"hitl_request_decode"` or `"hitl_locator"` for an undecodable row,
    /// [`HitlError::InvalidResolution`] when the kernel rejects the policy's
    /// resolution, and whatever the policy itself returns.
    pub fn sweep(&self, now: Timestamp) -> Result<SweepReport, HitlError> {
        let mut report = SweepReport::default();
        let mut wake_ids: BTreeMap<Arc<str>, BTreeSet<Arc<str>>> = BTreeMap::new();

        for row in self.store.load_active_summaries()? {
            if !wake_ids.contains_key(row.tenant_scope.as_ref()) {
                let pending = self
                    .wake
                    .load_tenant(row.tenant_scope.as_ref())?
                    .into_iter()
                    .filter(|wake| wake.reason == WakeReason::Interaction)
                    .map(|wake| wake.pending_id)
                    .collect();
                wake_ids.insert(Arc::clone(&row.tenant_scope), pending);
            }
            let awaited = wake_ids
                .get(row.tenant_scope.as_ref())
                .is_some_and(|pending| pending.contains(row.interaction_id.as_ref()));

            if !awaited {
                self.store.set_status(
                    row.tenant_scope.as_ref(),
                    row.interaction_id.as_ref(),
                    InteractionStatus::Closed,
                    row.resolved_by.as_deref(),
                    now,
                )?;
                report.reconciled += 1;
                continue;
            }
            if row.status != InteractionStatus::Open
                || row.expires_at.is_none_or(|deadline| deadline > now)
            {
                continue;
            }
            if self.expire_row(&row, now)? {
                report.expired += 1;
            }
        }
        Ok(report)
    }

    /// Deliver the expiry policy's resolution for one past-deadline row and
    /// mark it `Expired`. Returns `false` when the policy declines or when
    /// the row is no longer expirable.
    ///
    /// The summary arrives from [`HitlInboxStore::load_active_summaries`]'s
    /// snapshot, so the full row — request payload included — is loaded
    /// fresh here, once, immediately before the policy is consulted and the
    /// delivery happens: a `resolve` that landed since the snapshot must not
    /// have its command overwritten in the worker inbox by an expiry
    /// refusal. Anything that is no longer `Open` past its deadline is left
    /// exactly as found.
    fn expire_row(&self, summary: &InteractionSummary, now: Timestamp) -> Result<bool, HitlError> {
        let Some(fresh) = self.store.load(
            summary.tenant_scope.as_ref(),
            summary.interaction_id.as_ref(),
        )?
        else {
            return Ok(false);
        };
        if fresh.status != InteractionStatus::Open
            || fresh.expires_at.is_none_or(|deadline| deadline > now)
        {
            return Ok(false);
        }
        let request: InteractionRequest =
            serde_json::from_slice(fresh.request.as_ref()).map_err(|_| {
                HitlError::StoreIntegrity {
                    code: "hitl_request_decode",
                }
            })?;
        let Some(resolution) = self.expiry.expire(&fresh, &request)? else {
            return Ok(false);
        };
        let subject = self.deliver(
            &fresh,
            &request,
            ResolutionInput {
                resolution_id: Arc::from(format!("hitl-expiry-{}", fresh.interaction_id)),
                principal: resolution.principal,
                evidence: resolution.evidence,
                payload: resolution.payload,
                note: None,
            },
            now,
        )?;
        self.store.set_status(
            fresh.tenant_scope.as_ref(),
            fresh.interaction_id.as_ref(),
            InteractionStatus::Expired,
            Some(subject.as_ref()),
            now,
        )?;
        Ok(true)
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
