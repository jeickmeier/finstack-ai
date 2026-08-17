//! Authoritative replay-derived kernel state.

mod env;
mod hash_entries;
mod hash_projection;
pub(crate) mod projection;
mod types;
mod validate;
mod wire;

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::conversation::Message;
use crate::primitives::Digest;
use crate::primitives::Timestamp;
use crate::primitives::{BudgetReservationId, EffectId, LaneId, SessionId, ToolCallId};
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
use crate::reducer::KernelError;

use hash_entries::{
    completion_hash_entries, model_hash_entries, resolution_hash_entries, stage_hash_entries,
    tool_call_hash_entries, tool_settlement_hash_entries,
};
use hash_projection::{KernelStateHashV1, KernelStateHashV2};

pub use env::TransitionEnv;
pub use types::{
    BudgetReservationReplay, CancellationState, CompletionIdentity, CompletionIdentityHashEntryV1,
    CurrentTurn, InteractionTerminal, InteractionTerminalOutcome, ModelSettlementFingerprint,
    ModelSettlementHashEntryV1, ModelSettlementKind, PendingInteraction, PendingModelEffect,
    ResolutionIdentity, ResolutionIdentityHashEntryV6, RetryState, RunPhase,
    StageSettlementHashEntryV1, TerminalCandidate, TerminalState, ToolCallIdentityHashEntryV2,
    ToolSettlementHashEntryV2,
};

/// Complete authoritative state derived only from committed records.
#[derive(Debug, Clone)]
pub struct KernelState {
    /// State schema version.
    pub state_version: u16,
    /// Last successfully applied session sequence.
    pub last_applied_sequence: u64,
    /// Immutable accepted session identity.
    pub session_id: Option<SessionId>,
    /// Immutable accepted lane identity.
    pub lane_id: Option<LaneId>,
    /// Immutable accepted run payload.
    pub accepted: Option<RunAccepted>,
    /// Semantic timestamp of the accepted record.
    pub accepted_at: Option<Timestamp>,
    /// Current run phase.
    pub phase: Option<RunPhase>,
    /// Zero-based model cycle.
    pub cycle: u64,
    /// Current turn.
    pub current_turn: Option<CurrentTurn>,
    /// Durable final assistant messages in model-only order.
    ///
    /// `Arc<Vec<_>>` rather than `Arc<[_]>` so appending can reuse the buffer
    /// via [`Arc::make_mut`]. The transactional state clone shares the `Arc`,
    /// so the first append in a batch pays one copy and the rest are amortized
    /// O(1); an `Arc<[_]>` forces a full copy on every single append.
    pub messages: Arc<Vec<Message>>,
    /// Outstanding model effect.
    pub pending_model_effect: Option<PendingModelEffect>,
    /// Candidate gated by `before_finalize`.
    pub terminal_candidate: Option<TerminalCandidate>,
    /// Replay-derived aggregate stage settlement index.
    pub stage_settlements: BTreeMap<StageCursor, Digest>,
    /// Replay-derived terminal model settlement index.
    pub model_settlements: BTreeMap<EffectId, ModelSettlementFingerprint>,
    /// Replay-derived external completion identity index.
    pub completion_identities: BTreeMap<Arc<str>, CompletionIdentity>,
    /// Outstanding typed interaction.
    pub pending_interaction: Option<PendingInteraction>,
    /// Replay-derived interaction resolution identity index.
    pub resolution_identities: BTreeMap<Arc<str>, ResolutionIdentity>,
    /// Most recently settled interaction, when present.
    pub last_interaction_terminal: Option<InteractionTerminal>,
    /// Active source-ordered tool batch.
    pub active_tool_batch: Option<ActiveToolBatch>,
    /// Persistent source call identities.
    pub tool_calls: BTreeMap<ToolCallId, ToolCallIdentity>,
    /// Replay-derived terminal tool settlement index.
    pub tool_settlements: BTreeMap<EffectId, ToolSettlementFingerprint>,
    /// Most recently closed batch awaiting after-tool settlement.
    pub last_tool_batch: Option<ToolBatchClosed>,
    /// Frozen run-level output contract, when explicitly configured.
    pub output_configuration: Option<OutputConfiguration>,
    /// Complete sorted active capability set.
    pub active_capabilities: Arc<[ActiveCapability]>,
    /// Digest of the current immutable resolved run plan.
    pub resolved_plan_digest: Option<Digest>,
    /// Most recent valid structured final result.
    pub final_result: Option<FinalResultRecorded>,
    /// Most recent invalid structured result and retry feedback.
    pub validation_failure: Option<OutputValidationFailed>,
    /// Parent-owned child mappings indexed by parent effect.
    pub child_preparations: BTreeMap<EffectId, ChildRunPrepared>,
    /// Shared-budget reservation lifecycle indexed by reservation identity.
    pub budget_reservations: BTreeMap<BudgetReservationId, BudgetReservationReplay>,
    /// Shared-budget charge receipts indexed by settled effect identity.
    pub budget_charges: BTreeMap<EffectId, BudgetChargeReceipt>,
    /// Replay-derived cumulative limit usage.
    pub limit_usage: LimitUsage,
    /// Active or completed cancellation control state.
    pub cancellation: Option<CancellationState>,
    /// Replay-derived semantic retry state.
    pub retry: RetryState,
    /// Most recent reached limit.
    pub last_limit: Option<LimitReached>,
    /// Active suspension payload.
    pub suspension: Option<RunSuspended>,
    /// Applied terminal payload.
    pub terminal: Option<TerminalState>,
}

impl PartialEq for KernelState {
    /// Wire equality via JCS of `Serialize` (`KernelStateWireV*`), not digest
    /// equality. [`KernelState::state_hash`] uses `KernelStateHashV*`. Tests
    /// that care about snapshots must call `state_hash()`.
    ///
    /// A failed projection compares unequal: the previous `.ok() == .ok()`
    /// form reported two *unserializable* states as equal, which is exactly
    /// backwards for a fail-closed boundary.
    fn eq(&self, other: &Self) -> bool {
        match (
            serde_json_canonicalizer::to_vec(self),
            serde_json_canonicalizer::to_vec(other),
        ) {
            (Ok(left), Ok(right)) => left == right,
            _ => false,
        }
    }
}

impl Eq for KernelState {}

impl Default for KernelState {
    fn default() -> Self {
        Self {
            state_version: 1,
            last_applied_sequence: 0,
            session_id: None,
            lane_id: None,
            accepted: None,
            accepted_at: None,
            phase: None,
            cycle: 0,
            current_turn: None,
            messages: Arc::new(Vec::new()),
            pending_model_effect: None,
            terminal_candidate: None,
            stage_settlements: BTreeMap::new(),
            model_settlements: BTreeMap::new(),
            completion_identities: BTreeMap::new(),
            pending_interaction: None,
            resolution_identities: BTreeMap::new(),
            last_interaction_terminal: None,
            active_tool_batch: None,
            tool_calls: BTreeMap::new(),
            tool_settlements: BTreeMap::new(),
            last_tool_batch: None,
            output_configuration: None,
            active_capabilities: Arc::from([]),
            resolved_plan_digest: None,
            final_result: None,
            validation_failure: None,
            child_preparations: BTreeMap::new(),
            budget_reservations: BTreeMap::new(),
            budget_charges: BTreeMap::new(),
            limit_usage: LimitUsage::default(),
            cancellation: None,
            retry: RetryState::default(),
            last_limit: None,
            suspension: None,
            terminal: None,
        }
    }
}

impl KernelState {
    /// Compute the exact schema-1 JCS state digest under `kernel-state`.
    ///
    /// # Errors
    ///
    /// Returns [`KernelError::StateHashFailed`] if the internal projection cannot
    /// be represented or canonicalized as JSON.
    pub fn state_hash(&self) -> Result<Digest, KernelError> {
        self.validate().map_err(|_| KernelError::StateHashFailed)?;
        // Streamed into the hasher rather than canonicalized into a `Vec`: the
        // state projection is the largest single canonical payload the kernel
        // produces, and buffering it grew a fresh allocation every call.
        let mut writer =
            crate::primitives::DigestWriter::new("kernel-state", u32::from(self.state_version))
                .map_err(|_| KernelError::StateHashFailed)?;
        if self.state_version == 1 {
            serde_json_canonicalizer::to_writer(
                &KernelStateHashV1::from_state(
                    self,
                    stage_hash_entries(&self.stage_settlements),
                    model_hash_entries(&self.model_settlements),
                    completion_hash_entries(&self.completion_identities),
                ),
                &mut writer,
            )
        } else if self.state_version == 2 {
            serde_json_canonicalizer::to_writer(
                &KernelStateHashV2::from_state(
                    self,
                    stage_hash_entries(&self.stage_settlements),
                    model_hash_entries(&self.model_settlements),
                    completion_hash_entries(&self.completion_identities),
                    tool_call_hash_entries(&self.tool_calls),
                    tool_settlement_hash_entries(&self.tool_settlements),
                ),
                &mut writer,
            )
        } else if self.state_version == 3 {
            serde_json_canonicalizer::to_writer(
                &hash_projection::KernelStateHashV3::from_state(
                    self,
                    stage_hash_entries(&self.stage_settlements),
                    model_hash_entries(&self.model_settlements),
                    completion_hash_entries(&self.completion_identities),
                    tool_call_hash_entries(&self.tool_calls),
                    tool_settlement_hash_entries(&self.tool_settlements),
                ),
                &mut writer,
            )
        } else if self.state_version == 4 {
            serde_json_canonicalizer::to_writer(
                &hash_projection::KernelStateHashV4::from_state(
                    self,
                    stage_hash_entries(&self.stage_settlements),
                    model_hash_entries(&self.model_settlements),
                    completion_hash_entries(&self.completion_identities),
                    tool_call_hash_entries(&self.tool_calls),
                    tool_settlement_hash_entries(&self.tool_settlements),
                ),
                &mut writer,
            )
        } else if self.state_version == 5 {
            serde_json_canonicalizer::to_writer(
                &hash_projection::KernelStateHashV5::from_state(
                    self,
                    stage_hash_entries(&self.stage_settlements),
                    model_hash_entries(&self.model_settlements),
                    completion_hash_entries(&self.completion_identities),
                    tool_call_hash_entries(&self.tool_calls),
                    tool_settlement_hash_entries(&self.tool_settlements),
                ),
                &mut writer,
            )
        } else {
            serde_json_canonicalizer::to_writer(
                &hash_projection::KernelStateHashV6::from_state(
                    self,
                    stage_hash_entries(&self.stage_settlements),
                    model_hash_entries(&self.model_settlements),
                    completion_hash_entries(&self.completion_identities),
                    tool_call_hash_entries(&self.tool_calls),
                    tool_settlement_hash_entries(&self.tool_settlements),
                    resolution_hash_entries(&self.resolution_identities),
                ),
                &mut writer,
            )
        }
        .map_err(|_| KernelError::StateHashFailed)?;
        Ok(writer.finish().0)
    }
}
