//! Dedicated schema-1 state-hash projections with recursive explicit nulls.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::digest::Digest;
use crate::effects::{ComponentInvocation, EffectRelation, EffectRequested, PipelinePosition};
use crate::entries::{ContextPrepared, RunCompleted, RunFailed};
use crate::ids::{
    BudgetScopeId, EffectId, LaneId, LimitKey, MessageId, ModelRequestId, RunId, SessionId, TurnId,
};
use crate::limits::{CostLimit, RunLimits};
use crate::projection::{EffectDeferredProjection, ErrorProjection, MessageProjection};
use crate::refs::PrincipalRef;
use crate::run::{
    RunAccepted, RunPropagationPolicy, RunRelation, RunRelationKind, RunSecurityContext,
};
use crate::time::{Duration, Timestamp};

use super::{
    CompletionIdentityHashEntryV1, CurrentTurn, KernelState, ModelSettlementHashEntryV1,
    PendingModelEffect, RunPhase, StageSettlementHashEntryV1, TerminalCandidate, TerminalState,
};

#[derive(Serialize)]
pub(super) struct KernelStateHashV1<'a> {
    pub state_version: u16,
    pub last_applied_sequence: u64,
    pub session_id: Option<SessionId>,
    pub lane_id: Option<LaneId>,
    pub accepted: Option<RunAcceptedProjection<'a>>,
    pub phase: Option<RunPhase>,
    pub cycle: u64,
    pub current_turn: Option<CurrentTurnProjection<'a>>,
    pub messages: Vec<MessageProjection<'a>>,
    pub pending_model_effect: Option<PendingModelEffectProjection<'a>>,
    pub terminal_candidate: Option<TerminalCandidateProjection<'a>>,
    pub stage_settlements: Vec<StageSettlementHashEntryV1>,
    pub model_settlements: Vec<ModelSettlementHashEntryV1>,
    pub completion_identities: Vec<CompletionIdentityHashEntryV1>,
    pub terminal: Option<TerminalStateProjection<'a>>,
}

impl<'a> KernelStateHashV1<'a> {
    pub(super) fn from_state(
        state: &'a KernelState,
        stage_settlements: Vec<StageSettlementHashEntryV1>,
        model_settlements: Vec<ModelSettlementHashEntryV1>,
        completion_identities: Vec<CompletionIdentityHashEntryV1>,
    ) -> Self {
        Self {
            state_version: state.state_version,
            last_applied_sequence: state.last_applied_sequence,
            session_id: state.session_id,
            lane_id: state.lane_id,
            accepted: state.accepted.as_ref().map(RunAcceptedProjection::from),
            phase: state.phase,
            cycle: state.cycle,
            current_turn: state.current_turn.as_ref().map(CurrentTurnProjection::from),
            messages: state.messages.iter().map(MessageProjection::from).collect(),
            pending_model_effect: state
                .pending_model_effect
                .as_ref()
                .map(PendingModelEffectProjection::from),
            terminal_candidate: state
                .terminal_candidate
                .as_ref()
                .map(TerminalCandidateProjection::from),
            stage_settlements,
            model_settlements,
            completion_identities,
            terminal: state.terminal.as_ref().map(TerminalStateProjection::from),
        }
    }
}

#[derive(Serialize)]
pub(super) struct RunAcceptedProjection<'a> {
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
pub(super) struct CurrentTurnProjection<'a> {
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
    messages: Vec<MessageProjection<'a>>,
    context_digest: Digest,
}

impl<'a> From<&'a ContextPrepared> for ContextPreparedProjection<'a> {
    fn from(value: &'a ContextPrepared) -> Self {
        Self {
            cycle: value.cycle,
            turn_id: value.turn_id,
            messages: value.messages.iter().map(MessageProjection::from).collect(),
            context_digest: value.context_digest,
        }
    }
}

#[derive(Serialize)]
pub(super) struct PendingModelEffectProjection<'a> {
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
pub(super) enum TerminalCandidateProjection<'a> {
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
pub(super) enum TerminalStateProjection<'a> {
    Completed(&'a RunCompleted),
    Failed(Box<RunFailedProjection<'a>>),
}

impl<'a> From<&'a TerminalState> for TerminalStateProjection<'a> {
    fn from(value: &'a TerminalState) -> Self {
        match value {
            TerminalState::Completed(completed) => Self::Completed(completed),
            TerminalState::Failed(failed) => {
                Self::Failed(Box::new(RunFailedProjection::from(failed)))
            }
        }
    }
}

#[derive(Serialize)]
pub(super) struct RunFailedProjection<'a> {
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
