//! Limit, lifecycle, tool, run, and terminal hash projections.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::effects::{
    ComponentInvocation, EffectRelation, EffectRequested, InteractionKind, InteractionRequest,
    PipelinePosition,
};
use crate::primitives::Digest;
use crate::primitives::{
    AssigneeHint, BudgetReservationId, BudgetScopeId, ComponentId, ComponentRef, EffectId,
    ExternalHandleRef, InteractionId, LimitKey, MessageId, ModelRequestId, RunId, ToolBatchId,
    ToolCallId, TurnId, Version,
};
use crate::primitives::{CostAmount, PrincipalRef};
use crate::primitives::{Duration, Timestamp};
use crate::primitives::{Metadata, RawJson};
use crate::records::lifecycle::{
    ContextPrepared, RunCancelled, RunCompleted, RunFailed, RunSuspended, StageCursor,
};
use crate::records::policy::{
    BudgetChargeReceipt, BudgetReleaseReceipt, BudgetRequest, BudgetReservationReceipt,
    BudgetReserveRequest, CostLimit, FinalResultRecorded, LimitDimension, LimitReached, LimitUsage,
    LimitValue, OutputConfiguration, OutputEndStrategy, OutputSpec, OutputValidationFailed,
    RunLimits, SchemaRef, StructuredResultSource, ValidationIssue,
};
use crate::records::run::{
    ChildPlacement, ChildRunLocator, ChildRunPrepared, OperationLocator, RemoteRouteRef,
    RunAccepted, RunPropagationPolicy, RunRelation, RunRelationKind, RunSecurityContext,
};
use crate::records::tools::{
    ActiveToolBatch, ActiveToolCall, ActiveToolCallStatus, ToolBatchClosed, ToolBatchContinuation,
};
use crate::state::projection::{
    AssignedToolCallProjection, ContentSeq, EffectDeferredProjection, ErrorProjection, MessageSeq,
    ToolBatchOutcomeProjection, ToolResultProjection, UsageProjection,
};

use super::super::{
    BudgetReservationReplay, CancellationState, CurrentTurn, InteractionTerminal,
    InteractionTerminalOutcome, PendingInteraction, PendingModelEffect, RetryState, RunPhase,
    TerminalCandidate, TerminalState,
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

#[derive(Serialize)]
pub struct LimitReachedProjection<'a> {
    dimension: &'a LimitDimension,
    observed: &'a LimitValue,
    maximum: &'a LimitValue,
    usage: LimitUsageProjection<'a>,
    usage_digest: Digest,
}

impl<'a> From<&'a LimitReached> for LimitReachedProjection<'a> {
    fn from(value: &'a LimitReached) -> Self {
        Self {
            dimension: &value.dimension,
            observed: &value.observed,
            maximum: &value.maximum,
            usage: LimitUsageProjection::from(&value.usage),
            usage_digest: value.usage_digest,
        }
    }
}

#[derive(Serialize)]
pub struct OutputConfigurationProjection<'a> {
    output: &'a OutputSpec,
    end_strategy: OutputEndStrategy,
}

impl<'a> From<&'a OutputConfiguration> for OutputConfigurationProjection<'a> {
    fn from(value: &'a OutputConfiguration) -> Self {
        Self {
            output: &value.output,
            end_strategy: value.end_strategy,
        }
    }
}

#[derive(Serialize)]
pub struct FinalResultRecordedProjection<'a> {
    cycle: u64,
    turn_id: TurnId,
    model_request_id: ModelRequestId,
    effect_id: EffectId,
    message_id: MessageId,
    schema: &'a SchemaRef,
    value: &'a RawJson,
    value_digest: Digest,
    source: &'a StructuredResultSource,
    end_strategy: OutputEndStrategy,
    skipped_tool_call_ids: &'a [ToolCallId],
}

impl<'a> From<&'a FinalResultRecorded> for FinalResultRecordedProjection<'a> {
    fn from(value: &'a FinalResultRecorded) -> Self {
        Self {
            cycle: value.cycle,
            turn_id: value.turn_id,
            model_request_id: value.model_request_id,
            effect_id: value.effect_id,
            message_id: value.message_id,
            schema: &value.schema,
            value: &value.value,
            value_digest: value.value_digest,
            source: &value.source,
            end_strategy: value.end_strategy,
            skipped_tool_call_ids: &value.skipped_tool_call_ids,
        }
    }
}

#[derive(Serialize)]
pub struct OutputValidationFailedProjection<'a> {
    cycle: u64,
    turn_id: TurnId,
    model_request_id: ModelRequestId,
    effect_id: EffectId,
    message_id: MessageId,
    schema: &'a SchemaRef,
    candidate_digest: Digest,
    source: &'a StructuredResultSource,
    issues: Vec<ValidationIssueProjection<'a>>,
    feedback: &'a str,
    error: ErrorProjection<'a>,
    skipped_tool_call_ids: &'a [ToolCallId],
}

impl<'a> From<&'a OutputValidationFailed> for OutputValidationFailedProjection<'a> {
    fn from(value: &'a OutputValidationFailed) -> Self {
        Self {
            cycle: value.cycle,
            turn_id: value.turn_id,
            model_request_id: value.model_request_id,
            effect_id: value.effect_id,
            message_id: value.message_id,
            schema: &value.schema,
            candidate_digest: value.candidate_digest,
            source: &value.source,
            issues: value
                .issues
                .iter()
                .map(ValidationIssueProjection::from)
                .collect(),
            feedback: &value.feedback,
            error: ErrorProjection::from(&value.error),
            skipped_tool_call_ids: &value.skipped_tool_call_ids,
        }
    }
}

#[derive(Serialize)]
struct ValidationIssueProjection<'a> {
    instance_path: &'a str,
    schema_path: &'a str,
    keyword: Option<&'a str>,
    message: &'a str,
}

impl<'a> From<&'a ValidationIssue> for ValidationIssueProjection<'a> {
    fn from(value: &'a ValidationIssue) -> Self {
        Self {
            instance_path: &value.instance_path,
            schema_path: &value.schema_path,
            keyword: value.keyword.as_deref(),
            message: &value.message,
        }
    }
}

#[derive(Serialize)]
pub struct ChildRunPreparedProjection<'a> {
    parent_run_id: RunId,
    parent_effect_id: EffectId,
    child: ChildRunLocatorProjection<'a>,
    request_digest: Digest,
    placement: ChildPlacement,
    budget_reservation_id: Option<BudgetReservationId>,
}

impl<'a> From<&'a ChildRunPrepared> for ChildRunPreparedProjection<'a> {
    fn from(value: &'a ChildRunPrepared) -> Self {
        Self {
            parent_run_id: value.parent_run_id,
            parent_effect_id: value.parent_effect_id,
            child: ChildRunLocatorProjection::from(&value.child),
            request_digest: value.request_digest,
            placement: value.placement,
            budget_reservation_id: value.budget_reservation_id,
        }
    }
}

#[derive(Serialize)]
struct ChildRunLocatorProjection<'a> {
    operation: &'a OperationLocator,
    remote: Option<RemoteRouteRefProjection<'a>>,
}

impl<'a> From<&'a ChildRunLocator> for ChildRunLocatorProjection<'a> {
    fn from(value: &'a ChildRunLocator) -> Self {
        Self {
            operation: &value.operation,
            remote: value.remote.as_ref().map(RemoteRouteRefProjection::from),
        }
    }
}

#[derive(Serialize)]
struct RemoteRouteRefProjection<'a> {
    service: ComponentRefProjection<'a>,
    route: &'a ExternalHandleRef,
}

impl<'a> From<&'a RemoteRouteRef> for RemoteRouteRefProjection<'a> {
    fn from(value: &'a RemoteRouteRef) -> Self {
        Self {
            service: ComponentRefProjection::from(&value.service),
            route: &value.route,
        }
    }
}

#[derive(Serialize)]
struct ComponentRefProjection<'a> {
    id: &'a ComponentId,
    version: Option<Version>,
}

impl<'a> From<&'a ComponentRef> for ComponentRefProjection<'a> {
    fn from(value: &'a ComponentRef) -> Self {
        Self {
            id: value.id(),
            version: value.version(),
        }
    }
}

#[derive(Serialize)]
pub struct BudgetReservationReplayProjection<'a> {
    request: BudgetReserveRequestProjection<'a>,
    settlement: Option<BudgetReservationReceiptProjection<'a>>,
    release: Option<BudgetReleaseReceiptProjection<'a>>,
}

impl<'a> From<&'a BudgetReservationReplay> for BudgetReservationReplayProjection<'a> {
    fn from(value: &'a BudgetReservationReplay) -> Self {
        Self {
            request: BudgetReserveRequestProjection::from(&value.request),
            settlement: value
                .settlement
                .as_ref()
                .map(BudgetReservationReceiptProjection::from),
            release: value
                .release
                .as_ref()
                .map(BudgetReleaseReceiptProjection::from),
        }
    }
}

#[derive(Serialize)]
struct BudgetReserveRequestProjection<'a> {
    scope_id: BudgetScopeId,
    reservation_id: BudgetReservationId,
    run_id: RunId,
    amount: BudgetRequestProjection<'a>,
    request_digest: Digest,
}

impl<'a> From<&'a BudgetReserveRequest> for BudgetReserveRequestProjection<'a> {
    fn from(value: &'a BudgetReserveRequest) -> Self {
        Self {
            scope_id: value.scope_id,
            reservation_id: value.reservation_id,
            run_id: value.run_id,
            amount: BudgetRequestProjection::from(&value.amount),
            request_digest: value.request_digest,
        }
    }
}

#[derive(Serialize)]
struct BudgetRequestProjection<'a> {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cost: Option<&'a CostLimit>,
    extension_counters: &'a BTreeMap<LimitKey, u64>,
}

impl<'a> From<&'a BudgetRequest> for BudgetRequestProjection<'a> {
    fn from(value: &'a BudgetRequest) -> Self {
        Self {
            input_tokens: value.input_tokens,
            output_tokens: value.output_tokens,
            cost: value.cost.as_ref(),
            extension_counters: value.extension_counters.as_inner(),
        }
    }
}

#[derive(Serialize)]
struct BudgetReservationReceiptProjection<'a> {
    scope_id: BudgetScopeId,
    reservation_id: BudgetReservationId,
    reserved: BudgetRequestProjection<'a>,
    remaining: BudgetRequestProjection<'a>,
    request_digest: Digest,
    receipt_digest: Digest,
}

impl<'a> From<&'a BudgetReservationReceipt> for BudgetReservationReceiptProjection<'a> {
    fn from(value: &'a BudgetReservationReceipt) -> Self {
        Self {
            scope_id: value.scope_id,
            reservation_id: value.reservation_id,
            reserved: BudgetRequestProjection::from(&value.reserved),
            remaining: BudgetRequestProjection::from(&value.remaining),
            request_digest: value.request_digest,
            receipt_digest: value.receipt_digest,
        }
    }
}

#[derive(Serialize)]
struct BudgetReleaseReceiptProjection<'a> {
    scope_id: BudgetScopeId,
    reservation_id: BudgetReservationId,
    terminal_run_id: RunId,
    released_unused: BudgetRequestProjection<'a>,
    request_digest: Digest,
    receipt_digest: Digest,
}

impl<'a> From<&'a BudgetReleaseReceipt> for BudgetReleaseReceiptProjection<'a> {
    fn from(value: &'a BudgetReleaseReceipt) -> Self {
        Self {
            scope_id: value.scope_id,
            reservation_id: value.reservation_id,
            terminal_run_id: value.terminal_run_id,
            released_unused: BudgetRequestProjection::from(&value.released_unused),
            request_digest: value.request_digest,
            receipt_digest: value.receipt_digest,
        }
    }
}

#[derive(Serialize)]
pub struct BudgetChargeReceiptProjection<'a> {
    scope_id: BudgetScopeId,
    reservation_id: BudgetReservationId,
    effect_id: EffectId,
    charged_usage: UsageProjection<'a>,
    cumulative_usage: UsageProjection<'a>,
    usage_digest: Digest,
    receipt_digest: Digest,
}

impl<'a> From<&'a BudgetChargeReceipt> for BudgetChargeReceiptProjection<'a> {
    fn from(value: &'a BudgetChargeReceipt) -> Self {
        Self {
            scope_id: value.scope_id,
            reservation_id: value.reservation_id,
            effect_id: value.effect_id,
            charged_usage: UsageProjection::from(&value.charged_usage),
            cumulative_usage: UsageProjection::from(&value.cumulative_usage),
            usage_digest: value.usage_digest,
            receipt_digest: value.receipt_digest,
        }
    }
}

#[derive(Serialize)]
pub struct PendingInteractionProjection<'a> {
    request: InteractionRequestProjection<'a>,
    prior_phase: RunPhase,
    cursor: StageCursor,
}

impl<'a> From<&'a PendingInteraction> for PendingInteractionProjection<'a> {
    fn from(value: &'a PendingInteraction) -> Self {
        Self {
            request: InteractionRequestProjection::from(&value.request),
            prior_phase: value.prior_phase,
            cursor: value.cursor,
        }
    }
}

#[derive(Serialize)]
struct InteractionRequestProjection<'a> {
    request_version: u16,
    interaction_id: InteractionId,
    effect_id: EffectId,
    kind: &'a InteractionKind,
    prompt: ContentSeq<'a>,
    prompt_digest: Digest,
    response_schema: &'a RawJson,
    response_schema_digest: Digest,
    policy_component: ComponentRefProjection<'a>,
    policy_version: Version,
    assignee_hint: Option<AssigneeHintProjection<'a>>,
    expires_at: Option<Timestamp>,
    delegatable: bool,
    metadata: &'a Metadata,
}

impl<'a> From<&'a InteractionRequest> for InteractionRequestProjection<'a> {
    fn from(value: &'a InteractionRequest) -> Self {
        Self {
            request_version: value.request_version(),
            interaction_id: value.interaction_id(),
            effect_id: value.effect_id(),
            kind: value.kind(),
            prompt: ContentSeq::new(value.prompt()),
            prompt_digest: value.prompt_digest(),
            response_schema: value.response_schema(),
            response_schema_digest: value.response_schema_digest(),
            policy_component: ComponentRefProjection::from(value.policy_component()),
            policy_version: value.policy_version(),
            assignee_hint: value.assignee_hint().map(AssigneeHintProjection::from),
            expires_at: value.expires_at(),
            delegatable: value.delegatable(),
            metadata: value.metadata(),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum AssigneeHintProjection<'a> {
    Principal(PrincipalRefProjection<'a>),
    Role(&'a str),
    Queue(&'a str),
}

impl<'a> From<&'a AssigneeHint> for AssigneeHintProjection<'a> {
    fn from(value: &'a AssigneeHint) -> Self {
        match value {
            AssigneeHint::Principal(principal) => {
                Self::Principal(PrincipalRefProjection::from(principal))
            }
            AssigneeHint::Role(role) => Self::Role(role),
            AssigneeHint::Queue(queue) => Self::Queue(queue),
        }
    }
}

#[derive(Serialize)]
pub struct InteractionTerminalProjection<'a> {
    interaction_id: InteractionId,
    kind: &'a InteractionKind,
    cursor: StageCursor,
    outcome: InteractionTerminalOutcome,
}

impl<'a> From<&'a InteractionTerminal> for InteractionTerminalProjection<'a> {
    fn from(value: &'a InteractionTerminal) -> Self {
        Self {
            interaction_id: value.interaction_id,
            kind: &value.kind,
            cursor: value.cursor,
            outcome: value.outcome,
        }
    }
}
