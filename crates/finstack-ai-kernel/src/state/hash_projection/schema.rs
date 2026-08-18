//! Versioned kernel state-hash schemas.

use serde::Serialize;

use crate::primitives::Digest;
use crate::primitives::Timestamp;
use crate::primitives::{LaneId, SessionId};
use crate::records::policy::ActiveCapability;
use crate::records::policy::LimitReached;
use crate::state::projection::MessageSeq;

use super::super::types::{
    CompletionIdentityHashEntryV1, ModelSettlementHashEntryV1, ResolutionIdentityHashEntryV6,
    StageSettlementHashEntryV1, ToolCallIdentityHashRef, ToolSettlementHashEntryV2,
};
use super::super::{KernelState, RunPhase};

use super::projections::{
    ActiveToolBatchProjection, BudgetChargeReceiptProjection, BudgetReservationReplayProjection,
    CancellationStateProjection, ChildRunPreparedProjection, CurrentTurnProjection,
    FinalResultRecordedProjection, InteractionTerminalProjection, LimitReachedProjection,
    LimitUsageProjection, OutputConfigurationProjection, OutputValidationFailedProjection,
    PendingInteractionProjection, PendingModelEffectProjection, RetryStateProjection,
    RunAcceptedProjection, RunSuspendedProjection, TerminalCandidateProjection,
    TerminalStateProjection, ToolBatchClosedProjection,
};

#[derive(Serialize)]
pub struct KernelStateHashV1<'a> {
    pub state_version: u16,
    pub last_applied_sequence: u64,
    pub session_id: Option<SessionId>,
    pub lane_id: Option<LaneId>,
    pub accepted: Option<RunAcceptedProjection<'a>>,
    pub phase: Option<RunPhase>,
    pub cycle: u64,
    pub current_turn: Option<CurrentTurnProjection<'a>>,
    pub messages: MessageSeq<'a>,
    pub pending_model_effect: Option<PendingModelEffectProjection<'a>>,
    pub terminal_candidate: Option<TerminalCandidateProjection<'a>>,
    pub stage_settlements: Vec<StageSettlementHashEntryV1>,
    pub model_settlements: Vec<ModelSettlementHashEntryV1>,
    pub completion_identities: Vec<CompletionIdentityHashEntryV1>,
    pub terminal: Option<TerminalStateProjection<'a>>,
}

impl<'a> KernelStateHashV1<'a> {
    pub(crate) fn from_state(
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
            messages: MessageSeq::new(state.messages.as_slice()),
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
pub struct KernelStateHashV2<'a> {
    pub state_version: u16,
    pub last_applied_sequence: u64,
    pub session_id: Option<SessionId>,
    pub lane_id: Option<LaneId>,
    pub accepted: Option<RunAcceptedProjection<'a>>,
    pub phase: Option<RunPhase>,
    pub cycle: u64,
    pub current_turn: Option<CurrentTurnProjection<'a>>,
    pub messages: MessageSeq<'a>,
    pub pending_model_effect: Option<PendingModelEffectProjection<'a>>,
    pub terminal_candidate: Option<TerminalCandidateProjection<'a>>,
    pub stage_settlements: Vec<StageSettlementHashEntryV1>,
    pub model_settlements: Vec<ModelSettlementHashEntryV1>,
    pub completion_identities: Vec<CompletionIdentityHashEntryV1>,
    pub active_tool_batch: Option<ActiveToolBatchProjection<'a>>,
    pub tool_calls: Vec<ToolCallIdentityHashRef<'a>>,
    pub tool_settlements: Vec<ToolSettlementHashEntryV2>,
    pub last_tool_batch: Option<ToolBatchClosedProjection<'a>>,
    pub terminal: Option<TerminalStateProjection<'a>>,
}

impl<'a> KernelStateHashV2<'a> {
    pub(crate) fn from_state(
        state: &'a KernelState,
        stage_settlements: Vec<StageSettlementHashEntryV1>,
        model_settlements: Vec<ModelSettlementHashEntryV1>,
        completion_identities: Vec<CompletionIdentityHashEntryV1>,
        tool_calls: Vec<ToolCallIdentityHashRef<'a>>,
        tool_settlements: Vec<ToolSettlementHashEntryV2>,
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
            messages: MessageSeq::new(state.messages.as_slice()),
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
            active_tool_batch: state
                .active_tool_batch
                .as_ref()
                .map(ActiveToolBatchProjection::from),
            tool_calls,
            tool_settlements,
            last_tool_batch: state
                .last_tool_batch
                .as_ref()
                .map(ToolBatchClosedProjection::from),
            terminal: state.terminal.as_ref().map(TerminalStateProjection::from),
        }
    }
}

#[derive(Serialize)]
pub struct KernelStateHashV3<'a> {
    pub state_version: u16,
    pub last_applied_sequence: u64,
    pub session_id: Option<SessionId>,
    pub lane_id: Option<LaneId>,
    pub accepted: Option<RunAcceptedProjection<'a>>,
    pub phase: Option<RunPhase>,
    pub cycle: u64,
    pub current_turn: Option<CurrentTurnProjection<'a>>,
    pub messages: MessageSeq<'a>,
    pub pending_model_effect: Option<PendingModelEffectProjection<'a>>,
    pub terminal_candidate: Option<TerminalCandidateProjection<'a>>,
    pub stage_settlements: Vec<StageSettlementHashEntryV1>,
    pub model_settlements: Vec<ModelSettlementHashEntryV1>,
    pub completion_identities: Vec<CompletionIdentityHashEntryV1>,
    pub active_tool_batch: Option<ActiveToolBatchProjection<'a>>,
    pub tool_calls: Vec<ToolCallIdentityHashRef<'a>>,
    pub tool_settlements: Vec<ToolSettlementHashEntryV2>,
    pub last_tool_batch: Option<ToolBatchClosedProjection<'a>>,
    pub accepted_at: Option<Timestamp>,
    pub limit_usage: LimitUsageProjection<'a>,
    pub cancellation: Option<CancellationStateProjection<'a>>,
    pub retry: RetryStateProjection<'a>,
    pub last_limit: Option<&'a LimitReached>,
    pub suspension: Option<RunSuspendedProjection<'a>>,
    pub terminal: Option<TerminalStateProjection<'a>>,
}

impl<'a> KernelStateHashV3<'a> {
    pub(crate) fn from_state(
        state: &'a KernelState,
        stage_settlements: Vec<StageSettlementHashEntryV1>,
        model_settlements: Vec<ModelSettlementHashEntryV1>,
        completion_identities: Vec<CompletionIdentityHashEntryV1>,
        tool_calls: Vec<ToolCallIdentityHashRef<'a>>,
        tool_settlements: Vec<ToolSettlementHashEntryV2>,
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
            messages: MessageSeq::new(state.messages.as_slice()),
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
            active_tool_batch: state
                .active_tool_batch
                .as_ref()
                .map(ActiveToolBatchProjection::from),
            tool_calls,
            tool_settlements,
            last_tool_batch: state
                .last_tool_batch
                .as_ref()
                .map(ToolBatchClosedProjection::from),
            accepted_at: state.accepted_at,
            limit_usage: LimitUsageProjection::from(&state.limit_usage),
            cancellation: state
                .cancellation
                .as_ref()
                .map(CancellationStateProjection::from),
            retry: RetryStateProjection::from(&state.retry),
            last_limit: state.last_limit.as_ref(),
            suspension: state.suspension.as_ref().map(RunSuspendedProjection::from),
            terminal: state.terminal.as_ref().map(TerminalStateProjection::from),
        }
    }
}

#[derive(Serialize)]
pub struct KernelStateHashV4<'a> {
    pub state_version: u16,
    pub last_applied_sequence: u64,
    pub session_id: Option<SessionId>,
    pub lane_id: Option<LaneId>,
    pub accepted: Option<RunAcceptedProjection<'a>>,
    pub phase: Option<RunPhase>,
    pub cycle: u64,
    pub current_turn: Option<CurrentTurnProjection<'a>>,
    pub messages: MessageSeq<'a>,
    pub pending_model_effect: Option<PendingModelEffectProjection<'a>>,
    pub terminal_candidate: Option<TerminalCandidateProjection<'a>>,
    pub stage_settlements: Vec<StageSettlementHashEntryV1>,
    pub model_settlements: Vec<ModelSettlementHashEntryV1>,
    pub completion_identities: Vec<CompletionIdentityHashEntryV1>,
    pub active_tool_batch: Option<ActiveToolBatchProjection<'a>>,
    pub tool_calls: Vec<ToolCallIdentityHashRef<'a>>,
    pub tool_settlements: Vec<ToolSettlementHashEntryV2>,
    pub last_tool_batch: Option<ToolBatchClosedProjection<'a>>,
    pub accepted_at: Option<Timestamp>,
    pub limit_usage: LimitUsageProjection<'a>,
    pub cancellation: Option<CancellationStateProjection<'a>>,
    pub retry: RetryStateProjection<'a>,
    pub last_limit: Option<LimitReachedProjection<'a>>,
    pub suspension: Option<RunSuspendedProjection<'a>>,
    pub output_configuration: Option<OutputConfigurationProjection<'a>>,
    pub active_capabilities: &'a [ActiveCapability],
    pub resolved_plan_digest: Option<Digest>,
    pub final_result: Option<FinalResultRecordedProjection<'a>>,
    pub validation_failure: Option<OutputValidationFailedProjection<'a>>,
    pub terminal: Option<TerminalStateProjection<'a>>,
}

impl<'a> KernelStateHashV4<'a> {
    pub(crate) fn from_state(
        state: &'a KernelState,
        stage_settlements: Vec<StageSettlementHashEntryV1>,
        model_settlements: Vec<ModelSettlementHashEntryV1>,
        completion_identities: Vec<CompletionIdentityHashEntryV1>,
        tool_calls: Vec<ToolCallIdentityHashRef<'a>>,
        tool_settlements: Vec<ToolSettlementHashEntryV2>,
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
            messages: MessageSeq::new(state.messages.as_slice()),
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
            active_tool_batch: state
                .active_tool_batch
                .as_ref()
                .map(ActiveToolBatchProjection::from),
            tool_calls,
            tool_settlements,
            last_tool_batch: state
                .last_tool_batch
                .as_ref()
                .map(ToolBatchClosedProjection::from),
            accepted_at: state.accepted_at,
            limit_usage: LimitUsageProjection::from(&state.limit_usage),
            cancellation: state
                .cancellation
                .as_ref()
                .map(CancellationStateProjection::from),
            retry: RetryStateProjection::from(&state.retry),
            last_limit: state.last_limit.as_ref().map(LimitReachedProjection::from),
            suspension: state.suspension.as_ref().map(RunSuspendedProjection::from),
            output_configuration: state
                .output_configuration
                .as_ref()
                .map(OutputConfigurationProjection::from),
            active_capabilities: &state.active_capabilities,
            resolved_plan_digest: state.resolved_plan_digest,
            final_result: state
                .final_result
                .as_ref()
                .map(FinalResultRecordedProjection::from),
            validation_failure: state
                .validation_failure
                .as_ref()
                .map(OutputValidationFailedProjection::from),
            terminal: state.terminal.as_ref().map(TerminalStateProjection::from),
        }
    }
}

#[derive(Serialize)]
pub struct KernelStateHashV5<'a> {
    #[serde(flatten)]
    base: KernelStateHashV4<'a>,
    child_preparations: Vec<ChildRunPreparedProjection<'a>>,
    budget_reservations: Vec<BudgetReservationReplayProjection<'a>>,
    budget_charges: Vec<BudgetChargeReceiptProjection<'a>>,
}

impl<'a> KernelStateHashV5<'a> {
    pub(crate) fn from_state(
        state: &'a KernelState,
        stage_settlements: Vec<StageSettlementHashEntryV1>,
        model_settlements: Vec<ModelSettlementHashEntryV1>,
        completion_identities: Vec<CompletionIdentityHashEntryV1>,
        tool_calls: Vec<ToolCallIdentityHashRef<'a>>,
        tool_settlements: Vec<ToolSettlementHashEntryV2>,
    ) -> Self {
        Self {
            base: KernelStateHashV4::from_state(
                state,
                stage_settlements,
                model_settlements,
                completion_identities,
                tool_calls,
                tool_settlements,
            ),
            child_preparations: state
                .child_preparations
                .values()
                .map(ChildRunPreparedProjection::from)
                .collect(),
            budget_reservations: state
                .budget_reservations
                .values()
                .map(BudgetReservationReplayProjection::from)
                .collect(),
            budget_charges: state
                .budget_charges
                .values()
                .map(BudgetChargeReceiptProjection::from)
                .collect(),
        }
    }
}

#[derive(Serialize)]
pub struct KernelStateHashV6<'a> {
    #[serde(flatten)]
    base: KernelStateHashV5<'a>,
    pending_interaction: Option<PendingInteractionProjection<'a>>,
    resolution_identities: Vec<ResolutionIdentityHashEntryV6>,
    last_interaction_terminal: Option<InteractionTerminalProjection<'a>>,
}

impl<'a> KernelStateHashV6<'a> {
    pub(crate) fn from_state(
        state: &'a KernelState,
        stage_settlements: Vec<StageSettlementHashEntryV1>,
        model_settlements: Vec<ModelSettlementHashEntryV1>,
        completion_identities: Vec<CompletionIdentityHashEntryV1>,
        tool_calls: Vec<ToolCallIdentityHashRef<'a>>,
        tool_settlements: Vec<ToolSettlementHashEntryV2>,
        resolution_identities: Vec<ResolutionIdentityHashEntryV6>,
    ) -> Self {
        Self {
            base: KernelStateHashV5::from_state(
                state,
                stage_settlements,
                model_settlements,
                completion_identities,
                tool_calls,
                tool_settlements,
            ),
            pending_interaction: state
                .pending_interaction
                .as_ref()
                .map(PendingInteractionProjection::from),
            resolution_identities,
            last_interaction_terminal: state
                .last_interaction_terminal
                .as_ref()
                .map(InteractionTerminalProjection::from),
        }
    }
}
