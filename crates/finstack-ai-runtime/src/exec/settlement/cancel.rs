use std::future::Future;
use std::sync::Arc;

use finstack_ai_kernel::{
    ActiveToolCallStatus, CancellationReconciledInput, EffectId, KernelInput, RetrySafety, RunPhase,
};

use crate::coordinator::CommitCoordinator;
use crate::ids::{Clock, RandomSource};
use crate::run_types::RunHandleError;

use super::SettlementSources;
use super::ids::submit_runtime_input;

enum IdleCancelClass {
    Cancelled,
    Completed,
    Uncertain,
}

pub(crate) async fn reconcile_cancelled_effect<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    effect_id: EffectId,
    cancelled: bool,
    sources: &SettlementSources<C, R>,
) -> Result<(), RunHandleError> {
    reconcile_classified_effect(
        coordinator,
        effect_id,
        if cancelled {
            IdleCancelClass::Cancelled
        } else {
            IdleCancelClass::Completed
        },
        sources,
    )
    .await
}

async fn reconcile_classified_effect<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    effect_id: EffectId,
    class: IdleCancelClass,
    sources: &SettlementSources<C, R>,
) -> Result<(), RunHandleError> {
    let request_id = coordinator
        .state()
        .cancellation()
        .ok_or(RunHandleError::CancellationSettlement {
            code: "cancellation_request_missing",
        })?
        .request
        .request_id;
    let (completed_effects, cancelled_effects, uncertain_effects) = match class {
        IdleCancelClass::Cancelled => (Arc::from([]), Arc::from([effect_id]), Arc::from([])),
        IdleCancelClass::Completed => (Arc::from([effect_id]), Arc::from([]), Arc::from([])),
        IdleCancelClass::Uncertain => (Arc::from([]), Arc::from([]), Arc::from([effect_id])),
    };
    reconcile_effect_sets(
        coordinator,
        sources,
        CancellationReconciledInput {
            request_id,
            completed_effects,
            cancelled_effects,
            uncertain_effects,
        },
    )
    .await
}

fn reconcile_effect_sets<'a, C: Clock, R: RandomSource>(
    coordinator: &'a mut CommitCoordinator,
    sources: &'a SettlementSources<C, R>,
    reconciliation: CancellationReconciledInput,
) -> impl Future<Output = Result<(), RunHandleError>> + 'a {
    submit_runtime_input(
        coordinator,
        KernelInput::CancellationReconciled(reconciliation),
        sources,
        |code| RunHandleError::CancellationSettlement { code },
        |_| RunHandleError::CancellationSettlement {
            code: "runtime_input_rejected",
        },
    )
}

pub(crate) async fn drain_idle_cancellation<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    sources: &SettlementSources<C, R>,
    force_all: bool,
) -> Result<(), RunHandleError> {
    loop {
        let Some(cancellation) = coordinator.state().cancellation() else {
            return Ok(());
        };
        if coordinator.state().terminal().is_some()
            || coordinator.state().phase() == Some(RunPhase::Suspended)
        {
            return Ok(());
        }
        if cancellation.outstanding_effects.is_empty() {
            reconcile_effect_sets(
                coordinator,
                sources,
                CancellationReconciledInput {
                    request_id: cancellation.request.request_id,
                    completed_effects: Arc::from([]),
                    cancelled_effects: Arc::from([]),
                    uncertain_effects: Arc::from([]),
                },
            )
            .await?;
            continue;
        }
        let Some(effect_id) = cancellation
            .outstanding_effects
            .iter()
            .copied()
            .find(|id| idle_cancellation_class(coordinator.state(), *id, force_all).is_some())
        else {
            return Ok(());
        };
        let class = idle_cancellation_class(coordinator.state(), effect_id, force_all).ok_or(
            RunHandleError::CancellationSettlement {
                code: "idle_cancellation_class_missing",
            },
        )?;
        reconcile_classified_effect(coordinator, effect_id, class, sources).await?;
    }
}

fn idle_cancellation_class(
    state: &finstack_ai_kernel::KernelState,
    effect_id: EffectId,
    force_all: bool,
) -> Option<IdleCancelClass> {
    if state
        .pending_interaction()
        .is_some_and(|pending| pending.request.effect_id() == effect_id)
    {
        return Some(IdleCancelClass::Cancelled);
    }
    if state
        .retry()
        .pending
        .as_ref()
        .is_some_and(|pending| pending.timer_effect_id == effect_id)
    {
        return Some(IdleCancelClass::Cancelled);
    }
    if let Some(pending) = state.pending_model_effect()
        && pending.requested.effect_id() == effect_id
    {
        return effect_idle_class(
            pending.requested.retry_safety(),
            pending.deferred.is_some(),
            force_all,
        );
    }
    if let Some(pending) = state.pending_extension_effect()
        && pending.requested.effect_id() == effect_id
    {
        // The inline invocation waiter drops a cancelled call only after the
        // cancellation commits, before reaching this idle classification.
        // Recovery also reaches this path without an active invocation.
        return effect_idle_class(pending.requested.retry_safety(), true, force_all);
    }
    state.active_tool_batch().and_then(|batch| {
        batch.calls.iter().find_map(|call| {
            if call.assigned.effect_id != effect_id {
                return None;
            }
            match &call.status {
                ActiveToolCallStatus::Requested {
                    requested,
                    deferred,
                } => effect_idle_class(requested.retry_safety(), deferred.is_some(), force_all),
                _ => None,
            }
        })
    })
}

fn effect_idle_class(
    safety: RetrySafety,
    deferred: bool,
    force_all: bool,
) -> Option<IdleCancelClass> {
    if !deferred && !force_all {
        return None;
    }
    Some(match safety {
        RetrySafety::AtMostOnce | RetrySafety::Unknown => IdleCancelClass::Uncertain,
        RetrySafety::SafeToRetry | RetrySafety::IdempotentWithKey => IdleCancelClass::Cancelled,
    })
}
