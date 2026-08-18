use finstack_ai_kernel::{
    ActiveToolCallStatus, EffectDeferred, EffectId, ExternalEffectCompletedInput,
    ExternalEffectCompletion, ExternalEffectOutcome, KernelInput, KernelState,
    ReconciliationPolicy, Timestamp,
};

use crate::coordinator::CommitCoordinator;
use crate::run_types::RunHandleError;
use crate::{
    CancellationSignal, Clock, PendingToolEffect, RandomSource, ReconcileContext,
    ResolvedToolCatalog, RunCallContext, TOOL_DEFERRAL_EXPIRED, TOOL_RECONCILIATION_UNSUPPORTED,
    ToolError, ToolResumeAction,
};

use super::ids::submit_resume_input;
use super::tool::{apply_tool_reconcile_result, deferred_tool_seed};
use super::{SettlementSources, tool_handle_error};

/// A committed deferred effect's next poll deadline.
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
pub(crate) fn due_polls(state: &KernelState, now: Timestamp) -> Vec<DuePoll> {
    let _ = now;
    state
        .active_tool_batch
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
pub(crate) fn expired(deferred: &EffectDeferred, now: Timestamp) -> bool {
    deferred
        .expires_at
        .is_some_and(|expires_at| expires_at <= now)
}

/// Reconcile due deferrals after failing all committed expirations.
pub(crate) async fn drive_due_polls<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    catalog: &ResolvedToolCatalog,
    sources: &SettlementSources<C, R>,
    cancellation: &CancellationSignal,
) -> Result<(), RunHandleError> {
    let now = sources.now()?;
    let expired_effects = coordinator
        .state()
        .active_tool_batch
        .as_ref()
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
        fail_expired_deferral(coordinator, effect_id, sources).await?;
    }

    let due_effects = due_polls(coordinator.state(), now)
        .into_iter()
        .filter(|due| due.at <= now)
        .map(|due| due.effect_id)
        .collect::<Vec<_>>();
    for effect_id in due_effects {
        reconcile_due_tool(coordinator, catalog, effect_id, sources, cancellation).await?;
    }
    Ok(())
}

/// Return the earliest process-local wake-up needed by committed deferrals.
pub(crate) fn next_due_poll_or_expiry(state: &KernelState) -> Option<Timestamp> {
    state
        .active_tool_batch
        .iter()
        .flat_map(|batch| batch.calls.iter())
        .filter_map(|call| match &call.status {
            ActiveToolCallStatus::Requested {
                deferred: Some(deferred),
                ..
            } => Some(deferred),
            _ => None,
        })
        .flat_map(|deferred| {
            let poll = matches!(
                deferred.reconciliation,
                ReconciliationPolicy::Poll | ReconciliationPolicy::CallbackOrPoll
            )
            .then_some(deferred.next_poll_at)
            .flatten();
            [poll, deferred.expires_at]
        })
        .flatten()
        .min()
}

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

async fn reconcile_due_tool<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    catalog: &ResolvedToolCatalog,
    effect_id: EffectId,
    sources: &SettlementSources<C, R>,
    cancellation: &CancellationSignal,
) -> Result<(), RunHandleError> {
    let seed =
        deferred_tool_seed(coordinator, effect_id).ok_or(RunHandleError::ToolSettlement {
            code: "tool_resume_seed_missing",
        })?;
    let resolved = catalog
        .by_id(&seed.call.tool_id)
        .ok_or_else(|| RunHandleError::Tool {
            code: TOOL_RECONCILIATION_UNSUPPORTED.into(),
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
                },
                original_input_digest: seed.requested.input_digest(),
            },
            PendingToolEffect {
                call: seed.call.clone(),
            },
        )
        .await
        .map_err(|_| RunHandleError::Tool {
            code: TOOL_RECONCILIATION_UNSUPPORTED.into(),
        })?;
    if apply_tool_reconcile_result(coordinator, &seed, &result, sources).await?
        == ToolResumeAction::SuspendUncertain
    {
        return Err(RunHandleError::Tool {
            code: TOOL_RECONCILIATION_UNSUPPORTED.into(),
        });
    }
    Ok(())
}
