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
    completion_hash_entries, extension_hash_entries, model_hash_entries, resolution_hash_entries,
    stage_hash_entries, tool_call_hash_entries, tool_settlement_hash_entries,
};
use hash_projection::{KernelStateHashV1, KernelStateHashV2};

pub use env::TransitionEnv;
pub use types::{
    BudgetReservationReplay, CancellationState, CompletionIdentity, CurrentTurn,
    ExtensionSettlementFingerprint, ExtensionSettlementKind, InteractionTerminal,
    InteractionTerminalOutcome, ModelSettlementFingerprint, ModelSettlementKind,
    PendingExtensionEffect, PendingInteraction, PendingModelEffect, ResolutionIdentity, RetryState,
    RunPhase, TerminalCandidate, TerminalState,
};

/// Complete authoritative state derived only from committed records.
#[derive(Debug, Clone)]
pub struct KernelState {
    /// State schema version.
    pub(crate) state_version: u16,
    /// Last successfully applied session sequence.
    pub(crate) last_applied_sequence: u64,
    /// Immutable accepted session identity.
    pub(crate) session_id: Option<SessionId>,
    /// Immutable accepted lane identity.
    pub(crate) lane_id: Option<LaneId>,
    /// Immutable accepted run payload.
    pub(crate) accepted: Option<RunAccepted>,
    /// Semantic timestamp of the accepted record.
    pub(crate) accepted_at: Option<Timestamp>,
    /// Current run phase.
    pub(crate) phase: Option<RunPhase>,
    /// Zero-based model cycle.
    pub(crate) cycle: u64,
    /// Current turn.
    pub(crate) current_turn: Option<CurrentTurn>,
    /// Durable final assistant messages in model-only order.
    ///
    /// `Arc<Vec<_>>` rather than `Arc<[_]>` so appending can reuse the buffer
    /// via [`Arc::make_mut`]. The transactional state clone shares the `Arc`,
    /// so the first append in a batch pays one copy and the rest are amortized
    /// O(1); an `Arc<[_]>` forces a full copy on every single append.
    pub(crate) messages: Arc<Vec<Message>>,
    /// Outstanding model effect.
    pub(crate) pending_model_effect: Option<PendingModelEffect>,
    /// Outstanding context-provider or middleware effect.
    pub(crate) pending_extension_effect: Option<PendingExtensionEffect>,
    /// Candidate gated by `before_finalize`.
    pub(crate) terminal_candidate: Option<TerminalCandidate>,
    /// Replay-derived aggregate stage settlement index.
    pub(crate) stage_settlements: BTreeMap<StageCursor, Digest>,
    /// Replay-derived terminal model settlement index.
    pub(crate) model_settlements: BTreeMap<EffectId, ModelSettlementFingerprint>,
    /// Replay-derived terminal extension settlement index.
    pub(crate) extension_settlements: BTreeMap<EffectId, ExtensionSettlementFingerprint>,
    /// Replay-derived external completion identity index.
    pub(crate) completion_identities: BTreeMap<Arc<str>, CompletionIdentity>,
    /// Outstanding typed interaction.
    pub(crate) pending_interaction: Option<PendingInteraction>,
    /// Replay-derived interaction resolution identity index.
    pub(crate) resolution_identities: BTreeMap<Arc<str>, ResolutionIdentity>,
    /// Most recently settled interaction, when present.
    pub(crate) last_interaction_terminal: Option<InteractionTerminal>,
    /// Active source-ordered tool batch.
    pub(crate) active_tool_batch: Option<ActiveToolBatch>,
    /// Persistent source call identities.
    pub(crate) tool_calls: BTreeMap<ToolCallId, ToolCallIdentity>,
    /// Replay-derived terminal tool settlement index.
    pub(crate) tool_settlements: BTreeMap<EffectId, ToolSettlementFingerprint>,
    /// Most recently closed batch awaiting after-tool settlement.
    pub(crate) last_tool_batch: Option<ToolBatchClosed>,
    /// Frozen run-level output contract, when explicitly configured.
    pub(crate) output_configuration: Option<OutputConfiguration>,
    /// Complete sorted active capability set.
    pub(crate) active_capabilities: Arc<[ActiveCapability]>,
    /// Digest of the current immutable resolved run plan.
    pub(crate) resolved_plan_digest: Option<Digest>,
    /// Most recent valid structured final result.
    pub(crate) final_result: Option<FinalResultRecorded>,
    /// Most recent invalid structured result and retry feedback.
    pub(crate) validation_failure: Option<OutputValidationFailed>,
    /// Parent-owned child mappings indexed by parent effect.
    pub(crate) child_preparations: BTreeMap<EffectId, ChildRunPrepared>,
    /// Shared-budget reservation lifecycle indexed by reservation identity.
    pub(crate) budget_reservations: BTreeMap<BudgetReservationId, BudgetReservationReplay>,
    /// Shared-budget charge receipts indexed by settled effect identity.
    pub(crate) budget_charges: BTreeMap<EffectId, BudgetChargeReceipt>,
    /// Replay-derived cumulative limit usage.
    pub(crate) limit_usage: LimitUsage,
    /// Active or completed cancellation control state.
    pub(crate) cancellation: Option<CancellationState>,
    /// Replay-derived semantic retry state.
    pub(crate) retry: RetryState,
    /// Most recent reached limit.
    pub(crate) last_limit: Option<LimitReached>,
    /// Active suspension payload.
    pub(crate) suspension: Option<RunSuspended>,
    /// Applied terminal payload.
    pub(crate) terminal: Option<TerminalState>,
}

/// Read-only access to replay-derived state.
///
/// Fields are crate-private so every mutation flows through the reducer's
/// validated decide/apply path; consumers read through these accessors.
impl KernelState {
    /// State schema version.
    #[must_use]
    pub const fn state_version(&self) -> u16 {
        self.state_version
    }

    /// Last successfully applied session sequence.
    #[must_use]
    pub const fn last_applied_sequence(&self) -> u64 {
        self.last_applied_sequence
    }

    /// Immutable accepted session identity.
    #[must_use]
    pub const fn session_id(&self) -> Option<SessionId> {
        self.session_id
    }

    /// Immutable accepted lane identity.
    #[must_use]
    pub const fn lane_id(&self) -> Option<LaneId> {
        self.lane_id
    }

    /// Immutable accepted run payload.
    #[must_use]
    pub fn accepted(&self) -> Option<&RunAccepted> {
        self.accepted.as_ref()
    }

    /// Semantic timestamp of the accepted record.
    #[must_use]
    pub const fn accepted_at(&self) -> Option<Timestamp> {
        self.accepted_at
    }

    /// Current run phase.
    #[must_use]
    pub const fn phase(&self) -> Option<RunPhase> {
        self.phase
    }

    /// Zero-based model cycle.
    #[must_use]
    pub const fn cycle(&self) -> u64 {
        self.cycle
    }

    /// Current turn.
    #[must_use]
    pub fn current_turn(&self) -> Option<&CurrentTurn> {
        self.current_turn.as_ref()
    }

    /// Durable final assistant messages in model-only order.
    #[must_use]
    pub const fn messages(&self) -> &Arc<Vec<Message>> {
        &self.messages
    }

    /// Outstanding model effect.
    #[must_use]
    pub fn pending_model_effect(&self) -> Option<&PendingModelEffect> {
        self.pending_model_effect.as_ref()
    }

    /// Outstanding context-provider or middleware effect.
    #[must_use]
    pub fn pending_extension_effect(&self) -> Option<&PendingExtensionEffect> {
        self.pending_extension_effect.as_ref()
    }

    /// Candidate gated by `before_finalize`.
    #[must_use]
    pub fn terminal_candidate(&self) -> Option<&TerminalCandidate> {
        self.terminal_candidate.as_ref()
    }

    /// Replay-derived aggregate stage settlement index.
    #[must_use]
    pub const fn stage_settlements(&self) -> &BTreeMap<StageCursor, Digest> {
        &self.stage_settlements
    }

    /// Replay-derived terminal model settlement index.
    #[must_use]
    pub const fn model_settlements(&self) -> &BTreeMap<EffectId, ModelSettlementFingerprint> {
        &self.model_settlements
    }

    /// Replay-derived terminal extension settlement index.
    #[must_use]
    pub const fn extension_settlements(
        &self,
    ) -> &BTreeMap<EffectId, ExtensionSettlementFingerprint> {
        &self.extension_settlements
    }

    /// Replay-derived external completion identity index.
    #[must_use]
    pub const fn completion_identities(&self) -> &BTreeMap<Arc<str>, CompletionIdentity> {
        &self.completion_identities
    }

    /// Outstanding typed interaction.
    #[must_use]
    pub fn pending_interaction(&self) -> Option<&PendingInteraction> {
        self.pending_interaction.as_ref()
    }

    /// Replay-derived interaction resolution identity index.
    #[must_use]
    pub const fn resolution_identities(&self) -> &BTreeMap<Arc<str>, ResolutionIdentity> {
        &self.resolution_identities
    }

    /// Most recently settled interaction, when present.
    #[must_use]
    pub fn last_interaction_terminal(&self) -> Option<&InteractionTerminal> {
        self.last_interaction_terminal.as_ref()
    }

    /// Active source-ordered tool batch.
    #[must_use]
    pub fn active_tool_batch(&self) -> Option<&ActiveToolBatch> {
        self.active_tool_batch.as_ref()
    }

    /// Persistent source call identities.
    #[must_use]
    pub const fn tool_calls(&self) -> &BTreeMap<ToolCallId, ToolCallIdentity> {
        &self.tool_calls
    }

    /// Replay-derived terminal tool settlement index.
    #[must_use]
    pub const fn tool_settlements(&self) -> &BTreeMap<EffectId, ToolSettlementFingerprint> {
        &self.tool_settlements
    }

    /// Most recently closed batch awaiting after-tool settlement.
    #[must_use]
    pub fn last_tool_batch(&self) -> Option<&ToolBatchClosed> {
        self.last_tool_batch.as_ref()
    }

    /// Frozen run-level output contract, when explicitly configured.
    #[must_use]
    pub fn output_configuration(&self) -> Option<&OutputConfiguration> {
        self.output_configuration.as_ref()
    }

    /// Complete sorted active capability set.
    #[must_use]
    pub const fn active_capabilities(&self) -> &Arc<[ActiveCapability]> {
        &self.active_capabilities
    }

    /// Digest of the current immutable resolved run plan.
    #[must_use]
    pub const fn resolved_plan_digest(&self) -> Option<Digest> {
        self.resolved_plan_digest
    }

    /// Most recent valid structured final result.
    #[must_use]
    pub fn final_result(&self) -> Option<&FinalResultRecorded> {
        self.final_result.as_ref()
    }

    /// Most recent invalid structured result and retry feedback.
    #[must_use]
    pub fn validation_failure(&self) -> Option<&OutputValidationFailed> {
        self.validation_failure.as_ref()
    }

    /// Parent-owned child mappings indexed by parent effect.
    #[must_use]
    pub const fn child_preparations(&self) -> &BTreeMap<EffectId, ChildRunPrepared> {
        &self.child_preparations
    }

    /// Shared-budget reservation lifecycle indexed by reservation identity.
    #[must_use]
    pub const fn budget_reservations(
        &self,
    ) -> &BTreeMap<BudgetReservationId, BudgetReservationReplay> {
        &self.budget_reservations
    }

    /// Shared-budget charge receipts indexed by settled effect identity.
    #[must_use]
    pub const fn budget_charges(&self) -> &BTreeMap<EffectId, BudgetChargeReceipt> {
        &self.budget_charges
    }

    /// Replay-derived cumulative limit usage.
    #[must_use]
    pub const fn limit_usage(&self) -> &LimitUsage {
        &self.limit_usage
    }

    /// Active or completed cancellation control state.
    #[must_use]
    pub fn cancellation(&self) -> Option<&CancellationState> {
        self.cancellation.as_ref()
    }

    /// Replay-derived semantic retry state.
    #[must_use]
    pub const fn retry(&self) -> &RetryState {
        &self.retry
    }

    /// Most recent reached limit.
    #[must_use]
    pub fn last_limit(&self) -> Option<&LimitReached> {
        self.last_limit.as_ref()
    }

    /// Active suspension payload.
    #[must_use]
    pub fn suspension(&self) -> Option<&RunSuspended> {
        self.suspension.as_ref()
    }

    /// Applied terminal payload.
    #[must_use]
    pub fn terminal(&self) -> Option<&TerminalState> {
        self.terminal.as_ref()
    }
}

/// Test-fixture mutators.
///
/// These exist only so integration tests, benches, and codec fixtures in
/// other workspace crates can assemble representative states. They are not
/// part of the semantic API: production state changes flow exclusively
/// through the reducer's decide/apply path.
#[doc(hidden)]
impl KernelState {
    pub fn set_state_version(&mut self, state_version: u16) {
        self.state_version = state_version;
    }

    pub fn set_last_applied_sequence(&mut self, last_applied_sequence: u64) {
        self.last_applied_sequence = last_applied_sequence;
    }

    pub fn set_session_id(&mut self, session_id: Option<SessionId>) {
        self.session_id = session_id;
    }

    pub fn set_lane_id(&mut self, lane_id: Option<LaneId>) {
        self.lane_id = lane_id;
    }

    pub fn set_accepted(&mut self, accepted: Option<RunAccepted>) {
        self.accepted = accepted;
    }

    pub fn set_accepted_at(&mut self, accepted_at: Option<Timestamp>) {
        self.accepted_at = accepted_at;
    }

    pub fn set_phase(&mut self, phase: Option<RunPhase>) {
        self.phase = phase;
    }

    pub fn set_messages(&mut self, messages: Arc<Vec<Message>>) {
        self.messages = messages;
    }

    pub fn set_tool_calls(&mut self, tool_calls: BTreeMap<ToolCallId, ToolCallIdentity>) {
        self.tool_calls = tool_calls;
    }

    pub fn set_cycle(&mut self, cycle: u64) {
        self.cycle = cycle;
    }

    pub fn set_current_turn(&mut self, current_turn: Option<CurrentTurn>) {
        self.current_turn = current_turn;
    }

    pub fn set_pending_model_effect(&mut self, pending: Option<PendingModelEffect>) {
        self.pending_model_effect = pending;
    }

    pub fn set_pending_extension_effect(&mut self, pending: Option<PendingExtensionEffect>) {
        self.pending_extension_effect = pending;
    }

    pub fn set_terminal_candidate(&mut self, candidate: Option<TerminalCandidate>) {
        self.terminal_candidate = candidate;
    }

    pub fn set_pending_interaction(&mut self, pending: Option<PendingInteraction>) {
        self.pending_interaction = pending;
    }

    pub fn set_last_interaction_terminal(&mut self, terminal: Option<InteractionTerminal>) {
        self.last_interaction_terminal = terminal;
    }

    pub fn set_active_tool_batch(&mut self, batch: Option<ActiveToolBatch>) {
        self.active_tool_batch = batch;
    }

    pub fn set_tool_settlements(
        &mut self,
        settlements: BTreeMap<EffectId, ToolSettlementFingerprint>,
    ) {
        self.tool_settlements = settlements;
    }

    pub fn set_last_tool_batch(&mut self, batch: Option<ToolBatchClosed>) {
        self.last_tool_batch = batch;
    }

    pub fn set_output_configuration(&mut self, configuration: Option<OutputConfiguration>) {
        self.output_configuration = configuration;
    }

    pub fn set_active_capabilities(&mut self, capabilities: Arc<[ActiveCapability]>) {
        self.active_capabilities = capabilities;
    }

    pub fn set_resolved_plan_digest(&mut self, digest: Option<Digest>) {
        self.resolved_plan_digest = digest;
    }

    pub fn set_final_result(&mut self, final_result: Option<FinalResultRecorded>) {
        self.final_result = final_result;
    }

    pub fn set_validation_failure(&mut self, failure: Option<OutputValidationFailed>) {
        self.validation_failure = failure;
    }

    pub fn set_completion_identities(
        &mut self,
        identities: BTreeMap<Arc<str>, CompletionIdentity>,
    ) {
        self.completion_identities = identities;
    }

    pub fn set_resolution_identities(
        &mut self,
        identities: BTreeMap<Arc<str>, ResolutionIdentity>,
    ) {
        self.resolution_identities = identities;
    }

    pub fn set_stage_settlements(&mut self, settlements: BTreeMap<StageCursor, Digest>) {
        self.stage_settlements = settlements;
    }

    pub fn set_model_settlements(
        &mut self,
        settlements: BTreeMap<EffectId, ModelSettlementFingerprint>,
    ) {
        self.model_settlements = settlements;
    }

    pub fn set_extension_settlements(
        &mut self,
        settlements: BTreeMap<EffectId, ExtensionSettlementFingerprint>,
    ) {
        self.extension_settlements = settlements;
    }

    pub fn set_child_preparations(&mut self, preparations: BTreeMap<EffectId, ChildRunPrepared>) {
        self.child_preparations = preparations;
    }

    pub fn set_budget_reservations(
        &mut self,
        reservations: BTreeMap<BudgetReservationId, BudgetReservationReplay>,
    ) {
        self.budget_reservations = reservations;
    }

    pub fn set_budget_charges(&mut self, charges: BTreeMap<EffectId, BudgetChargeReceipt>) {
        self.budget_charges = charges;
    }

    pub fn set_limit_usage(&mut self, usage: LimitUsage) {
        self.limit_usage = usage;
    }

    pub fn set_cancellation(&mut self, cancellation: Option<CancellationState>) {
        self.cancellation = cancellation;
    }

    pub fn set_retry(&mut self, retry: RetryState) {
        self.retry = retry;
    }

    pub fn set_last_limit(&mut self, limit: Option<LimitReached>) {
        self.last_limit = limit;
    }

    pub fn set_suspension(&mut self, suspension: Option<RunSuspended>) {
        self.suspension = suspension;
    }

    pub fn set_terminal(&mut self, terminal: Option<TerminalState>) {
        self.terminal = terminal;
    }
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
            pending_extension_effect: None,
            terminal_candidate: None,
            stage_settlements: BTreeMap::new(),
            model_settlements: BTreeMap::new(),
            extension_settlements: BTreeMap::new(),
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
    /// Compute the versioned JCS state digest under domain `kernel-state`.
    ///
    /// The projection schema is selected by [`Self::state_version`] and spans
    /// versions 1–7.
    ///
    /// # Errors
    ///
    /// Returns [`KernelError::StateHashFailed`] if the internal projection cannot
    /// be represented or canonicalized as JSON.
    pub fn state_hash(&self) -> Result<Digest, KernelError> {
        self.validate().map_err(|_| KernelError::StateHashFailed)?;
        self.hash_projection()
    }

    /// Stream the versioned hash projection without a second `validate`.
    ///
    /// Callers that already validated (restore, apply-adjacent hashing) use this
    /// so digest bytes stay identical to [`Self::state_hash`] without walking
    /// tool/message invariants twice.
    pub(crate) fn hash_projection(&self) -> Result<Digest, KernelError> {
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
        } else if self.state_version == 6 {
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
        } else {
            serde_json_canonicalizer::to_writer(
                &hash_projection::KernelStateHashV7::from_state(
                    self,
                    stage_hash_entries(&self.stage_settlements),
                    model_hash_entries(&self.model_settlements),
                    completion_hash_entries(&self.completion_identities),
                    tool_call_hash_entries(&self.tool_calls),
                    tool_settlement_hash_entries(&self.tool_settlements),
                    resolution_hash_entries(&self.resolution_identities),
                    extension_hash_entries(&self.extension_settlements),
                ),
                &mut writer,
            )
        }
        .map_err(|_| KernelError::StateHashFailed)?;
        Ok(writer.finish().0)
    }
}
