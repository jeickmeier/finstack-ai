//! Worker-to-HITL lifecycle bridge.

use std::sync::Arc;

use finstack_ai_kernel::{InteractionRequest, RunSecurityContext, Timestamp};
use finstack_ai_runtime::{WorkflowCheckpoint, WorkflowWait};
use finstack_ai_workflow_worker::{InteractionDeliveryOutcome, InteractionLifecycle, WorkerError};

use crate::capture::capture;
use crate::error::HitlError;
use crate::row::InteractionStatus;
use crate::store::{HitlInboxStore, InteractionTransition};

/// HITL lifecycle adapter registered on [`finstack_ai_workflow_worker::WorkerBuilder`].
pub struct HitlLifecycle {
    store: Arc<dyn HitlInboxStore>,
}

impl HitlLifecycle {
    /// Bridge worker callbacks into `store`.
    #[must_use]
    pub fn new(store: Arc<dyn HitlInboxStore>) -> Self {
        Self { store }
    }
}

impl InteractionLifecycle for HitlLifecycle {
    fn capture(
        &self,
        checkpoint: &WorkflowCheckpoint,
        request: &InteractionRequest,
        security: &RunSecurityContext,
        captured_at: Timestamp,
    ) -> Result<(), WorkerError> {
        let wait = WorkflowWait::Interaction {
            interaction_id: request.interaction_id(),
            request: request.clone(),
        };
        capture(
            self.store.as_ref(),
            checkpoint,
            &wait,
            security,
            captured_at,
        )
        .map(|_| ())
        .map_err(|error| worker_error(&error))
    }

    fn settled(
        &self,
        tenant_scope: &str,
        interaction_id: &str,
        outcome: InteractionDeliveryOutcome,
        settled_at: Timestamp,
    ) -> Result<(), WorkerError> {
        let (status, code) = match outcome {
            InteractionDeliveryOutcome::Accepted => (InteractionStatus::Accepted, "accepted"),
            InteractionDeliveryOutcome::Rejected { reason_code } => {
                (InteractionStatus::Rejected, reason_code)
            }
        };
        let changed = self
            .store
            .transition(
                tenant_scope,
                interaction_id,
                InteractionTransition {
                    expected: InteractionStatus::Buffered,
                    next: status,
                    resolved_by: None,
                    outcome_code: Some(code),
                    updated_at: settled_at,
                },
            )
            .map_err(|error| worker_error(&error))?;
        if changed {
            return Ok(());
        }
        let row = self
            .store
            .load(tenant_scope, interaction_id)
            .map_err(|error| worker_error(&error))?
            .ok_or(WorkerError::StoreIntegrity {
                code: "hitl_lifecycle_missing",
            })?;
        if row.status == status && row.outcome_code.as_deref() == Some(code) {
            Ok(())
        } else {
            Err(WorkerError::Conflict {
                code: "hitl_lifecycle_conflict",
            })
        }
    }
}

fn worker_error(error: &HitlError) -> WorkerError {
    match error {
        HitlError::StoreUnavailable { code } => WorkerError::StoreUnavailable { code },
        HitlError::StoreIntegrity { code } => WorkerError::StoreIntegrity { code },
        _ => WorkerError::StoreIntegrity {
            code: "hitl_lifecycle",
        },
    }
}
