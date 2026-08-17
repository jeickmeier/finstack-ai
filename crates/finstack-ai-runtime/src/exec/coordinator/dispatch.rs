use std::sync::Arc;

use crate::{AuthorizationContext, PortFuture, PortObject};

use finstack_ai_kernel::{
    ActiveToolCallStatus, CommittedBatch, EffectId, EffectRequested, KernelInput, KernelState,
    Metadata, OperationLocator, PendingModelEffect, PostCommitAction, RecordBody, Timestamp,
    ToolBatchId, ToolCallId, ValidatedToolCall,
};

use super::CommitCoordinator;

impl CommitCoordinator {
    #[cfg(feature = "native-tokio")]
    pub(crate) fn pending_timer_seed(&self) -> Option<TimerDispatchSeed> {
        Some(TimerDispatchSeed {
            scheduled: self.kernel.state().retry.pending.clone()?,
            scheduled_at: self.pending_timer_scheduled_at?,
        })
    }

    #[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
    pub(crate) fn pending_model_seed(&self) -> Option<ModelDispatchSeed> {
        let state = self.kernel.state();
        let pending = state.pending_model_effect.clone()?;
        let (locator, authorization, budget_scope_id) = dispatch_security_context(state)?;
        Some(ModelDispatchSeed {
            pending,
            locator,
            authorization,
            budget_scope_id,
            attempt: state.retry.attempts.checked_add(1)?,
            requested_at: state.accepted_at?,
            continuation_state: self.last_model_continuation.clone(),
        })
    }

    #[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
    pub(crate) fn pending_tool_seeds(&self) -> Vec<ToolDispatchSeed> {
        let state = self.kernel.state();
        let Some(batch) = state.active_tool_batch.as_ref() else {
            return Vec::new();
        };
        let Some((locator, authorization, budget_scope_id)) = dispatch_security_context(state)
        else {
            return Vec::new();
        };
        let Some(requested_at) = state.accepted_at else {
            return Vec::new();
        };
        batch
            .calls
            .iter()
            .filter_map(|call| {
                let ActiveToolCallStatus::Requested {
                    requested,
                    deferred: None,
                } = &call.status
                else {
                    return None;
                };
                let finstack_ai_kernel::ToolCallPlan::Execute(validated) = &call.assigned.plan
                else {
                    return None;
                };
                Some(ToolDispatchSeed {
                    requested: requested.clone(),
                    tool_batch_id: batch.opened.tool_batch_id,
                    tool_call_id: *validated.call.tool_call_id(),
                    call: validated.clone(),
                    locator: locator.clone(),
                    authorization: authorization.clone(),
                    budget_scope_id,
                    attempt: 1,
                    requested_at,
                })
            })
            .collect()
    }

    /// Identity, authorization, budget scope, attempt, and deadline for a
    /// stage-driver invocation at the coordinator's current cursor.
    ///
    /// Exact peer of [`Self::pending_model_seed`] / [`Self::pending_tool_seeds`],
    /// built from the same private [`dispatch_security_context`]. Exists
    /// because that free fn is private to this module and the stage-driver
    /// choke point (a later task, outside this module) cannot call it
    /// directly. `None` before a run is accepted, matching the other seeds.
    #[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
    #[allow(
        dead_code,
        reason = "consumed by the stage-settlement choke point wired in a later task"
    )]
    pub(crate) fn stage_dispatch_seed(&self) -> Option<StageDispatchSeed> {
        let state = self.kernel.state();
        let (locator, authorization, budget_scope_id) = dispatch_security_context(state)?;
        Some(StageDispatchSeed {
            locator,
            authorization,
            budget_scope_id,
            attempt: state.retry.attempts.checked_add(1)?,
            deadline: state.accepted.as_ref()?.effective_deadline(),
        })
    }
}

pub(super) fn action_is_authorized(
    state: &KernelState,
    action: PostCommitAction,
    now: Timestamp,
    committed: &CommittedBatch,
) -> bool {
    if state.terminal.is_some() {
        return false;
    }
    let effect_id = match action {
        PostCommitAction::ExecuteEffect { effect_id }
        | PostCommitAction::CancelEffect { effect_id } => effect_id,
    };
    match action {
        PostCommitAction::ExecuteEffect { .. } => {
            if state.cancellation.is_some() {
                return false;
            }
            committed_effect_request(committed, effect_id)
                .or_else(|| pending_effect_request(state, effect_id))
                .is_some_and(|request| request.deadline().is_none_or(|deadline| deadline > now))
        }
        PostCommitAction::CancelEffect { .. } => state
            .cancellation
            .as_ref()
            .is_some_and(|value| value.outstanding_effects.contains(&effect_id)),
    }
}

fn committed_effect_request(
    committed: &CommittedBatch,
    effect_id: EffectId,
) -> Option<&finstack_ai_kernel::EffectRequested> {
    committed.records.iter().find_map(|record| {
        let RecordBody::EffectRequested(request) = record.body() else {
            return None;
        };
        (request.effect_id() == effect_id).then_some(request)
    })
}

fn pending_effect_request(
    state: &KernelState,
    effect_id: EffectId,
) -> Option<&finstack_ai_kernel::EffectRequested> {
    if let Some(pending) = &state.pending_model_effect
        && pending.requested.effect_id() == effect_id
        && pending.deferred.is_none()
    {
        return Some(&pending.requested);
    }
    state.active_tool_batch.as_ref().and_then(|batch| {
        batch.calls.iter().find_map(|call| {
            let finstack_ai_kernel::ActiveToolCallStatus::Requested {
                requested,
                deferred,
            } = &call.status
            else {
                return None;
            };
            (requested.effect_id() == effect_id && deferred.is_none()).then_some(requested)
        })
    })
}

#[derive(Debug)]
pub(crate) struct DispatchError {
    pub(crate) code: &'static str,
}

/// Look up a registered cancellation signal.
///
/// A poisoned registry is a dispatch error. Missing `effect_id` after a
/// successful lock is an idempotent no-op for the caller.
#[cfg(any(feature = "native-tokio", feature = "wasm-host", test))]
pub(crate) fn cancel_registered_effect(
    active: &std::sync::Mutex<std::collections::BTreeMap<EffectId, crate::CancellationSignal>>,
    effect_id: EffectId,
    unavailable: &'static str,
) -> Result<Option<crate::CancellationSignal>, DispatchError> {
    let guard = active
        .lock()
        .map_err(|_| DispatchError { code: unavailable })?;
    Ok(guard.get(&effect_id).cloned())
}

#[derive(Debug, Clone)]
#[cfg_attr(not(feature = "native-tokio"), allow(dead_code))]
pub(crate) struct ModelDispatchSeed {
    pub(crate) pending: PendingModelEffect,
    pub(crate) locator: OperationLocator,
    pub(crate) authorization: AuthorizationContext,
    pub(crate) budget_scope_id: Option<finstack_ai_kernel::BudgetScopeId>,
    pub(crate) attempt: u32,
    pub(crate) requested_at: Timestamp,
    pub(crate) continuation_state: Option<finstack_ai_kernel::RawJson>,
}

#[derive(Debug, Clone)]
#[cfg_attr(not(feature = "native-tokio"), allow(dead_code))]
pub(crate) struct ToolDispatchSeed {
    pub(crate) requested: EffectRequested,
    pub(crate) tool_batch_id: ToolBatchId,
    pub(crate) tool_call_id: ToolCallId,
    pub(crate) call: ValidatedToolCall,
    pub(crate) locator: OperationLocator,
    pub(crate) authorization: AuthorizationContext,
    pub(crate) budget_scope_id: Option<finstack_ai_kernel::BudgetScopeId>,
    pub(crate) attempt: u32,
    pub(crate) requested_at: Timestamp,
}

#[derive(Debug, Clone)]
#[cfg_attr(not(feature = "native-tokio"), allow(dead_code))]
pub(crate) struct TimerDispatchSeed {
    pub(crate) scheduled: finstack_ai_kernel::RetryScheduled,
    pub(crate) scheduled_at: Timestamp,
}

/// Identity, authorization, budget scope, attempt, and deadline for a
/// stage-driver invocation. Built by [`CommitCoordinator::stage_dispatch_seed`],
/// the exact peer of [`ModelDispatchSeed`] / [`ToolDispatchSeed`] for the
/// middleware stage boundary rather than a model or tool effect.
#[derive(Debug, Clone)]
#[allow(
    dead_code,
    reason = "consumed by the stage-settlement choke point wired in a later task"
)]
pub(crate) struct StageDispatchSeed {
    pub(crate) locator: OperationLocator,
    pub(crate) authorization: AuthorizationContext,
    pub(crate) budget_scope_id: Option<finstack_ai_kernel::BudgetScopeId>,
    pub(crate) attempt: u32,
    pub(crate) deadline: Option<Timestamp>,
}

#[derive(Debug, Clone)]
#[cfg_attr(not(feature = "native-tokio"), allow(dead_code))]
pub(crate) struct RuntimeDispatch {
    pub(crate) action: PostCommitAction,
    pub(crate) model: Option<ModelDispatchSeed>,
    pub(crate) tool: Option<ToolDispatchSeed>,
    pub(crate) timer: Option<TimerDispatchSeed>,
}

pub(crate) trait PostCommitDispatcher: PortObject {
    fn validate_before_commit(&self, _input: &KernelInput) -> Result<(), DispatchError> {
        Ok(())
    }

    fn dispatch(&self, dispatch: RuntimeDispatch) -> PortFuture<Result<(), DispatchError>>;
}

pub(super) fn model_dispatch_seed(
    state: &KernelState,
    action: PostCommitAction,
    committed: &CommittedBatch,
    continuation_state: Option<finstack_ai_kernel::RawJson>,
) -> Option<ModelDispatchSeed> {
    let PostCommitAction::ExecuteEffect { effect_id } = action else {
        return None;
    };
    let pending = state.pending_model_effect.as_ref()?;
    if pending.requested.effect_id() != effect_id || pending.deferred.is_some() {
        return None;
    }
    let (locator, authorization, budget_scope_id) = dispatch_security_context(state)?;
    Some(ModelDispatchSeed {
        pending: pending.clone(),
        locator,
        authorization,
        budget_scope_id,
        attempt: state.retry.attempts.checked_add(1)?,
        requested_at: effect_requested_at(committed, effect_id)?,
        continuation_state,
    })
}

pub(super) fn tool_dispatch_seed(
    state: &KernelState,
    action: PostCommitAction,
    committed: &CommittedBatch,
) -> Option<ToolDispatchSeed> {
    let PostCommitAction::ExecuteEffect { effect_id } = action else {
        return None;
    };
    let batch = state.active_tool_batch.as_ref()?;
    let active = batch.calls.iter().find(|call| {
        call.assigned.effect_id == effect_id
            && matches!(
                call.status,
                ActiveToolCallStatus::Requested { deferred: None, .. }
            )
    })?;
    let ActiveToolCallStatus::Requested { requested, .. } = &active.status else {
        return None;
    };
    let finstack_ai_kernel::ToolCallPlan::Execute(call) = &active.assigned.plan else {
        return None;
    };
    let (locator, authorization, budget_scope_id) = dispatch_security_context(state)?;
    Some(ToolDispatchSeed {
        requested: requested.clone(),
        tool_batch_id: batch.opened.tool_batch_id,
        tool_call_id: *call.call.tool_call_id(),
        call: call.clone(),
        locator,
        authorization,
        budget_scope_id,
        attempt: 1,
        requested_at: effect_requested_at(committed, effect_id)?,
    })
}

pub(super) fn timer_dispatch_seed(
    state: &KernelState,
    action: PostCommitAction,
    scheduled_at: Option<Timestamp>,
) -> Option<TimerDispatchSeed> {
    let PostCommitAction::ExecuteEffect { effect_id } = action else {
        return None;
    };
    let scheduled = state.retry.pending.as_ref()?;
    let scheduled_at = scheduled_at?;
    (scheduled.timer_effect_id == effect_id).then(|| TimerDispatchSeed {
        scheduled: scheduled.clone(),
        scheduled_at,
    })
}

fn effect_requested_at(committed: &CommittedBatch, effect_id: EffectId) -> Option<Timestamp> {
    committed.records.iter().find_map(|record| {
        matches!(record.body(), RecordBody::EffectRequested(request) if request.effect_id() == effect_id)
            .then(|| record.timestamp())
    })
}

fn dispatch_security_context(
    state: &KernelState,
) -> Option<(
    OperationLocator,
    AuthorizationContext,
    Option<finstack_ai_kernel::BudgetScopeId>,
)> {
    let accepted = state.accepted.as_ref()?;
    let security = accepted.security();
    let locator = OperationLocator::try_new(
        security.tenant_scope(),
        state.session_id?,
        state.lane_id?,
        accepted.run_id(),
    )
    .ok()?;
    let authorization = AuthorizationContext {
        principal: security.principal().clone(),
        authentication_method: Arc::from(security.authentication_method()),
        assurance_level: Arc::from(security.assurance_level()),
        roles: Arc::from([]),
        permitted_scopes: Arc::from([Arc::from(security.tenant_scope())]),
        safe_claims: Metadata::empty(),
        policy_version: Arc::from(security.authorization_policy_version()),
        decision_id: Arc::from(security.authorization_decision_id()),
    };
    Some((
        locator,
        authorization,
        accepted.relation().budget_scope_id(),
    ))
}
