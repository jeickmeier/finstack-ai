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
use crate::store::HitlInboxStore;

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

    /// Wake index this router shares with the worker.
    #[must_use]
    pub fn wake_index(&self) -> &Arc<dyn WakeIndexStore> {
        &self.wake
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
    #[allow(clippy::too_many_arguments)]
    pub fn resolve(
        &self,
        tenant_scope: &str,
        interaction_id: &str,
        resolution_id: &str,
        principal: PrincipalRef,
        evidence: AuthorizationEvidence,
        payload: RawJson,
        note: Option<&str>,
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
        self.authorizer.authorize(&row, &request, &principal)?;

        let locator = OperationLocator::try_new(
            row.tenant_scope.as_ref(),
            row.session_id,
            row.lane_id,
            row.run_id,
        )
        .map_err(|_| HitlError::StoreIntegrity {
            code: "hitl_locator",
        })?;
        let subject: Arc<str> = Arc::from(principal.subject());
        let resolution = InteractionResolution::try_new(
            request.interaction_id(),
            resolution_id,
            principal,
            evidence,
            payload,
            note,
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
        self.store.set_status(
            tenant_scope,
            interaction_id,
            InteractionStatus::Delivered,
            Some(subject.as_ref()),
            now,
        )
    }
}
