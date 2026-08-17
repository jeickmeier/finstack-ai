use std::collections::BTreeMap;
use std::sync::Arc;

use serde::de;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::conversation::Message;
use crate::primitives::Digest;
use crate::primitives::Timestamp;
use crate::primitives::{BoundedVec, SEMANTIC_ARRAY_MAX_ITEMS, SEMANTIC_MAP_MAX_ENTRIES};
use crate::primitives::{LaneId, SessionId};
use crate::records::lifecycle::{RunSuspended, StageCursor};
use crate::records::policy::ActiveCapability;
use crate::records::policy::BudgetChargeReceipt;
use crate::records::policy::OutputValidationFailed;
use crate::records::policy::{FinalResultRecorded, OutputConfiguration};
use crate::records::policy::{LimitReached, LimitUsage};
use crate::records::run::{ChildRunPrepared, RunAccepted};
use crate::records::tools::{
    ActiveToolBatch, ToolBatchClosed, ToolCallIdentity, ToolSettlementFingerprint,
};

use super::hash_entries::{
    completion_hash_entries, model_hash_entries, resolution_hash_entries, stage_hash_entries,
    tool_call_hash_entries, tool_settlement_hash_entries,
};
use super::{
    BudgetReservationReplay, CancellationState, CompletionIdentity, CompletionIdentityHashEntryV1,
    CurrentTurn, InteractionTerminal, KernelState, ModelSettlementFingerprint,
    ModelSettlementHashEntryV1, PendingInteraction, PendingModelEffect, ResolutionIdentity,
    ResolutionIdentityHashEntryV6, RetryState, RunPhase, StageSettlementHashEntryV1,
    TerminalCandidate, TerminalState, ToolCallIdentityHashEntryV2, ToolSettlementHashEntryV2,
};

#[derive(Serialize)]
struct KernelStateWireV1<'a> {
    state_version: u16,
    last_applied_sequence: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    session_id: Option<SessionId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    lane_id: Option<LaneId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    accepted: Option<&'a RunAccepted>,
    #[serde(skip_serializing_if = "Option::is_none")]
    phase: Option<RunPhase>,
    cycle: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    current_turn: Option<&'a CurrentTurn>,
    messages: &'a [Message],
    #[serde(skip_serializing_if = "Option::is_none")]
    pending_model_effect: Option<&'a PendingModelEffect>,
    #[serde(skip_serializing_if = "Option::is_none")]
    terminal_candidate: Option<&'a TerminalCandidate>,
    stage_settlements: Vec<StageSettlementHashEntryV1>,
    model_settlements: Vec<ModelSettlementHashEntryV1>,
    completion_identities: Vec<CompletionIdentityHashEntryV1>,
    #[serde(skip_serializing_if = "Option::is_none")]
    terminal: Option<&'a TerminalState>,
    /// Snapshot-only sidecar; excluded from the v1 hash and from v3 field names.
    #[serde(skip_serializing_if = "Option::is_none")]
    snapshot_accepted_at: Option<Timestamp>,
    /// Snapshot-only sidecar; excluded from the v1 hash and from v3 field names.
    #[serde(skip_serializing_if = "Option::is_none")]
    snapshot_limit_usage: Option<&'a LimitUsage>,
}

#[derive(Serialize)]
struct KernelStateWireV2<'a> {
    state_version: u16,
    last_applied_sequence: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    session_id: Option<SessionId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    lane_id: Option<LaneId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    accepted: Option<&'a RunAccepted>,
    #[serde(skip_serializing_if = "Option::is_none")]
    phase: Option<RunPhase>,
    cycle: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    current_turn: Option<&'a CurrentTurn>,
    messages: &'a [Message],
    #[serde(skip_serializing_if = "Option::is_none")]
    pending_model_effect: Option<&'a PendingModelEffect>,
    #[serde(skip_serializing_if = "Option::is_none")]
    terminal_candidate: Option<&'a TerminalCandidate>,
    stage_settlements: Vec<StageSettlementHashEntryV1>,
    model_settlements: Vec<ModelSettlementHashEntryV1>,
    completion_identities: Vec<CompletionIdentityHashEntryV1>,
    active_tool_batch: Option<&'a ActiveToolBatch>,
    tool_calls: Vec<ToolCallIdentityHashEntryV2>,
    tool_settlements: Vec<ToolSettlementHashEntryV2>,
    last_tool_batch: Option<&'a ToolBatchClosed>,
    #[serde(skip_serializing_if = "Option::is_none")]
    terminal: Option<&'a TerminalState>,
    /// Snapshot-only sidecar; excluded from the v2 hash and from v3 field names.
    #[serde(skip_serializing_if = "Option::is_none")]
    snapshot_accepted_at: Option<Timestamp>,
    /// Snapshot-only sidecar; excluded from the v2 hash and from v3 field names.
    #[serde(skip_serializing_if = "Option::is_none")]
    snapshot_limit_usage: Option<&'a LimitUsage>,
}

#[derive(Serialize)]
struct KernelStateWireV3<'a> {
    state_version: u16,
    last_applied_sequence: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    session_id: Option<SessionId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    lane_id: Option<LaneId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    accepted: Option<&'a RunAccepted>,
    accepted_at: Option<Timestamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    phase: Option<RunPhase>,
    cycle: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    current_turn: Option<&'a CurrentTurn>,
    messages: &'a [Message],
    #[serde(skip_serializing_if = "Option::is_none")]
    pending_model_effect: Option<&'a PendingModelEffect>,
    #[serde(skip_serializing_if = "Option::is_none")]
    terminal_candidate: Option<&'a TerminalCandidate>,
    stage_settlements: Vec<StageSettlementHashEntryV1>,
    model_settlements: Vec<ModelSettlementHashEntryV1>,
    completion_identities: Vec<CompletionIdentityHashEntryV1>,
    active_tool_batch: Option<&'a ActiveToolBatch>,
    tool_calls: Vec<ToolCallIdentityHashEntryV2>,
    tool_settlements: Vec<ToolSettlementHashEntryV2>,
    last_tool_batch: Option<&'a ToolBatchClosed>,
    limit_usage: &'a LimitUsage,
    cancellation: Option<&'a CancellationState>,
    retry: &'a RetryState,
    last_limit: Option<&'a LimitReached>,
    suspension: Option<&'a RunSuspended>,
    #[serde(skip_serializing_if = "Option::is_none")]
    terminal: Option<&'a TerminalState>,
}

#[derive(Serialize)]
struct KernelStateWireV4<'a> {
    state_version: u16,
    last_applied_sequence: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    session_id: Option<SessionId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    lane_id: Option<LaneId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    accepted: Option<&'a RunAccepted>,
    accepted_at: Option<Timestamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    phase: Option<RunPhase>,
    cycle: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    current_turn: Option<&'a CurrentTurn>,
    messages: &'a [Message],
    #[serde(skip_serializing_if = "Option::is_none")]
    pending_model_effect: Option<&'a PendingModelEffect>,
    #[serde(skip_serializing_if = "Option::is_none")]
    terminal_candidate: Option<&'a TerminalCandidate>,
    stage_settlements: Vec<StageSettlementHashEntryV1>,
    model_settlements: Vec<ModelSettlementHashEntryV1>,
    completion_identities: Vec<CompletionIdentityHashEntryV1>,
    active_tool_batch: Option<&'a ActiveToolBatch>,
    tool_calls: Vec<ToolCallIdentityHashEntryV2>,
    tool_settlements: Vec<ToolSettlementHashEntryV2>,
    last_tool_batch: Option<&'a ToolBatchClosed>,
    limit_usage: &'a LimitUsage,
    cancellation: Option<&'a CancellationState>,
    retry: &'a RetryState,
    last_limit: Option<&'a LimitReached>,
    suspension: Option<&'a RunSuspended>,
    output_configuration: Option<&'a OutputConfiguration>,
    active_capabilities: &'a [ActiveCapability],
    resolved_plan_digest: Option<Digest>,
    final_result: Option<&'a FinalResultRecorded>,
    validation_failure: Option<&'a OutputValidationFailed>,
    #[serde(skip_serializing_if = "Option::is_none")]
    terminal: Option<&'a TerminalState>,
}

#[derive(Serialize)]
struct KernelStateWireV5<'a> {
    #[serde(flatten)]
    base: KernelStateWireV4<'a>,
    child_preparations: Vec<&'a ChildRunPrepared>,
    budget_reservations: Vec<&'a BudgetReservationReplay>,
    budget_charges: Vec<&'a BudgetChargeReceipt>,
}

#[derive(Serialize)]
struct KernelStateWireV6<'a> {
    #[serde(flatten)]
    base: KernelStateWireV5<'a>,
    pending_interaction: Option<&'a PendingInteraction>,
    resolution_identities: Vec<ResolutionIdentityHashEntryV6>,
    last_interaction_terminal: Option<&'a InteractionTerminal>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct KernelStateWireOwned {
    state_version: u16,
    last_applied_sequence: u64,
    #[serde(default)]
    session_id: Option<SessionId>,
    #[serde(default)]
    lane_id: Option<LaneId>,
    #[serde(default)]
    accepted: Option<RunAccepted>,
    #[serde(default)]
    accepted_at: NullableField<Timestamp>,
    #[serde(default)]
    phase: Option<RunPhase>,
    cycle: u64,
    #[serde(default)]
    current_turn: Option<CurrentTurn>,
    messages: BoundedVec<Message, SEMANTIC_ARRAY_MAX_ITEMS>,
    #[serde(default)]
    pending_model_effect: Option<PendingModelEffect>,
    #[serde(default)]
    terminal_candidate: Option<TerminalCandidate>,
    stage_settlements: BoundedVec<StageSettlementHashEntryV1, SEMANTIC_MAP_MAX_ENTRIES>,
    model_settlements: BoundedVec<ModelSettlementHashEntryV1, SEMANTIC_MAP_MAX_ENTRIES>,
    completion_identities: BoundedVec<CompletionIdentityHashEntryV1, SEMANTIC_MAP_MAX_ENTRIES>,
    #[serde(default)]
    active_tool_batch: NullableField<ActiveToolBatch>,
    #[serde(default)]
    tool_calls: RequiredField<BoundedVec<ToolCallIdentityHashEntryV2, SEMANTIC_MAP_MAX_ENTRIES>>,
    #[serde(default)]
    tool_settlements:
        RequiredField<BoundedVec<ToolSettlementHashEntryV2, SEMANTIC_MAP_MAX_ENTRIES>>,
    #[serde(default)]
    last_tool_batch: NullableField<ToolBatchClosed>,
    #[serde(default)]
    limit_usage: RequiredField<LimitUsage>,
    #[serde(default)]
    cancellation: NullableField<CancellationState>,
    #[serde(default)]
    retry: RequiredField<RetryState>,
    #[serde(default)]
    last_limit: NullableField<LimitReached>,
    #[serde(default)]
    suspension: NullableField<RunSuspended>,
    #[serde(default)]
    output_configuration: NullableField<OutputConfiguration>,
    #[serde(default)]
    active_capabilities: RequiredField<BoundedVec<ActiveCapability, SEMANTIC_ARRAY_MAX_ITEMS>>,
    #[serde(default)]
    resolved_plan_digest: NullableField<Digest>,
    #[serde(default)]
    final_result: NullableField<FinalResultRecorded>,
    #[serde(default)]
    validation_failure: NullableField<OutputValidationFailed>,
    #[serde(default)]
    child_preparations: RequiredField<BoundedVec<ChildRunPrepared, SEMANTIC_MAP_MAX_ENTRIES>>,
    #[serde(default)]
    budget_reservations:
        RequiredField<BoundedVec<BudgetReservationReplay, SEMANTIC_MAP_MAX_ENTRIES>>,
    #[serde(default)]
    budget_charges: RequiredField<BoundedVec<BudgetChargeReceipt, SEMANTIC_MAP_MAX_ENTRIES>>,
    #[serde(default)]
    pending_interaction: NullableField<PendingInteraction>,
    #[serde(default)]
    resolution_identities:
        RequiredField<BoundedVec<ResolutionIdentityHashEntryV6, SEMANTIC_MAP_MAX_ENTRIES>>,
    #[serde(default)]
    last_interaction_terminal: NullableField<InteractionTerminal>,
    #[serde(default)]
    terminal: Option<TerminalState>,
    #[serde(default)]
    snapshot_accepted_at: Option<Timestamp>,
    #[serde(default)]
    snapshot_limit_usage: Option<LimitUsage>,
}

#[derive(Default)]
enum RequiredField<T> {
    #[default]
    Missing,
    Present(T),
}

impl<'de, T> Deserialize<'de> for RequiredField<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        T::deserialize(deserializer).map(Self::Present)
    }
}

#[derive(Default)]
enum NullableField<T> {
    #[default]
    Missing,
    Present(Option<T>),
}

impl<'de, T> Deserialize<'de> for NullableField<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Option::<T>::deserialize(deserializer).map(Self::Present)
    }
}

impl Serialize for KernelState {
    #[expect(
        clippy::too_many_lines,
        reason = "each state version has an explicit fixed wire projection to prevent field drift"
    )]
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if self.state_version == 1 {
            KernelStateWireV1 {
                state_version: self.state_version,
                last_applied_sequence: self.last_applied_sequence,
                session_id: self.session_id,
                lane_id: self.lane_id,
                accepted: self.accepted.as_ref(),
                phase: self.phase,
                cycle: self.cycle,
                current_turn: self.current_turn.as_ref(),
                messages: self.messages.as_slice(),
                pending_model_effect: self.pending_model_effect.as_ref(),
                terminal_candidate: self.terminal_candidate.as_ref(),
                stage_settlements: stage_hash_entries(&self.stage_settlements),
                model_settlements: model_hash_entries(&self.model_settlements),
                completion_identities: completion_hash_entries(&self.completion_identities),
                terminal: self.terminal.as_ref(),
                snapshot_accepted_at: self.accepted_at,
                snapshot_limit_usage: v1_v2_snapshot_limit_usage(&self.limit_usage),
            }
            .serialize(serializer)
        } else if self.state_version == 2 {
            KernelStateWireV2 {
                state_version: self.state_version,
                last_applied_sequence: self.last_applied_sequence,
                session_id: self.session_id,
                lane_id: self.lane_id,
                accepted: self.accepted.as_ref(),
                phase: self.phase,
                cycle: self.cycle,
                current_turn: self.current_turn.as_ref(),
                messages: self.messages.as_slice(),
                pending_model_effect: self.pending_model_effect.as_ref(),
                terminal_candidate: self.terminal_candidate.as_ref(),
                stage_settlements: stage_hash_entries(&self.stage_settlements),
                model_settlements: model_hash_entries(&self.model_settlements),
                completion_identities: completion_hash_entries(&self.completion_identities),
                active_tool_batch: self.active_tool_batch.as_ref(),
                tool_calls: tool_call_hash_entries(&self.tool_calls),
                tool_settlements: tool_settlement_hash_entries(&self.tool_settlements),
                last_tool_batch: self.last_tool_batch.as_ref(),
                terminal: self.terminal.as_ref(),
                snapshot_accepted_at: self.accepted_at,
                snapshot_limit_usage: v1_v2_snapshot_limit_usage(&self.limit_usage),
            }
            .serialize(serializer)
        } else if self.state_version == 3 {
            KernelStateWireV3 {
                state_version: self.state_version,
                last_applied_sequence: self.last_applied_sequence,
                session_id: self.session_id,
                lane_id: self.lane_id,
                accepted: self.accepted.as_ref(),
                accepted_at: self.accepted_at,
                phase: self.phase,
                cycle: self.cycle,
                current_turn: self.current_turn.as_ref(),
                messages: self.messages.as_slice(),
                pending_model_effect: self.pending_model_effect.as_ref(),
                terminal_candidate: self.terminal_candidate.as_ref(),
                stage_settlements: stage_hash_entries(&self.stage_settlements),
                model_settlements: model_hash_entries(&self.model_settlements),
                completion_identities: completion_hash_entries(&self.completion_identities),
                active_tool_batch: self.active_tool_batch.as_ref(),
                tool_calls: tool_call_hash_entries(&self.tool_calls),
                tool_settlements: tool_settlement_hash_entries(&self.tool_settlements),
                last_tool_batch: self.last_tool_batch.as_ref(),
                limit_usage: &self.limit_usage,
                cancellation: self.cancellation.as_ref(),
                retry: &self.retry,
                last_limit: self.last_limit.as_ref(),
                suspension: self.suspension.as_ref(),
                terminal: self.terminal.as_ref(),
            }
            .serialize(serializer)
        } else if self.state_version == 4 {
            KernelStateWireV4 {
                state_version: self.state_version,
                last_applied_sequence: self.last_applied_sequence,
                session_id: self.session_id,
                lane_id: self.lane_id,
                accepted: self.accepted.as_ref(),
                accepted_at: self.accepted_at,
                phase: self.phase,
                cycle: self.cycle,
                current_turn: self.current_turn.as_ref(),
                messages: self.messages.as_slice(),
                pending_model_effect: self.pending_model_effect.as_ref(),
                terminal_candidate: self.terminal_candidate.as_ref(),
                stage_settlements: stage_hash_entries(&self.stage_settlements),
                model_settlements: model_hash_entries(&self.model_settlements),
                completion_identities: completion_hash_entries(&self.completion_identities),
                active_tool_batch: self.active_tool_batch.as_ref(),
                tool_calls: tool_call_hash_entries(&self.tool_calls),
                tool_settlements: tool_settlement_hash_entries(&self.tool_settlements),
                last_tool_batch: self.last_tool_batch.as_ref(),
                limit_usage: &self.limit_usage,
                cancellation: self.cancellation.as_ref(),
                retry: &self.retry,
                last_limit: self.last_limit.as_ref(),
                suspension: self.suspension.as_ref(),
                output_configuration: self.output_configuration.as_ref(),
                active_capabilities: &self.active_capabilities,
                resolved_plan_digest: self.resolved_plan_digest,
                final_result: self.final_result.as_ref(),
                validation_failure: self.validation_failure.as_ref(),
                terminal: self.terminal.as_ref(),
            }
            .serialize(serializer)
        } else {
            let base = KernelStateWireV5 {
                base: KernelStateWireV4 {
                    state_version: self.state_version,
                    last_applied_sequence: self.last_applied_sequence,
                    session_id: self.session_id,
                    lane_id: self.lane_id,
                    accepted: self.accepted.as_ref(),
                    accepted_at: self.accepted_at,
                    phase: self.phase,
                    cycle: self.cycle,
                    current_turn: self.current_turn.as_ref(),
                    messages: self.messages.as_slice(),
                    pending_model_effect: self.pending_model_effect.as_ref(),
                    terminal_candidate: self.terminal_candidate.as_ref(),
                    stage_settlements: stage_hash_entries(&self.stage_settlements),
                    model_settlements: model_hash_entries(&self.model_settlements),
                    completion_identities: completion_hash_entries(&self.completion_identities),
                    active_tool_batch: self.active_tool_batch.as_ref(),
                    tool_calls: tool_call_hash_entries(&self.tool_calls),
                    tool_settlements: tool_settlement_hash_entries(&self.tool_settlements),
                    last_tool_batch: self.last_tool_batch.as_ref(),
                    limit_usage: &self.limit_usage,
                    cancellation: self.cancellation.as_ref(),
                    retry: &self.retry,
                    last_limit: self.last_limit.as_ref(),
                    suspension: self.suspension.as_ref(),
                    output_configuration: self.output_configuration.as_ref(),
                    active_capabilities: &self.active_capabilities,
                    resolved_plan_digest: self.resolved_plan_digest,
                    final_result: self.final_result.as_ref(),
                    validation_failure: self.validation_failure.as_ref(),
                    terminal: self.terminal.as_ref(),
                },
                child_preparations: self.child_preparations.values().collect(),
                budget_reservations: self.budget_reservations.values().collect(),
                budget_charges: self.budget_charges.values().collect(),
            };
            if self.state_version == 5 {
                base.serialize(serializer)
            } else {
                KernelStateWireV6 {
                    base,
                    pending_interaction: self.pending_interaction.as_ref(),
                    resolution_identities: resolution_hash_entries(&self.resolution_identities),
                    last_interaction_terminal: self.last_interaction_terminal.as_ref(),
                }
                .serialize(serializer)
            }
        }
    }
}

impl<'de> Deserialize<'de> for KernelState {
    #[expect(
        clippy::too_many_lines,
        reason = "version dispatch and duplicate-map rejection must remain one atomic decode path"
    )]
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = KernelStateWireOwned::deserialize(deserializer)?;
        if !matches!(wire.state_version, 1..=6) {
            return Err(de::Error::custom("unsupported kernel state_version"));
        }
        let tool_fields_present = matches!(&wire.active_tool_batch, NullableField::Present(_))
            || matches!(&wire.tool_calls, RequiredField::Present(_))
            || matches!(&wire.tool_settlements, RequiredField::Present(_))
            || matches!(&wire.last_tool_batch, NullableField::Present(_));
        let control_fields_present = matches!(&wire.accepted_at, NullableField::Present(_))
            || matches!(&wire.limit_usage, RequiredField::Present(_))
            || matches!(&wire.cancellation, NullableField::Present(_))
            || matches!(&wire.retry, RequiredField::Present(_))
            || matches!(&wire.last_limit, NullableField::Present(_))
            || matches!(&wire.suspension, NullableField::Present(_));
        if wire.state_version == 1 && tool_fields_present {
            return Err(de::Error::custom("v1 kernel state contains tool fields"));
        }
        let all_tool_fields_present = matches!(&wire.active_tool_batch, NullableField::Present(_))
            && matches!(&wire.tool_calls, RequiredField::Present(_))
            && matches!(&wire.tool_settlements, RequiredField::Present(_))
            && matches!(&wire.last_tool_batch, NullableField::Present(_));
        if matches!(wire.state_version, 2..=6) && !all_tool_fields_present {
            return Err(de::Error::custom("v2 kernel state is missing tool indexes"));
        }
        let all_control_fields_present = matches!(&wire.accepted_at, NullableField::Present(_))
            && matches!(&wire.limit_usage, RequiredField::Present(_))
            && matches!(&wire.cancellation, NullableField::Present(_))
            && matches!(&wire.retry, RequiredField::Present(_))
            && matches!(&wire.last_limit, NullableField::Present(_))
            && matches!(&wire.suspension, NullableField::Present(_));
        if wire.state_version < 3 && control_fields_present {
            return Err(de::Error::custom(
                "v1/v2 kernel state contains control fields",
            ));
        }
        if wire.state_version >= 3 && !all_control_fields_present {
            return Err(de::Error::custom(
                "v3/v4 kernel state is missing control fields",
            ));
        }
        let structured_fields_present =
            matches!(&wire.output_configuration, NullableField::Present(_))
                || matches!(&wire.active_capabilities, RequiredField::Present(_))
                || matches!(&wire.resolved_plan_digest, NullableField::Present(_))
                || matches!(&wire.final_result, NullableField::Present(_))
                || matches!(&wire.validation_failure, NullableField::Present(_));
        let all_structured_fields_present =
            matches!(&wire.output_configuration, NullableField::Present(_))
                && matches!(&wire.active_capabilities, RequiredField::Present(_))
                && matches!(&wire.resolved_plan_digest, NullableField::Present(_))
                && matches!(&wire.final_result, NullableField::Present(_))
                && matches!(&wire.validation_failure, NullableField::Present(_));
        if wire.state_version < 4 && structured_fields_present {
            return Err(de::Error::custom(
                "v1/v2/v3 kernel state contains structured-output fields",
            ));
        }
        if wire.state_version >= 4 && !all_structured_fields_present {
            return Err(de::Error::custom(
                "v4+ kernel state is missing structured-output fields",
            ));
        }
        let composition_fields_present =
            matches!(&wire.child_preparations, RequiredField::Present(_))
                || matches!(&wire.budget_reservations, RequiredField::Present(_))
                || matches!(&wire.budget_charges, RequiredField::Present(_));
        let all_composition_fields_present =
            matches!(&wire.child_preparations, RequiredField::Present(_))
                && matches!(&wire.budget_reservations, RequiredField::Present(_))
                && matches!(&wire.budget_charges, RequiredField::Present(_));
        if wire.state_version < 5 && composition_fields_present {
            return Err(de::Error::custom(
                "v1/v2/v3/v4 kernel state contains composition fields",
            ));
        }
        if wire.state_version >= 5 && !all_composition_fields_present {
            return Err(de::Error::custom(
                "v5+ kernel state is missing composition fields",
            ));
        }
        let interaction_fields_present =
            matches!(&wire.pending_interaction, NullableField::Present(_))
                || matches!(&wire.resolution_identities, RequiredField::Present(_))
                || matches!(&wire.last_interaction_terminal, NullableField::Present(_));
        let all_interaction_fields_present =
            matches!(&wire.pending_interaction, NullableField::Present(_))
                && matches!(&wire.resolution_identities, RequiredField::Present(_))
                && matches!(&wire.last_interaction_terminal, NullableField::Present(_));
        if wire.state_version < 6 && interaction_fields_present {
            return Err(de::Error::custom(
                "v1-v5 kernel state contains interaction fields",
            ));
        }
        if wire.state_version == 6 && !all_interaction_fields_present {
            return Err(de::Error::custom(
                "v6 kernel state is missing interaction fields",
            ));
        }
        let mut stage_settlements = BTreeMap::new();
        for entry in wire.stage_settlements.into_inner() {
            if stage_settlements
                .insert(
                    StageCursor {
                        cycle: entry.cycle,
                        stage: entry.stage,
                    },
                    entry.settlement_digest,
                )
                .is_some()
            {
                return Err(de::Error::custom("duplicate stage settlement key"));
            }
        }
        let mut model_settlements = BTreeMap::new();
        for entry in wire.model_settlements.into_inner() {
            if model_settlements
                .insert(
                    entry.effect_id,
                    ModelSettlementFingerprint {
                        kind: entry.kind,
                        digest: entry.settlement_digest,
                    },
                )
                .is_some()
            {
                return Err(de::Error::custom("duplicate model settlement key"));
            }
        }
        let mut completion_identities = BTreeMap::new();
        for entry in wire.completion_identities.into_inner() {
            if completion_identities
                .insert(
                    entry.completion_id,
                    CompletionIdentity {
                        effect_id: entry.effect_id,
                        settlement_digest: entry.settlement_digest,
                    },
                )
                .is_some()
            {
                return Err(de::Error::custom("duplicate completion identity"));
            }
        }
        let mut tool_calls = BTreeMap::new();
        let tool_call_entries = match wire.tool_calls {
            RequiredField::Missing => Vec::new(),
            RequiredField::Present(entries) => entries.into_inner(),
        };
        for entry in tool_call_entries {
            if tool_calls
                .insert(
                    entry.tool_call_id,
                    ToolCallIdentity {
                        cycle: entry.cycle,
                        turn_id: entry.turn_id,
                        source_message_id: entry.source_message_id,
                        tool_batch_id: entry.tool_batch_id,
                        effect_id: entry.effect_id,
                        call: entry.call,
                    },
                )
                .is_some()
            {
                return Err(de::Error::custom("duplicate tool call identity"));
            }
        }
        let mut tool_settlements = BTreeMap::new();
        let tool_settlement_entries = match wire.tool_settlements {
            RequiredField::Missing => Vec::new(),
            RequiredField::Present(entries) => entries.into_inner(),
        };
        for entry in tool_settlement_entries {
            if tool_settlements
                .insert(
                    entry.effect_id,
                    ToolSettlementFingerprint {
                        kind: entry.kind,
                        digest: entry.settlement_digest,
                    },
                )
                .is_some()
            {
                return Err(de::Error::custom("duplicate tool settlement identity"));
            }
        }
        let mut child_preparations = BTreeMap::new();
        let child_entries = match wire.child_preparations {
            RequiredField::Missing => Vec::new(),
            RequiredField::Present(entries) => entries.into_inner(),
        };
        for entry in child_entries {
            if child_preparations
                .insert(entry.parent_effect_id, entry)
                .is_some()
            {
                return Err(de::Error::custom("duplicate child preparation identity"));
            }
        }
        let mut budget_reservations = BTreeMap::new();
        let reservation_entries = match wire.budget_reservations {
            RequiredField::Missing => Vec::new(),
            RequiredField::Present(entries) => entries.into_inner(),
        };
        for entry in reservation_entries {
            if budget_reservations
                .insert(entry.request.reservation_id, entry)
                .is_some()
            {
                return Err(de::Error::custom("duplicate budget reservation identity"));
            }
        }
        let mut budget_charges = BTreeMap::new();
        let charge_entries = match wire.budget_charges {
            RequiredField::Missing => Vec::new(),
            RequiredField::Present(entries) => entries.into_inner(),
        };
        for entry in charge_entries {
            if budget_charges.insert(entry.effect_id, entry).is_some() {
                return Err(de::Error::custom("duplicate budget charge identity"));
            }
        }
        let mut resolution_identities = BTreeMap::new();
        let resolution_entries = match wire.resolution_identities {
            RequiredField::Missing => Vec::new(),
            RequiredField::Present(entries) => entries.into_inner(),
        };
        for entry in resolution_entries {
            if resolution_identities
                .insert(
                    entry.resolution_id,
                    ResolutionIdentity {
                        interaction_id: entry.interaction_id,
                        settlement_digest: entry.settlement_digest,
                    },
                )
                .is_some()
            {
                return Err(de::Error::custom("duplicate resolution identity"));
            }
        }
        let accepted_at = if wire.state_version >= 3 {
            match wire.accepted_at {
                NullableField::Missing => None,
                NullableField::Present(value) => value,
            }
        } else {
            wire.snapshot_accepted_at
        };
        let limit_usage = if wire.state_version >= 3 {
            match wire.limit_usage {
                RequiredField::Missing => LimitUsage::default(),
                RequiredField::Present(value) => value,
            }
        } else {
            wire.snapshot_limit_usage.unwrap_or_default()
        };
        let state = Self {
            state_version: wire.state_version,
            last_applied_sequence: wire.last_applied_sequence,
            session_id: wire.session_id,
            lane_id: wire.lane_id,
            accepted: wire.accepted,
            accepted_at,
            phase: wire.phase,
            cycle: wire.cycle,
            current_turn: wire.current_turn,
            messages: wire.messages.into_inner().into(),
            pending_model_effect: wire.pending_model_effect,
            terminal_candidate: wire.terminal_candidate,
            stage_settlements,
            model_settlements,
            completion_identities,
            pending_interaction: match wire.pending_interaction {
                NullableField::Missing => None,
                NullableField::Present(value) => value,
            },
            resolution_identities,
            last_interaction_terminal: match wire.last_interaction_terminal {
                NullableField::Missing => None,
                NullableField::Present(value) => value,
            },
            active_tool_batch: match wire.active_tool_batch {
                NullableField::Missing => None,
                NullableField::Present(value) => value,
            },
            tool_calls,
            tool_settlements,
            last_tool_batch: match wire.last_tool_batch {
                NullableField::Missing => None,
                NullableField::Present(value) => value,
            },
            limit_usage,
            cancellation: match wire.cancellation {
                NullableField::Missing => None,
                NullableField::Present(value) => value,
            },
            retry: match wire.retry {
                RequiredField::Missing => RetryState::default(),
                RequiredField::Present(value) => value,
            },
            last_limit: match wire.last_limit {
                NullableField::Missing => None,
                NullableField::Present(value) => value,
            },
            suspension: match wire.suspension {
                NullableField::Missing => None,
                NullableField::Present(value) => value,
            },
            output_configuration: match wire.output_configuration {
                NullableField::Missing => None,
                NullableField::Present(value) => value,
            },
            active_capabilities: match wire.active_capabilities {
                RequiredField::Missing => Arc::from([]),
                RequiredField::Present(value) => Arc::from(value.into_inner()),
            },
            resolved_plan_digest: match wire.resolved_plan_digest {
                NullableField::Missing => None,
                NullableField::Present(value) => value,
            },
            final_result: match wire.final_result {
                NullableField::Missing => None,
                NullableField::Present(value) => value,
            },
            validation_failure: match wire.validation_failure {
                NullableField::Missing => None,
                NullableField::Present(value) => value,
            },
            child_preparations,
            budget_reservations,
            budget_charges,
            terminal: wire.terminal,
        };
        state.validate().map_err(de::Error::custom)?;
        Ok(state)
    }
}

fn v1_v2_snapshot_limit_usage(usage: &LimitUsage) -> Option<&LimitUsage> {
    (*usage != LimitUsage::default()).then_some(usage)
}
