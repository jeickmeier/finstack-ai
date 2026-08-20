//! Park a session on its classified wait and index it for the worker.

use std::sync::Arc;

use finstack_ai_runtime::{WorkflowCheckpoint, WorkflowSession, WorkflowWait, classify_wait};

use crate::error::WorkerError;
use crate::wake::{WakeIndexStore, WakeReason, WakeRow};

/// Classify the current wait, index it for the worker, and drop the owner.
///
/// The row is a hint: the journal stays authoritative. A terminal state
/// deletes any existing row instead of writing one.
///
/// # Errors
///
/// Returns [`WorkerError::NotParked`] when no wait is classified, and
/// store or handoff failures otherwise.
pub fn park(
    session: &mut WorkflowSession,
    wake: &dyn WakeIndexStore,
    workflow_kind: &str,
) -> Result<WorkflowCheckpoint, WorkerError> {
    let wait = classify_wait(session.last_state()).ok_or(WorkerError::NotParked)?;
    let checkpoint = session.persist_handoff()?;
    match &wait {
        WorkflowWait::Terminal { .. } => {
            wake.delete(checkpoint.tenant_scope.as_ref(), checkpoint.session_id)?;
        }
        WorkflowWait::Timer { effect_id, due_at } => {
            wake.upsert(&row(
                &checkpoint,
                workflow_kind,
                WakeReason::Timer,
                Some(*due_at),
                &effect_id.to_canonical_string(),
            ))?;
        }
        WorkflowWait::Interaction { interaction_id, .. } => {
            wake.upsert(&row(
                &checkpoint,
                workflow_kind,
                WakeReason::Interaction,
                None,
                &interaction_id.to_canonical_string(),
            ))?;
        }
        WorkflowWait::DeferredEffect { effect_id, .. } => {
            wake.upsert(&row(
                &checkpoint,
                workflow_kind,
                WakeReason::Deferred,
                None,
                &effect_id.to_canonical_string(),
            ))?;
        }
    }
    session.abort_owner();
    Ok(checkpoint)
}

/// Build the wake-index row for a non-terminal wait.
fn row(
    checkpoint: &WorkflowCheckpoint,
    workflow_kind: &str,
    reason: WakeReason,
    wake_at: Option<finstack_ai_kernel::Timestamp>,
    pending_id: &str,
) -> WakeRow {
    WakeRow {
        tenant_scope: Arc::clone(&checkpoint.tenant_scope),
        session_id: checkpoint.session_id,
        lane_id: checkpoint.lane_id,
        run_id: checkpoint.run_id,
        workflow_kind: Arc::from(workflow_kind),
        reason,
        wake_at,
        pending_id: Arc::from(pending_id),
        leased_by: None,
        lease_expires_at: None,
        attempts: 0,
    }
}
