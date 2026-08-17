//! Limit, lifecycle, tool, run, and terminal hash projections.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::effects::{ComponentInvocation, EffectRelation, EffectRequested, PipelinePosition};
use crate::lifecycle::{ContextPrepared, RunCancelled, RunCompleted, RunFailed, RunSuspended};
use crate::policy::{CostLimit, LimitUsage, RunLimits};
use crate::primitives::Digest;
use crate::primitives::{
    BudgetScopeId, EffectId, LimitKey, MessageId, ModelRequestId, RunId, ToolBatchId, ToolCallId,
    ToolId, TurnId,
};
use crate::primitives::{Duration, Timestamp};
use crate::refs::{CostAmount, PrincipalRef};
use crate::run::{
    RunAccepted, RunPropagationPolicy, RunRelation, RunRelationKind, RunSecurityContext,
};
use crate::state::projection::{
    ContentProjection, EffectDeferredProjection, ErrorProjection, MessageSeq,
};
use crate::tools::{
    ActiveToolBatch, ActiveToolCall, ActiveToolCallStatus, AssignedToolCall, SyntheticToolClosure,
    ToolBatchClosed, ToolBatchContinuation, ToolBatchOutcome, ToolCallPlan, ToolExecutionMode,
    ToolFailurePolicy, ValidatedToolCall,
};

use super::super::{
    CancellationState, CurrentTurn, PendingModelEffect, RetryState, RunPhase, TerminalCandidate,
    TerminalState,
};

#[derive(Serialize)]
pub struct LimitUsageProjection<'a> {
    model_requests: u64,
    turns: u64,
    tool_calls: u64,
    max_parallel_tools: u32,
    input_tokens: u64,
    output_tokens: u64,
    context_bytes: u64,
    output_bytes: u64,
    retries: u32,
    wall_time: Duration,
    cost: Option<&'a CostAmount>,
    extension_counters: Vec<LimitCounterProjection<'a>>,
}

#[derive(Serialize)]
struct LimitCounterProjection<'a> {
    key: &'a LimitKey,
    value: u64,
}

impl<'a> From<&'a LimitUsage> for LimitUsageProjection<'a> {
    fn from(value: &'a LimitUsage) -> Self {
        Self {
            model_requests: value.model_requests,
            turns: value.turns,
            tool_calls: value.tool_calls,
            max_parallel_tools: value.max_parallel_tools,
            input_tokens: value.input_tokens,
            output_tokens: value.output_tokens,
            context_bytes: value.context_bytes,
            output_bytes: value.output_bytes,
            retries: value.retries,
            wall_time: value.wall_time,
            cost: value.cost.as_ref(),
            extension_counters: value
                .extension_counters
                .iter()
                .map(|(key, value)| LimitCounterProjection { key, value: *value })
                .collect(),
        }
    }
}

#[derive(Serialize)]
pub struct CancellationStateProjection<'a> {
    request: CancellationRequestProjection<'a>,
    prior_phase: RunPhase,
    completed_effects: &'a [EffectId],
    cancelled_effects: &'a [EffectId],
    uncertain_effects: &'a [EffectId],
    outstanding_effects: &'a [EffectId],
}

#[derive(Serialize)]
struct CancellationRequestProjection<'a> {
    request_id: crate::CancellationRequestId,
    initiator: &'a crate::CancellationInitiator,
    reason: Option<&'a str>,
}

impl<'a> From<&'a CancellationState> for CancellationStateProjection<'a> {
    fn from(value: &'a CancellationState) -> Self {
        Self {
            request: CancellationRequestProjection {
                request_id: value.request.request_id,
                initiator: &value.request.initiator,
                reason: value.request.reason.as_deref(),
            },
            prior_phase: value.prior_phase,
            completed_effects: &value.completed_effects,
            cancelled_effects: &value.cancelled_effects,
            uncertain_effects: &value.uncertain_effects,
            outstanding_effects: &value.outstanding_effects,
        }
    }
}

#[derive(Serialize)]
pub struct RetryStateProjection<'a> {
    attempts: u32,
    pending: Option<&'a crate::RetryScheduled>,
    timer_firings: Vec<&'a crate::TimerFired>,
}

impl<'a> From<&'a RetryState> for RetryStateProjection<'a> {
    fn from(value: &'a RetryState) -> Self {
        Self {
            attempts: value.attempts,
            pending: value.pending.as_ref(),
            timer_firings: value.timer_firings.values().collect(),
        }
    }
}

#[derive(Serialize)]
pub struct RunSuspendedProjection<'a> {
    reason_code: &'a crate::ErrorCode,
    cancellation_request_id: Option<crate::CancellationRequestId>,
}

impl<'a> From<&'a RunSuspended> for RunSuspendedProjection<'a> {
    fn from(value: &'a RunSuspended) -> Self {
        Self {
            reason_code: &value.reason_code,
            cancellation_request_id: value.cancellation_request_id,
        }
    }
}

#[derive(Serialize)]
pub struct ActiveToolBatchProjection<'a> {
    opened: ToolBatchOpenedProjection<'a>,
    calls: Vec<ActiveToolCallProjection<'a>>,
    current_group: u32,
    next_source_index: u32,
    result_message_ids: &'a [MessageId],
    fatal_error: Option<ErrorProjection<'a>>,
}

impl<'a> From<&'a ActiveToolBatch> for ActiveToolBatchProjection<'a> {
    fn from(value: &'a ActiveToolBatch) -> Self {
        Self {
            opened: ToolBatchOpenedProjection::from(&value.opened),
            calls: value
                .calls
                .iter()
                .map(ActiveToolCallProjection::from)
                .collect(),
            current_group: value.current_group,
            next_source_index: value.next_source_index,
            result_message_ids: &value.result_message_ids,
            fatal_error: value.fatal_error.as_ref().map(ErrorProjection::from),
        }
    }
}

#[derive(Serialize)]
struct ToolBatchOpenedProjection<'a> {
    cycle: u64,
    turn_id: TurnId,
    tool_batch_id: ToolBatchId,
    source_message_id: MessageId,
    calls: Vec<AssignedToolCallProjection<'a>>,
    continuation: ToolBatchContinuation,
    plan_digest: Digest,
}

impl<'a> From<&'a crate::ToolBatchOpened> for ToolBatchOpenedProjection<'a> {
    fn from(value: &'a crate::ToolBatchOpened) -> Self {
        Self {
            cycle: value.cycle,
            turn_id: value.turn_id,
            tool_batch_id: value.tool_batch_id,
            source_message_id: value.source_message_id,
            calls: value
                .calls
                .iter()
                .map(AssignedToolCallProjection::from)
                .collect(),
            continuation: value.continuation,
            plan_digest: value.plan_digest,
        }
    }
}

#[derive(Serialize)]
struct AssignedToolCallProjection<'a> {
    source_index: u32,
    group_index: u32,
    effect_id: EffectId,
    plan: ToolCallPlanProjection<'a>,
}

impl<'a> From<&'a AssignedToolCall> for AssignedToolCallProjection<'a> {
    fn from(value: &'a AssignedToolCall) -> Self {
        Self {
            source_index: value.source_index,
            group_index: value.group_index,
            effect_id: value.effect_id,
            plan: ToolCallPlanProjection::from(&value.plan),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum ToolCallPlanProjection<'a> {
    Execute(ValidatedToolCallProjection<'a>),
    SyntheticClosure(Box<SyntheticToolClosureProjection<'a>>),
}

impl<'a> From<&'a ToolCallPlan> for ToolCallPlanProjection<'a> {
    fn from(value: &'a ToolCallPlan) -> Self {
        match value {
            ToolCallPlan::Execute(call) => Self::Execute(ValidatedToolCallProjection::from(call)),
            ToolCallPlan::SyntheticClosure(closure) => {
                Self::SyntheticClosure(Box::new(SyntheticToolClosureProjection::from(closure)))
            }
        }
    }
}

#[derive(Serialize)]
struct ValidatedToolCallProjection<'a> {
    call: &'a crate::ToolCallBlock,
    tool_id: ToolId,
    component: Option<&'a ComponentInvocation>,
    output_contract: &'a crate::EffectOutputContract,
    retry_safety: crate::RetrySafety,
    deadline: Option<Timestamp>,
    execution: ToolExecutionMode,
    failure_policy: ToolFailurePolicy,
}

impl<'a> From<&'a ValidatedToolCall> for ValidatedToolCallProjection<'a> {
    fn from(value: &'a ValidatedToolCall) -> Self {
        Self {
            call: &value.call,
            tool_id: value.tool_id.clone(),
            component: value.component.as_ref(),
            output_contract: &value.output_contract,
            retry_safety: value.retry_safety,
            deadline: value.deadline,
            execution: value.execution,
            failure_policy: value.failure_policy,
        }
    }
}

#[derive(Serialize)]
struct SyntheticToolClosureProjection<'a> {
    call: &'a crate::ToolCallBlock,
    execution: ToolExecutionMode,
    failure_policy: ToolFailurePolicy,
    error: ErrorProjection<'a>,
}

impl<'a> From<&'a SyntheticToolClosure> for SyntheticToolClosureProjection<'a> {
    fn from(value: &'a SyntheticToolClosure) -> Self {
        Self {
            call: &value.call,
            execution: value.execution,
            failure_policy: value.failure_policy,
            error: ErrorProjection::from(&value.error),
        }
    }
}

#[derive(Serialize)]
struct ActiveToolCallProjection<'a> {
    assigned: AssignedToolCallProjection<'a>,
    status: ActiveToolCallStatusProjection<'a>,
}

impl<'a> From<&'a ActiveToolCall> for ActiveToolCallProjection<'a> {
    fn from(value: &'a ActiveToolCall) -> Self {
        Self {
            assigned: AssignedToolCallProjection::from(&value.assigned),
            status: ActiveToolCallStatusProjection::from(&value.status),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum ActiveToolCallStatusProjection<'a> {
    Undispatched,
    Requested {
        requested: EffectRequestedProjection<'a>,
        deferred: Option<EffectDeferredProjection<'a>>,
    },
    Buffered {
        result: ToolResultProjection<'a>,
        settlement_digest: Digest,
        synthetic: bool,
        error: Option<Box<ErrorProjection<'a>>>,
    },
    Settled {
        result_message_id: MessageId,
        settlement_digest: Digest,
    },
}

impl<'a> From<&'a ActiveToolCallStatus> for ActiveToolCallStatusProjection<'a> {
    fn from(value: &'a ActiveToolCallStatus) -> Self {
        match value {
            ActiveToolCallStatus::Undispatched => Self::Undispatched,
            ActiveToolCallStatus::Requested {
                requested,
                deferred,
            } => Self::Requested {
                requested: EffectRequestedProjection::from(requested),
                deferred: deferred.as_ref().map(EffectDeferredProjection::from),
            },
            ActiveToolCallStatus::Buffered {
                result,
                settlement_digest,
                synthetic,
                error,
            } => Self::Buffered {
                result: ToolResultProjection::from(result),
                settlement_digest: *settlement_digest,
                synthetic: *synthetic,
                error: error.as_ref().map(ErrorProjection::from).map(Box::new),
            },
            ActiveToolCallStatus::Settled {
                result_message_id,
                settlement_digest,
            } => Self::Settled {
                result_message_id: *result_message_id,
                settlement_digest: *settlement_digest,
            },
        }
    }
}

#[derive(Serialize)]
struct ToolResultProjection<'a> {
    tool_call_id: ToolCallId,
    content: Vec<ContentProjection<'a>>,
    is_error: bool,
}

impl<'a> From<&'a crate::ToolResultBlock> for ToolResultProjection<'a> {
    fn from(value: &'a crate::ToolResultBlock) -> Self {
        Self {
            tool_call_id: *value.tool_call_id(),
            content: value
                .content()
                .iter()
                .map(ContentProjection::from)
                .collect(),
            is_error: value.is_error(),
        }
    }
}

#[derive(Serialize)]
pub struct ToolBatchClosedProjection<'a> {
    cycle: u64,
    turn_id: TurnId,
    tool_batch_id: ToolBatchId,
    source_message_id: MessageId,
    result_message_ids: &'a [MessageId],
    outcome: ToolBatchOutcomeProjection<'a>,
    close_digest: Digest,
}

impl<'a> From<&'a ToolBatchClosed> for ToolBatchClosedProjection<'a> {
    fn from(value: &'a ToolBatchClosed) -> Self {
        Self {
            cycle: value.cycle,
            turn_id: value.turn_id,
            tool_batch_id: value.tool_batch_id,
            source_message_id: value.source_message_id,
            result_message_ids: &value.result_message_ids,
            outcome: ToolBatchOutcomeProjection::from(&value.outcome),
            close_digest: value.close_digest,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum ToolBatchOutcomeProjection<'a> {
    ContinueModel,
    Finalize,
    Failed { error: Box<ErrorProjection<'a>> },
}

impl<'a> From<&'a ToolBatchOutcome> for ToolBatchOutcomeProjection<'a> {
    fn from(value: &'a ToolBatchOutcome) -> Self {
        match value {
            ToolBatchOutcome::ContinueModel => Self::ContinueModel,
            ToolBatchOutcome::Finalize => Self::Finalize,
            ToolBatchOutcome::Failed { error } => Self::Failed {
                error: Box::new(ErrorProjection::from(error)),
            },
        }
    }
}

#[derive(Serialize)]
pub struct RunAcceptedProjection<'a> {
    run_id: RunId,
    relation: RunRelationProjection<'a>,
    security: RunSecurityContextProjection<'a>,
    effective_deadline: Option<Timestamp>,
    limits: RunLimitsProjection<'a>,
    propagation: RunPropagationPolicy,
    resolved_agent_lock_digest: Digest,
}

impl<'a> From<&'a RunAccepted> for RunAcceptedProjection<'a> {
    fn from(value: &'a RunAccepted) -> Self {
        Self {
            run_id: value.run_id(),
            relation: RunRelationProjection::from(value.relation()),
            security: RunSecurityContextProjection::from(value.security()),
            effective_deadline: value.effective_deadline(),
            limits: RunLimitsProjection::from(value.limits()),
            propagation: value.propagation(),
            resolved_agent_lock_digest: value.resolved_agent_lock_digest(),
        }
    }
}

#[derive(Serialize)]
struct RunRelationProjection<'a> {
    root_run_id: RunId,
    parent_run_id: Option<RunId>,
    parent_effect_id: Option<EffectId>,
    kind: RunRelationKind,
    depth: u16,
    budget_scope_id: Option<BudgetScopeId>,
    external_work_ref: Option<&'a str>,
}

impl<'a> From<&'a RunRelation> for RunRelationProjection<'a> {
    fn from(value: &'a RunRelation) -> Self {
        Self {
            root_run_id: value.root_run_id(),
            parent_run_id: value.parent_run_id(),
            parent_effect_id: value.parent_effect_id(),
            kind: value.kind(),
            depth: value.depth(),
            budget_scope_id: value.budget_scope_id(),
            external_work_ref: value.external_work_ref(),
        }
    }
}

#[derive(Serialize)]
struct RunSecurityContextProjection<'a> {
    tenant_scope: &'a str,
    principal: PrincipalRefProjection<'a>,
    authentication_method: &'a str,
    assurance_level: &'a str,
    authorization_policy_version: &'a str,
    authorization_decision_id: &'a str,
    delegated_from: Option<PrincipalRefProjection<'a>>,
}

impl<'a> From<&'a RunSecurityContext> for RunSecurityContextProjection<'a> {
    fn from(value: &'a RunSecurityContext) -> Self {
        Self {
            tenant_scope: value.tenant_scope(),
            principal: PrincipalRefProjection::from(value.principal()),
            authentication_method: value.authentication_method(),
            assurance_level: value.assurance_level(),
            authorization_policy_version: value.authorization_policy_version(),
            authorization_decision_id: value.authorization_decision_id(),
            delegated_from: value.delegated_from().map(PrincipalRefProjection::from),
        }
    }
}

#[derive(Serialize)]
struct PrincipalRefProjection<'a> {
    issuer: &'a str,
    subject: &'a str,
    tenant_scope: Option<&'a str>,
}

impl<'a> From<&'a PrincipalRef> for PrincipalRefProjection<'a> {
    fn from(value: &'a PrincipalRef) -> Self {
        Self {
            issuer: value.issuer(),
            subject: value.subject(),
            tenant_scope: value.tenant_scope(),
        }
    }
}

#[derive(Serialize)]
struct RunLimitsProjection<'a> {
    max_model_requests: Option<u64>,
    max_turns: Option<u64>,
    max_tool_calls: Option<u64>,
    max_parallel_tools: Option<u32>,
    max_input_tokens: Option<u64>,
    max_output_tokens: Option<u64>,
    max_context_bytes: Option<u64>,
    max_output_bytes: Option<u64>,
    max_retries: Option<u32>,
    max_wall_time: Option<&'a Duration>,
    max_cost: Option<&'a CostLimit>,
    extension_counters: &'a BTreeMap<LimitKey, u64>,
}

impl<'a> From<&'a RunLimits> for RunLimitsProjection<'a> {
    fn from(value: &'a RunLimits) -> Self {
        Self {
            max_model_requests: value.max_model_requests,
            max_turns: value.max_turns,
            max_tool_calls: value.max_tool_calls,
            max_parallel_tools: value.max_parallel_tools,
            max_input_tokens: value.max_input_tokens,
            max_output_tokens: value.max_output_tokens,
            max_context_bytes: value.max_context_bytes,
            max_output_bytes: value.max_output_bytes,
            max_retries: value.max_retries,
            max_wall_time: value.max_wall_time.as_ref(),
            max_cost: value.max_cost.as_ref(),
            extension_counters: &value.extension_counters,
        }
    }
}

#[derive(Serialize)]
pub struct CurrentTurnProjection<'a> {
    cycle: u64,
    turn_id: TurnId,
    context: ContextPreparedProjection<'a>,
    model_request_id: Option<ModelRequestId>,
    effect_id: Option<EffectId>,
    final_message_id: Option<MessageId>,
}

impl<'a> From<&'a CurrentTurn> for CurrentTurnProjection<'a> {
    fn from(value: &'a CurrentTurn) -> Self {
        Self {
            cycle: value.cycle,
            turn_id: value.turn_id,
            context: ContextPreparedProjection::from(&value.context),
            model_request_id: value.model_request_id,
            effect_id: value.effect_id,
            final_message_id: value.final_message_id,
        }
    }
}

#[derive(Serialize)]
struct ContextPreparedProjection<'a> {
    cycle: u64,
    turn_id: TurnId,
    messages: MessageSeq<'a>,
    context_digest: Digest,
}

impl<'a> From<&'a ContextPrepared> for ContextPreparedProjection<'a> {
    fn from(value: &'a ContextPrepared) -> Self {
        Self {
            cycle: value.cycle,
            turn_id: value.turn_id,
            messages: MessageSeq::new(&value.messages),
            context_digest: value.context_digest,
        }
    }
}

#[derive(Serialize)]
pub struct PendingModelEffectProjection<'a> {
    cycle: u64,
    turn_id: TurnId,
    model_request_id: ModelRequestId,
    requested: EffectRequestedProjection<'a>,
    deferred: Option<EffectDeferredProjection<'a>>,
}

impl<'a> From<&'a PendingModelEffect> for PendingModelEffectProjection<'a> {
    fn from(value: &'a PendingModelEffect) -> Self {
        Self {
            cycle: value.cycle,
            turn_id: value.turn_id,
            model_request_id: value.model_request_id,
            requested: EffectRequestedProjection::from(&value.requested),
            deferred: value.deferred.as_ref().map(EffectDeferredProjection::from),
        }
    }
}

#[derive(Serialize)]
struct EffectRequestedProjection<'a> {
    effect_id: EffectId,
    kind: crate::effects::EffectKind,
    relation: Option<&'a EffectRelation>,
    component: Option<&'a ComponentInvocation>,
    pipeline: Option<&'a PipelinePosition>,
    output_contract: &'a crate::effects::EffectOutputContract,
    input: &'a crate::effects::EffectInput,
    input_digest: Digest,
    retry_safety: crate::effects::RetrySafety,
    deadline: Option<Timestamp>,
}

impl<'a> From<&'a EffectRequested> for EffectRequestedProjection<'a> {
    fn from(value: &'a EffectRequested) -> Self {
        Self {
            effect_id: value.effect_id(),
            kind: value.kind(),
            relation: value.relation(),
            component: value.component(),
            pipeline: value.pipeline(),
            output_contract: value.output_contract(),
            input: value.input(),
            input_digest: value.input_digest(),
            retry_safety: value.retry_safety(),
            deadline: value.deadline(),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalCandidateProjection<'a> {
    Completed {
        cycle: u64,
        turn_id: TurnId,
        model_request_id: ModelRequestId,
        effect_id: EffectId,
        message_id: MessageId,
        result_digest: Digest,
    },
    Failed {
        cycle: u64,
        turn_id: Option<TurnId>,
        model_request_id: Option<ModelRequestId>,
        effect_id: Option<EffectId>,
        error: Box<ErrorProjection<'a>>,
    },
}

impl<'a> From<&'a TerminalCandidate> for TerminalCandidateProjection<'a> {
    fn from(value: &'a TerminalCandidate) -> Self {
        match value {
            TerminalCandidate::Completed {
                cycle,
                turn_id,
                model_request_id,
                effect_id,
                message_id,
                result_digest,
            } => Self::Completed {
                cycle: *cycle,
                turn_id: *turn_id,
                model_request_id: *model_request_id,
                effect_id: *effect_id,
                message_id: *message_id,
                result_digest: *result_digest,
            },
            TerminalCandidate::Failed {
                cycle,
                turn_id,
                model_request_id,
                effect_id,
                error,
            } => Self::Failed {
                cycle: *cycle,
                turn_id: *turn_id,
                model_request_id: *model_request_id,
                effect_id: *effect_id,
                error: Box::new(ErrorProjection::from(error)),
            },
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalStateProjection<'a> {
    Completed(&'a RunCompleted),
    Failed(Box<RunFailedProjection<'a>>),
    Cancelled(&'a RunCancelled),
}

impl<'a> From<&'a TerminalState> for TerminalStateProjection<'a> {
    fn from(value: &'a TerminalState) -> Self {
        match value {
            TerminalState::Completed(completed) => Self::Completed(completed),
            TerminalState::Failed(failed) => {
                Self::Failed(Box::new(RunFailedProjection::from(failed)))
            }
            TerminalState::Cancelled(cancelled) => Self::Cancelled(cancelled),
        }
    }
}

#[derive(Serialize)]
pub struct RunFailedProjection<'a> {
    cycle: u64,
    turn_id: Option<TurnId>,
    model_request_id: Option<ModelRequestId>,
    effect_id: Option<EffectId>,
    error: ErrorProjection<'a>,
}

impl<'a> From<&'a RunFailed> for RunFailedProjection<'a> {
    fn from(value: &'a RunFailed) -> Self {
        Self {
            cycle: value.cycle,
            turn_id: value.turn_id,
            model_request_id: value.model_request_id,
            effect_id: value.effect_id,
            error: ErrorProjection::from(&value.error),
        }
    }
}
