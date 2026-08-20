//! Capture interaction parks into the HITL inbox.
//!
//! [`capture`] is the pure store write; [`park`] composes the worker's park
//! with it so a single call indexes the wake row and the inbox row from the
//! same classified wait.

use std::sync::Arc;

use finstack_ai_kernel::{InteractionKind, Timestamp};
use finstack_ai_runtime::{WorkflowCheckpoint, WorkflowSession, WorkflowWait, classify_wait};
use finstack_ai_workflow_worker::{WakeIndexStore, park as worker_park};

use crate::error::HitlError;
use crate::row::{InteractionRow, InteractionStatus};
use crate::store::HitlInboxStore;

/// Stable filter token for an interaction kind, matching the `kind` tag
/// `InteractionKind` serializes with.
fn kind_token(kind: &InteractionKind) -> Arc<str> {
    match kind {
        InteractionKind::Approval => Arc::from("approval"),
        InteractionKind::Choice => Arc::from("choice"),
        InteractionKind::Form => Arc::from("form"),
        InteractionKind::FreeText => Arc::from("free_text"),
        InteractionKind::Review => Arc::from("review"),
        InteractionKind::Correction => Arc::from("correction"),
        InteractionKind::Custom { name } => Arc::clone(name),
    }
}

/// Record an interaction wait into the HITL inbox.
///
/// Returns `true` when a row was written — that is, when `wait` is
/// [`WorkflowWait::Interaction`] — and `false` for every other wait, which
/// the HITL battery does not own. Coordinates come from `checkpoint`;
/// `interaction_id`, `kind`, and `expires_at` come from the committed
/// request; `request` holds its `serde_json` bytes. The row is written
/// `Open` with `requested_at == updated_at == now`, and re-capturing the
/// same interaction upserts the same key.
///
/// Re-capture preserves a still-meaningful settlement: an existing
/// `Delivered` or `Expired` row keeps its status and `resolved_by`, because
/// a worker restart re-parks an unresolved session on the same interaction
/// and must not revive a row whose decision is already buffered. A `Closed`
/// row is **not** preserved: `capture` only runs for an interaction the
/// journal still holds pending, so a `Closed` row here is stale by
/// definition — the usual cause is a [`crate::HitlRouter::sweep`] that
/// reconciled the row before its wake row was indexed — and it is reset to
/// `Open` instead of being pinned closed forever. [`park`] remains the
/// recommended entry point: it indexes the wake row (via
/// [`finstack_ai_workflow_worker::park`]) before capturing, which keeps a
/// racing sweep from closing the row in the first place.
///
/// # Errors
///
/// Returns [`HitlError::StoreIntegrity`] with code `"hitl_request_encode"`
/// when the request envelope fails to serialize, and store failures from
/// [`HitlInboxStore::load`] or [`HitlInboxStore::upsert`].
pub fn capture(
    store: &dyn HitlInboxStore,
    checkpoint: &WorkflowCheckpoint,
    wait: &WorkflowWait,
    now: Timestamp,
) -> Result<bool, HitlError> {
    let WorkflowWait::Interaction { request, .. } = wait else {
        return Ok(false);
    };
    let encoded = serde_json::to_vec(request).map_err(|_| HitlError::StoreIntegrity {
        code: "hitl_request_encode",
    })?;
    let interaction_id: Arc<str> = Arc::from(request.interaction_id().to_canonical_string());
    let settled = store
        .load(checkpoint.tenant_scope.as_ref(), interaction_id.as_ref())?
        .filter(|existing| {
            matches!(
                existing.status,
                InteractionStatus::Delivered | InteractionStatus::Expired
            )
        });
    let (status, resolved_by) = settled.map_or((InteractionStatus::Open, None), |existing| {
        (existing.status, existing.resolved_by)
    });
    store.upsert(&InteractionRow {
        tenant_scope: Arc::clone(&checkpoint.tenant_scope),
        session_id: checkpoint.session_id,
        lane_id: checkpoint.lane_id,
        run_id: checkpoint.run_id,
        interaction_id,
        kind: kind_token(request.kind()),
        requested_at: now,
        expires_at: request.expires_at(),
        request: Arc::from(encoded),
        status,
        resolved_by,
        updated_at: now,
    })?;
    Ok(true)
}

/// Park a session for the worker and capture any interaction it parked on.
///
/// Classifies the wait before delegating, because
/// [`finstack_ai_workflow_worker::park`] consumes the session's state and
/// aborts its owner. Non-interaction waits are indexed by the worker and
/// left out of the inbox.
///
/// # Errors
///
/// Returns [`HitlError::Worker`] when the session is not parked or the wake
/// index rejects the row, and [`capture`]'s errors otherwise.
pub fn park(
    session: &mut WorkflowSession,
    wake: &dyn WakeIndexStore,
    inbox: &dyn HitlInboxStore,
    workflow_kind: &str,
    now: Timestamp,
) -> Result<WorkflowCheckpoint, HitlError> {
    let wait = classify_wait(session.last_state());
    let checkpoint = worker_park(session, wake, workflow_kind)?;
    if let Some(wait) = wait {
        capture(inbox, &checkpoint, &wait, now)?;
    }
    Ok(checkpoint)
}
