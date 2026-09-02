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
    completion_hash_entries, extension_hash_entries, model_hash_entries, resolution_hash_entries,
    stage_hash_entries, tool_call_hash_entries, tool_settlement_hash_entries,
};
use super::types::{
    CompletionIdentityHashEntryV1, ExtensionSettlementHashEntryV7, ModelSettlementHashEntryV1,
    ResolutionIdentityHashEntryV6, StageSettlementHashEntryV1, ToolCallIdentityHashEntryV2,
    ToolCallIdentityHashRef, ToolSettlementHashEntryV2,
};
use super::{
    BudgetReservationReplay, CancellationState, CompletionIdentity, CurrentTurn,
    ExtensionSettlementFingerprint, InteractionTerminal, KernelState, ModelSettlementFingerprint,
    PendingExtensionEffect, PendingInteraction, PendingModelEffect, ResolutionIdentity, RetryState,
    RunPhase, TerminalCandidate, TerminalState,
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
    tool_calls: Vec<ToolCallIdentityHashRef<'a>>,
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
    tool_calls: Vec<ToolCallIdentityHashRef<'a>>,
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
    tool_calls: Vec<ToolCallIdentityHashRef<'a>>,
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

#[derive(Serialize)]
struct KernelStateWireV7<'a> {
    #[serde(flatten)]
    base: KernelStateWireV6<'a>,
    pending_extension_effect: Option<&'a PendingExtensionEffect>,
    extension_settlements: Vec<ExtensionSettlementHashEntryV7>,
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
    pending_extension_effect: NullableField<PendingExtensionEffect>,
    #[serde(default)]
    extension_settlements:
        RequiredField<BoundedVec<ExtensionSettlementHashEntryV7, SEMANTIC_MAP_MAX_ENTRIES>>,
    #[serde(default)]
    terminal: Option<TerminalState>,
    #[serde(default)]
    snapshot_accepted_at: Option<Timestamp>,
    #[serde(default)]
    snapshot_limit_usage: Option<LimitUsage>,
}

/// Field that must be present (non-null) once its state version introduced it.
#[derive(Default)]
enum RequiredField<T> {
    #[default]
    Missing,
    Present(T),
}

impl<T> RequiredField<T> {
    const fn is_present(&self) -> bool {
        matches!(self, Self::Present(_))
    }

    fn unwrap_or_default(self) -> T
    where
        T: Default,
    {
        match self {
            Self::Missing => T::default(),
            Self::Present(value) => value,
        }
    }
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

/// Field that must be present (possibly `null`) once its state version introduced it.
#[derive(Default)]
enum NullableField<T> {
    #[default]
    Missing,
    Present(Option<T>),
}

impl<T> NullableField<T> {
    const fn is_present(&self) -> bool {
        matches!(self, Self::Present(_))
    }

    fn into_option(self) -> Option<T> {
        match self {
            Self::Missing => None,
            Self::Present(value) => value,
        }
    }
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

/// Fields introduced together at `introduced_in` must be absent before that
/// version and all present from it on.
fn check_field_group<E: de::Error>(
    state_version: u16,
    introduced_in: u16,
    present: &[bool],
    unexpected: &'static str,
    missing: &'static str,
) -> Result<(), E> {
    if state_version < introduced_in && present.iter().any(|flag| *flag) {
        return Err(E::custom(unexpected));
    }
    if state_version >= introduced_in && !present.iter().all(|flag| *flag) {
        return Err(E::custom(missing));
    }
    Ok(())
}

/// Index wire entries by key; `None` when a key repeats.
fn indexed<K: Ord, T, V>(entries: Vec<T>, entry: impl Fn(T) -> (K, V)) -> Option<BTreeMap<K, V>> {
    let mut map = BTreeMap::new();
    for item in entries {
        let (key, value) = entry(item);
        if map.insert(key, value).is_some() {
            return None;
        }
    }
    Some(map)
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
                let v6 = KernelStateWireV6 {
                    base,
                    pending_interaction: self.pending_interaction.as_ref(),
                    resolution_identities: resolution_hash_entries(&self.resolution_identities),
                    last_interaction_terminal: self.last_interaction_terminal.as_ref(),
                };
                if self.state_version == 6 {
                    v6.serialize(serializer)
                } else {
                    KernelStateWireV7 {
                        base: v6,
                        pending_extension_effect: self.pending_extension_effect.as_ref(),
                        extension_settlements: extension_hash_entries(&self.extension_settlements),
                    }
                    .serialize(serializer)
                }
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
        let version = wire.state_version;
        if !matches!(version, 1..=7) {
            return Err(de::Error::custom("unsupported kernel state_version"));
        }
        check_field_group(
            version,
            2,
            &[
                wire.active_tool_batch.is_present(),
                wire.tool_calls.is_present(),
                wire.tool_settlements.is_present(),
                wire.last_tool_batch.is_present(),
            ],
            "v1 kernel state contains tool fields",
            "v2 kernel state is missing tool indexes",
        )?;
        check_field_group(
            version,
            3,
            &[
                wire.accepted_at.is_present(),
                wire.limit_usage.is_present(),
                wire.cancellation.is_present(),
                wire.retry.is_present(),
                wire.last_limit.is_present(),
                wire.suspension.is_present(),
            ],
            "v1/v2 kernel state contains control fields",
            "v3/v4 kernel state is missing control fields",
        )?;
        check_field_group(
            version,
            4,
            &[
                wire.output_configuration.is_present(),
                wire.active_capabilities.is_present(),
                wire.resolved_plan_digest.is_present(),
                wire.final_result.is_present(),
                wire.validation_failure.is_present(),
            ],
            "v1/v2/v3 kernel state contains structured-output fields",
            "v4+ kernel state is missing structured-output fields",
        )?;
        check_field_group(
            version,
            5,
            &[
                wire.child_preparations.is_present(),
                wire.budget_reservations.is_present(),
                wire.budget_charges.is_present(),
            ],
            "v1/v2/v3/v4 kernel state contains composition fields",
            "v5+ kernel state is missing composition fields",
        )?;
        check_field_group(
            version,
            6,
            &[
                wire.pending_interaction.is_present(),
                wire.resolution_identities.is_present(),
                wire.last_interaction_terminal.is_present(),
            ],
            "v1-v5 kernel state contains interaction fields",
            "v6 kernel state is missing interaction fields",
        )?;
        check_field_group(
            version,
            7,
            &[
                wire.pending_extension_effect.is_present(),
                wire.extension_settlements.is_present(),
            ],
            "v1-v6 kernel state contains extension fields",
            "v7 kernel state is missing extension fields",
        )?;

        let stage_settlements = indexed(wire.stage_settlements.into_inner(), |entry| {
            (
                StageCursor {
                    cycle: entry.cycle,
                    stage: entry.stage,
                },
                entry.settlement_digest,
            )
        })
        .ok_or_else(|| de::Error::custom("duplicate stage settlement key"))?;
        let model_settlements = indexed(wire.model_settlements.into_inner(), |entry| {
            (
                entry.effect_id,
                ModelSettlementFingerprint {
                    kind: entry.kind,
                    digest: entry.settlement_digest,
                },
            )
        })
        .ok_or_else(|| de::Error::custom("duplicate model settlement key"))?;
        let completion_identities = indexed(wire.completion_identities.into_inner(), |entry| {
            (
                entry.completion_id,
                CompletionIdentity {
                    effect_id: entry.effect_id,
                    settlement_digest: entry.settlement_digest,
                },
            )
        })
        .ok_or_else(|| de::Error::custom("duplicate completion identity"))?;
        let tool_calls = indexed(wire.tool_calls.unwrap_or_default().into_inner(), |entry| {
            (
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
        })
        .ok_or_else(|| de::Error::custom("duplicate tool call identity"))?;
        let tool_settlements = indexed(
            wire.tool_settlements.unwrap_or_default().into_inner(),
            |entry| {
                (
                    entry.effect_id,
                    ToolSettlementFingerprint {
                        kind: entry.kind,
                        digest: entry.settlement_digest,
                    },
                )
            },
        )
        .ok_or_else(|| de::Error::custom("duplicate tool settlement identity"))?;
        let child_preparations = indexed(
            wire.child_preparations.unwrap_or_default().into_inner(),
            |entry| (entry.parent_effect_id, entry),
        )
        .ok_or_else(|| de::Error::custom("duplicate child preparation identity"))?;
        let budget_reservations = indexed(
            wire.budget_reservations.unwrap_or_default().into_inner(),
            |entry| (entry.request.reservation_id, entry),
        )
        .ok_or_else(|| de::Error::custom("duplicate budget reservation identity"))?;
        let budget_charges = indexed(
            wire.budget_charges.unwrap_or_default().into_inner(),
            |entry| (entry.effect_id, entry),
        )
        .ok_or_else(|| de::Error::custom("duplicate budget charge identity"))?;
        let resolution_identities = indexed(
            wire.resolution_identities.unwrap_or_default().into_inner(),
            |entry| {
                (
                    entry.resolution_id,
                    ResolutionIdentity {
                        interaction_id: entry.interaction_id,
                        settlement_digest: entry.settlement_digest,
                    },
                )
            },
        )
        .ok_or_else(|| de::Error::custom("duplicate resolution identity"))?;
        let extension_settlements = indexed(
            wire.extension_settlements.unwrap_or_default().into_inner(),
            |entry| {
                (
                    entry.effect_id,
                    ExtensionSettlementFingerprint {
                        kind: entry.kind,
                        digest: entry.settlement_digest,
                    },
                )
            },
        )
        .ok_or_else(|| de::Error::custom("duplicate extension settlement identity"))?;

        let (accepted_at, limit_usage) = if version >= 3 {
            (
                wire.accepted_at.into_option(),
                wire.limit_usage.unwrap_or_default(),
            )
        } else {
            (
                wire.snapshot_accepted_at,
                wire.snapshot_limit_usage.unwrap_or_default(),
            )
        };
        let state = Self {
            state_version: version,
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
            pending_extension_effect: wire.pending_extension_effect.into_option(),
            terminal_candidate: wire.terminal_candidate,
            stage_settlements,
            model_settlements,
            extension_settlements,
            completion_identities,
            pending_interaction: wire.pending_interaction.into_option(),
            resolution_identities,
            last_interaction_terminal: wire.last_interaction_terminal.into_option(),
            active_tool_batch: wire.active_tool_batch.into_option(),
            tool_calls,
            tool_settlements,
            last_tool_batch: wire.last_tool_batch.into_option(),
            limit_usage,
            cancellation: wire.cancellation.into_option(),
            retry: wire.retry.unwrap_or_default(),
            last_limit: wire.last_limit.into_option(),
            suspension: wire.suspension.into_option(),
            output_configuration: wire.output_configuration.into_option(),
            active_capabilities: Arc::from(
                wire.active_capabilities.unwrap_or_default().into_inner(),
            ),
            resolved_plan_digest: wire.resolved_plan_digest.into_option(),
            final_result: wire.final_result.into_option(),
            validation_failure: wire.validation_failure.into_option(),
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
