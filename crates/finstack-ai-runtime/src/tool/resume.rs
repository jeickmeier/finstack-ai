use finstack_ai_kernel::{
    ActiveToolCallStatus, EffectId, EffectRequested, KernelState, ReconciliationPolicy,
    RetrySafety, ToolCallPlan,
};

use crate::{SideEffectClass, ToolSpec};

use super::types::{ToolReconcileResult, ToolResumeAction};

/// Classify recovery for one tool effect from committed journal state only.
///
/// First-pass never returns [`ToolResumeAction::Retry`]. Unstarted, in-flight,
/// and completed-but-uncommitted journals are identical for one call
/// (`Requested` without deferral, no settlement) and classify as
/// [`ToolResumeAction::Reconcile`].
#[must_use]
pub fn tool_resume_action(state: &KernelState, effect_id: EffectId) -> ToolResumeAction {
    let Some(batch) = state.active_tool_batch.as_ref() else {
        return ToolResumeAction::NoOutstanding;
    };
    let Some(call) = batch
        .calls
        .iter()
        .find(|call| call.assigned.effect_id == effect_id)
    else {
        return ToolResumeAction::NoOutstanding;
    };
    if state.tool_settlements.contains_key(&effect_id)
        || matches!(
            call.status,
            ActiveToolCallStatus::Settled { .. } | ActiveToolCallStatus::Buffered { .. }
        )
        || matches!(call.assigned.plan, ToolCallPlan::SyntheticClosure(_))
    {
        return ToolResumeAction::UseRecorded;
    }
    match &call.status {
        ActiveToolCallStatus::Undispatched => ToolResumeAction::NoOutstanding,
        ActiveToolCallStatus::Requested { deferred: None, .. } => ToolResumeAction::Reconcile,
        ActiveToolCallStatus::Requested {
            deferred: Some(deferred),
            ..
        } => match deferred.reconciliation {
            ReconciliationPolicy::CallbackOnly | ReconciliationPolicy::ExternalWorkflow => {
                ToolResumeAction::WaitExternal
            }
            ReconciliationPolicy::Poll | ReconciliationPolicy::CallbackOrPoll => {
                ToolResumeAction::Reconcile
            }
        },
        ActiveToolCallStatus::Buffered { .. } | ActiveToolCallStatus::Settled { .. } => {
            ToolResumeAction::UseRecorded
        }
    }
}

/// Whether the committed request plus tool side-effect class allow a same-identity retry.
#[must_use]
pub fn tool_retry_allowed(requested: &EffectRequested, spec: &ToolSpec) -> bool {
    matches!(
        requested.retry_safety(),
        RetrySafety::SafeToRetry | RetrySafety::IdempotentWithKey
    ) && matches!(
        spec.side_effect,
        SideEffectClass::ReadOnly | SideEffectClass::IdempotentWrite
    )
}

/// Map one tool reconcile result onto the documented post-reconcile action.
///
/// `awaiting_external` is that call's `deferred.is_some()`, not the run phase.
#[must_use]
pub fn map_tool_reconcile_result(
    state: &KernelState,
    effect_id: EffectId,
    result: &ToolReconcileResult,
    retry_allowed: bool,
) -> ToolResumeAction {
    match tool_resume_action(state, effect_id) {
        recorded @ (ToolResumeAction::NoOutstanding | ToolResumeAction::UseRecorded) => {
            return recorded;
        }
        ToolResumeAction::Reconcile
        | ToolResumeAction::Retry
        | ToolResumeAction::WaitExternal
        | ToolResumeAction::SuspendUncertain => {}
    }
    let awaiting_external = state.active_tool_batch.as_ref().is_some_and(|batch| {
        batch.calls.iter().any(|call| {
            call.assigned.effect_id == effect_id
                && matches!(
                    call.status,
                    ActiveToolCallStatus::Requested {
                        deferred: Some(_),
                        ..
                    }
                )
        })
    });
    match result {
        ToolReconcileResult::Completed(_) => ToolResumeAction::UseRecorded,
        ToolReconcileResult::Deferred(_) | ToolReconcileResult::StillRunning(_) => {
            ToolResumeAction::WaitExternal
        }
        ToolReconcileResult::NonRepeatable => ToolResumeAction::SuspendUncertain,
        ToolReconcileResult::NotStarted | ToolReconcileResult::RetrySafe => {
            if awaiting_external {
                ToolResumeAction::SuspendUncertain
            } else {
                ToolResumeAction::Retry
            }
        }
        ToolReconcileResult::Unknown => {
            if awaiting_external || !retry_allowed {
                ToolResumeAction::SuspendUncertain
            } else {
                ToolResumeAction::Retry
            }
        }
    }
}
