//! Park a session on its classified wait and index it for the worker.

use std::sync::Arc;

use finstack_ai_runtime::workflow::{
    WorkflowCheckpoint, WorkflowSession, WorkflowWait, classify_wait,
};

use crate::error::WorkerError;
use crate::wake::{WakeIndexStore, WakeLease, WakeReason, WakeRow};

/// Classify the current wait, index it for the worker, and drop the owner.
///
/// Distinct from `finstack_ai_workflow_hitl::park_for_interaction`, which
/// parks on a pending human interaction and also records to an inbox.
///
/// The row is a hint: the journal stays authoritative. A terminal state
/// deletes any existing row instead of writing one.
///
/// An [`WorkflowWait::Interaction`] row also records the committed request's
/// `expires_at` on [`WakeRow::expires_at`], which is what lets the tick drive
/// the kernel's credential-free expiry once that deadline passes without any
/// resolution arriving. Re-parking after the expiry settles rewrites (or,
/// on a terminal state, deletes) this row, and the rewritten row no longer
/// classifies as the same interaction wait — so a later tick cannot expire
/// the same interaction twice.
///
/// # Errors
///
/// Returns [`WorkerError::NotParked`] when no wait is classified, and
/// store or handoff failures otherwise.
pub fn park_for_wake(
    session: &mut WorkflowSession,
    wake: &dyn WakeIndexStore,
    workflow_kind: &str,
) -> Result<WorkflowCheckpoint, WorkerError> {
    let wait = classify_wait(session.last_state()).ok_or(WorkerError::NotParked)?;
    let checkpoint = session.persist_handoff()?;
    index_checkpoint(wake, &checkpoint, &wait, workflow_kind, None)?;
    session.abort_owner();
    Ok(checkpoint)
}

pub(crate) fn index_checkpoint(
    wake: &dyn WakeIndexStore,
    checkpoint: &WorkflowCheckpoint,
    wait: &WorkflowWait,
    workflow_kind: &str,
    lease: Option<&WakeLease>,
) -> Result<(), WorkerError> {
    match wait {
        WorkflowWait::Terminal { .. } => {
            wake.delete(
                checkpoint.tenant_scope.as_ref(),
                checkpoint.session_id,
                lease,
            )?;
        }
        WorkflowWait::Timer { effect_id, due_at } => {
            wake.upsert(
                &row(
                    checkpoint,
                    workflow_kind,
                    WakeReason::Timer,
                    Some(*due_at),
                    None,
                    &effect_id.to_canonical_string(),
                ),
                lease,
            )?;
        }
        WorkflowWait::Interaction {
            interaction_id,
            request,
        } => {
            wake.upsert(
                &row(
                    checkpoint,
                    workflow_kind,
                    WakeReason::Interaction,
                    None,
                    request.expires_at(),
                    &interaction_id.to_canonical_string(),
                ),
                lease,
            )?;
        }
        WorkflowWait::DeferredEffect { effect_id, .. } => {
            wake.upsert(
                &row(
                    checkpoint,
                    workflow_kind,
                    WakeReason::Deferred,
                    None,
                    None,
                    &effect_id.to_canonical_string(),
                ),
                lease,
            )?;
        }
    }
    Ok(())
}

/// Build the wake-index row for a non-terminal wait.
fn row(
    checkpoint: &WorkflowCheckpoint,
    workflow_kind: &str,
    reason: WakeReason,
    wake_at: Option<finstack_ai_kernel::Timestamp>,
    expires_at: Option<finstack_ai_kernel::Timestamp>,
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
        expires_at,
        pending_id: Arc::from(pending_id),
        leased_by: None,
        lease_id: None,
        lease_expires_at: None,
        attempts: 0,
    }
}
