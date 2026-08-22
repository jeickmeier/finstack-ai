#[cfg(feature = "native-tokio")]
use std::collections::BTreeMap;

#[cfg(any(feature = "native-tokio", test))]
use finstack_ai_kernel::{
    ActiveToolCallStatus, EffectDeferred, EffectId, KernelState, ReconciliationPolicy, Timestamp,
};
#[cfg(feature = "native-tokio")]
use finstack_ai_kernel::{
    ExternalEffectCompletedInput, ExternalEffectCompletion, ExternalEffectOutcome, KernelInput,
};

#[cfg(feature = "native-tokio")]
use crate::coordinator::CommitCoordinator;
#[cfg(feature = "native-tokio")]
use crate::ids::{Clock, RandomSource};
use crate::ports::model::{CancellationSignal, ReconcileContext, RunCallContext};
use crate::ports::tool::{
    PendingToolEffect, ResolvedToolCatalog, TOOL_DEFERRAL_EXPIRED, TOOL_RECONCILIATION_UNSUPPORTED,
    ToolError, ToolReconcileResult, ToolResumeAction, map_tool_reconcile_result,
    tool_retry_allowed,
};
#[cfg(feature = "native-tokio")]
use crate::run_types::RunHandleError;

#[cfg(feature = "native-tokio")]
use super::ids::submit_resume_input;
#[cfg(feature = "native-tokio")]
use super::tool::{apply_tool_reconcile_result, deferred_tool_seed};
#[cfg(feature = "native-tokio")]
use super::{SettlementSources, tool_handle_error};

/// A committed deferred effect's next poll deadline.
#[cfg(any(feature = "native-tokio", test))]
pub(crate) struct DuePoll {
    /// Deferred effect identity.
    pub effect_id: EffectId,
    /// Semantic time at which the effect becomes due for polling.
    pub at: Timestamp,
}

/// Return all committed pollable deferrals that have a next poll deadline.
///
/// Membership ignores `now`; the driver is responsible for filtering deadlines
/// whose `at` is less than or equal to `now`.
#[cfg(any(feature = "native-tokio", test))]
pub(crate) fn due_polls(state: &KernelState, now: Timestamp) -> Vec<DuePoll> {
    let _ = now;
    state
        .active_tool_batch()
        .iter()
        .flat_map(|batch| batch.calls.iter())
        .filter_map(|call| {
            let ActiveToolCallStatus::Requested {
                deferred: Some(deferred),
                ..
            } = &call.status
            else {
                return None;
            };
            if !matches!(
                deferred.reconciliation,
                ReconciliationPolicy::Poll | ReconciliationPolicy::CallbackOrPoll
            ) {
                return None;
            }
            deferred.next_poll_at.map(|at| DuePoll {
                effect_id: deferred.effect_id,
                at,
            })
        })
        .collect()
}

/// Return whether a deferred effect has reached its inclusive expiry.
#[cfg(any(feature = "native-tokio", test))]
pub(crate) fn expired(deferred: &EffectDeferred, now: Timestamp) -> bool {
    deferred
        .expires_at
        .is_some_and(|expires_at| expires_at <= now)
}

/// Reconcile due deferrals after failing all committed expirations.
///
/// Re-arms each effect's future process-local deadline received from a live
/// reconciliation. Those deadlines are not committed.
///
/// # Errors
///
/// Returns a stable tool error when expiry settlement, reconciliation, or
/// uncertainty handling fails.
#[cfg(feature = "native-tokio")]
pub(crate) async fn drive_due_polls<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    catalog: &ResolvedToolCatalog,
    sources: &SettlementSources<C, R>,
    cancellation: &CancellationSignal,
    process_local_deadlines: &mut BTreeMap<EffectId, Option<Timestamp>>,
) -> Result<(), RunHandleError> {
    let now = sources.now()?;
    let expired_effects = coordinator
        .state()
        .active_tool_batch()
        .map(|batch| {
            batch
                .calls
                .iter()
                .filter_map(|call| match &call.status {
                    ActiveToolCallStatus::Requested {
                        deferred: Some(deferred),
                        ..
                    } if expired(deferred, now) => Some(call.assigned.effect_id),
                    _ => None,
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    for effect_id in expired_effects {
        process_local_deadlines.remove(&effect_id);
        fail_expired_deferral(coordinator, effect_id, sources).await?;
    }

    let due_effects = due_polls(coordinator.state(), now)
        .into_iter()
        .filter(|due| {
            due.at <= now
                && process_local_deadlines
                    .get(&due.effect_id)
                    .is_none_or(|deadline| deadline.is_some_and(|deadline| deadline <= now))
        })
        .map(|due| due.effect_id)
        .collect::<Vec<_>>();
    for effect_id in due_effects {
        if let Some(next_poll_at) =
            reconcile_due_tool(coordinator, catalog, effect_id, now, sources, cancellation).await?
        {
            process_local_deadlines.insert(effect_id, next_poll_at);
        } else {
            process_local_deadlines.remove(&effect_id);
        }
    }
    Ok(())
}

/// Return the earliest wake-up needed by committed and local deferral state.
#[cfg(feature = "native-tokio")]
pub(crate) fn next_due_poll_or_expiry(
    state: &KernelState,
    process_local_deadlines: &BTreeMap<EffectId, Option<Timestamp>>,
) -> Option<Timestamp> {
    state
        .active_tool_batch()
        .iter()
        .flat_map(|batch| batch.calls.iter())
        .flat_map(|call| match &call.status {
            ActiveToolCallStatus::Requested {
                deferred: Some(deferred),
                ..
            } => {
                let local = process_local_deadlines.get(&deferred.effect_id);
                let poll = matches!(
                    deferred.reconciliation,
                    ReconciliationPolicy::Poll | ReconciliationPolicy::CallbackOrPoll
                )
                .then_some(deferred.next_poll_at)
                .flatten()
                .filter(|deadline| {
                    local.is_none_or(|local_deadline| {
                        local_deadline.is_some_and(|local_deadline| local_deadline <= *deadline)
                    })
                });
                [poll, deferred.expires_at, local.copied().flatten()]
            }
            _ => [None, None, None],
        })
        .flatten()
        .min()
}

#[cfg(feature = "native-tokio")]
async fn fail_expired_deferral<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    effect_id: EffectId,
    sources: &SettlementSources<C, R>,
) -> Result<(), RunHandleError> {
    let error = ToolError::stable(
        TOOL_DEFERRAL_EXPIRED,
        "deferred tool effect expired before completion",
    )
    .to_descriptor()
    .map_err(|error| tool_handle_error(&error))?;
    let completion_id = effect_id.to_canonical_string();
    let completion = ExternalEffectCompletion::try_new(
        effect_id,
        &completion_id,
        ExternalEffectOutcome::Failed { error },
    )
    .map_err(|_| RunHandleError::ToolSettlement {
        code: "tool_external_completion_invalid",
    })?;
    submit_resume_input(
        coordinator,
        KernelInput::ExternalEffectCompleted(ExternalEffectCompletedInput {
            completion,
            assistant_message: None,
        }),
        sources,
    )
    .await
}

#[cfg(feature = "native-tokio")]
async fn reconcile_due_tool<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    catalog: &ResolvedToolCatalog,
    effect_id: EffectId,
    now: Timestamp,
    sources: &SettlementSources<C, R>,
    cancellation: &CancellationSignal,
) -> Result<Option<Option<Timestamp>>, RunHandleError> {
    let seed =
        deferred_tool_seed(coordinator, effect_id).ok_or(RunHandleError::ToolSettlement {
            code: "tool_resume_seed_missing",
        })?;
    let resolved = catalog
        .by_id(&seed.call.tool_id)
        .ok_or(RunHandleError::Faulted {
            code: TOOL_RECONCILIATION_UNSUPPORTED,
        })?;
    let result = resolved
        .toolset
        .reconcile(
            ReconcileContext {
                run: RunCallContext {
                    locator: seed.locator.clone(),
                    authorization: seed.authorization.clone(),
                    effect_id,
                    attempt: seed.attempt,
                    deadline: seed.requested.deadline(),
                    budget_scope_id: seed.budget_scope_id,
                    cancellation: cancellation.child(),
                    relation_depth: coordinator.accepted_relation_depth(),
                },
                original_input_digest: seed.requested.input_digest(),
            },
            PendingToolEffect {
                call: seed.call.clone(),
            },
        )
        .await
        .map_err(|error| tool_handle_error(&error))?;
    let action = map_tool_reconcile_result(
        coordinator.state(),
        effect_id,
        &result,
        tool_retry_allowed(&seed.requested, &resolved.spec),
    );
    if action == ToolResumeAction::SuspendUncertain {
        return Err(RunHandleError::Faulted {
            code: TOOL_RECONCILIATION_UNSUPPORTED,
        });
    }
    if apply_tool_reconcile_result(coordinator, &seed, &result, sources).await?
        == ToolResumeAction::SuspendUncertain
    {
        return Err(RunHandleError::Faulted {
            code: TOOL_RECONCILIATION_UNSUPPORTED,
        });
    }
    Ok(match result {
        ToolReconcileResult::Deferred(deferral) | ToolReconcileResult::StillRunning(deferral) => {
            Some(
                deferral
                    .next_poll_at
                    .filter(|next_poll_at| *next_poll_at > now),
            )
        }
        ToolReconcileResult::Completed(_)
        | ToolReconcileResult::NotStarted
        | ToolReconcileResult::RetrySafe
        | ToolReconcileResult::Unknown
        | ToolReconcileResult::NonRepeatable => None,
    })
}
