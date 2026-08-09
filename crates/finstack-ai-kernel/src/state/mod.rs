//! Authoritative replay-derived kernel state.

mod env;
mod hash_projection;

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::de;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::bounds::{BoundedVec, SEMANTIC_ARRAY_MAX_ITEMS, SEMANTIC_MAP_MAX_ENTRIES};
use crate::content::{BoundedString, LABEL_MAX_BYTES};
use crate::digest::Digest;
use crate::effects::{EffectDeferred, EffectRequested};
use crate::entries::{ContextPrepared, RunCompleted, RunFailed, Stage, StageCursor};
use crate::error::ErrorDescriptor;
use crate::ids::{EffectId, LaneId, MessageId, ModelRequestId, SessionId, TurnId};
use crate::message::Message;
use crate::reducer::KernelError;
use crate::run::RunAccepted;

use hash_projection::KernelStateHashV1;

pub use env::TransitionEnv;

/// Complete frozen run-phase vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunPhase {
    /// Acceptance record applied.
    Accepted,
    /// Aggregate before-run boundary.
    BeforeRun,
    /// Aggregate context preparation.
    PreparingContext,
    /// Aggregate before-model boundary.
    BeforeModel,
    /// A direct model effect is outstanding.
    AwaitingModel,
    /// A completed model response awaits aggregate settlement.
    AfterModel,
    /// Aggregate before-tool-batch boundary.
    BeforeToolBatch,
    /// Tool effects are outstanding.
    AwaitingTools,
    /// Aggregate after-tool-batch boundary.
    AfterToolBatch,
    /// Final behavior-changing boundary.
    BeforeFinalize,
    /// An interaction is outstanding.
    AwaitingInteraction,
    /// Deferred external work is outstanding.
    AwaitingExternal,
    /// A durable timer is outstanding.
    Sleeping,
    /// Cancellation is reconciling.
    Cancelling,
    /// Operator or application action is required.
    Suspended,
    /// Successful terminal.
    Completed,
    /// Failed terminal.
    Failed,
    /// Cancelled terminal.
    Cancelled,
}

/// Replay-derived state for the current model turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CurrentTurn {
    /// Model cycle.
    pub cycle: u64,
    /// Turn identity.
    pub turn_id: TurnId,
    /// Prepared context.
    pub context: ContextPrepared,
    /// Current model request identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_request_id: Option<ModelRequestId>,
    /// Current model effect identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect_id: Option<EffectId>,
    /// Final assistant message identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_message_id: Option<MessageId>,
}

/// Outstanding model effect and optional external deferral.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PendingModelEffect {
    /// Model cycle.
    pub cycle: u64,
    /// Turn identity.
    pub turn_id: TurnId,
    /// Model request identity.
    pub model_request_id: ModelRequestId,
    /// Original effect request.
    pub requested: EffectRequested,
    /// Deferred external handle, when externally suspended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deferred: Option<EffectDeferred>,
}

/// Candidate that must pass `before_finalize` before terminal commitment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
#[expect(
    clippy::large_enum_variant,
    reason = "the frozen public payload stores ErrorDescriptor by value"
)]
pub enum TerminalCandidate {
    /// Successful model result.
    Completed {
        /// Model cycle.
        cycle: u64,
        /// Turn identity.
        turn_id: TurnId,
        /// Model request identity.
        model_request_id: ModelRequestId,
        /// Model effect identity.
        effect_id: EffectId,
        /// Durable assistant message identity.
        message_id: MessageId,
        /// Model output digest.
        result_digest: Digest,
    },
    /// Safe failure candidate.
    Failed {
        /// Model cycle.
        cycle: u64,
        /// Turn identity when a turn existed.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        turn_id: Option<TurnId>,
        /// Model request identity when a request existed.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model_request_id: Option<ModelRequestId>,
        /// Model effect identity when an effect existed.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        effect_id: Option<EffectId>,
        /// Safe failure descriptor.
        error: ErrorDescriptor,
    },
}

/// Final model settlement kind retained for replay classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelSettlementKind {
    /// Successful completion.
    Completed,
    /// Failed completion.
    Failed,
}

/// Digest retained for an effect's terminal model settlement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelSettlementFingerprint {
    /// Terminal settlement kind.
    pub kind: ModelSettlementKind,
    /// `model-settlement` schema-1 digest.
    pub digest: Digest,
}

/// Global external completion identity retained for fail-closed reuse checks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompletionIdentity {
    /// Effect identified by the external completion.
    pub effect_id: EffectId,
    /// Terminal model-settlement digest.
    pub settlement_digest: Digest,
}

/// Applied terminal state owned by PR-009.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
#[expect(
    clippy::large_enum_variant,
    reason = "the frozen public terminal payloads are stored by value"
)]
pub enum TerminalState {
    /// Successful terminal.
    Completed(RunCompleted),
    /// Failed terminal.
    Failed(RunFailed),
}

/// Sorted state-hash projection entry for one stage settlement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StageSettlementHashEntryV1 {
    /// Settled cycle.
    pub cycle: u64,
    /// Settled stage.
    pub stage: Stage,
    /// Settlement digest.
    pub settlement_digest: Digest,
}

/// Sorted state-hash projection entry for one model settlement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelSettlementHashEntryV1 {
    /// Settled effect identity.
    pub effect_id: EffectId,
    /// Settlement kind.
    pub kind: ModelSettlementKind,
    /// Settlement digest.
    pub settlement_digest: Digest,
}

/// Sorted state-hash projection entry for one external completion identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CompletionIdentityHashEntryV1 {
    /// External completion identity.
    pub completion_id: Arc<str>,
    /// Settled effect identity.
    pub effect_id: EffectId,
    /// Settlement digest.
    pub settlement_digest: Digest,
}

impl<'de> Deserialize<'de> for CompletionIdentityHashEntryV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            completion_id: BoundedString<LABEL_MAX_BYTES>,
            effect_id: EffectId,
            settlement_digest: Digest,
        }
        let wire = Wire::deserialize(deserializer)?;
        let completion_id = wire.completion_id.into_inner();
        if completion_id.is_empty() || completion_id.as_bytes().contains(&0) {
            return Err(de::Error::custom("invalid completion_id"));
        }
        Ok(Self {
            completion_id: completion_id.into(),
            effect_id: wire.effect_id,
            settlement_digest: wire.settlement_digest,
        })
    }
}

/// Complete authoritative state derived only from committed records.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// Current run phase.
    pub phase: Option<RunPhase>,
    /// Zero-based model cycle.
    pub cycle: u64,
    /// Current turn.
    pub current_turn: Option<CurrentTurn>,
    /// Durable final assistant messages in model-only order.
    pub messages: Arc<[Message]>,
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
    /// Applied terminal payload.
    pub terminal: Option<TerminalState>,
}

impl Default for KernelState {
    fn default() -> Self {
        Self {
            state_version: 1,
            last_applied_sequence: 0,
            session_id: None,
            lane_id: None,
            accepted: None,
            phase: None,
            cycle: 0,
            current_turn: None,
            messages: Arc::from([]),
            pending_model_effect: None,
            terminal_candidate: None,
            stage_settlements: BTreeMap::new(),
            model_settlements: BTreeMap::new(),
            completion_identities: BTreeMap::new(),
            terminal: None,
        }
    }
}

impl KernelState {
    /// Validate v1 collection ceilings for a programmatically assembled state.
    ///
    /// # Errors
    ///
    /// Returns [`KernelError::InvalidInputPayload`] when a semantic collection
    /// exceeds its frozen v1 ceiling.
    pub fn validate(&self) -> Result<(), KernelError> {
        if self.messages.len() > SEMANTIC_ARRAY_MAX_ITEMS {
            return Err(KernelError::InvalidInputPayload {
                field: "messages",
                reason_code: "too_many_items",
            });
        }
        for (field, length) in [
            ("stage_settlements", self.stage_settlements.len()),
            ("model_settlements", self.model_settlements.len()),
            ("completion_identities", self.completion_identities.len()),
        ] {
            if length > SEMANTIC_MAP_MAX_ENTRIES {
                return Err(KernelError::InvalidInputPayload {
                    field,
                    reason_code: "too_many_items",
                });
            }
        }
        if self.completion_identities.keys().any(|completion_id| {
            completion_id.is_empty()
                || completion_id.len() > LABEL_MAX_BYTES
                || completion_id.as_bytes().contains(&0)
        }) {
            return Err(KernelError::InvalidInputPayload {
                field: "completion_identities",
                reason_code: "invalid_label",
            });
        }
        Ok(())
    }

    /// Compute the exact schema-1 JCS state digest under `kernel-state`.
    ///
    /// # Errors
    ///
    /// Returns [`KernelError::StateHashFailed`] if the internal projection cannot
    /// be represented or canonicalized as JSON.
    pub fn state_hash(&self) -> Result<Digest, KernelError> {
        self.validate().map_err(|_| KernelError::StateHashFailed)?;
        let projection = KernelStateHashV1::from_state(
            self,
            stage_hash_entries(&self.stage_settlements),
            model_hash_entries(&self.model_settlements),
            completion_hash_entries(&self.completion_identities),
        );
        let canonical = serde_json_canonicalizer::to_vec(&projection)
            .map_err(|_| KernelError::StateHashFailed)?;
        Digest::domain_separated("kernel-state", 1, &canonical)
            .map_err(|_| KernelError::StateHashFailed)
    }
}

#[derive(Serialize)]
struct KernelStateWire<'a> {
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
    terminal: Option<TerminalState>,
}

impl Serialize for KernelState {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        KernelStateWire {
            state_version: self.state_version,
            last_applied_sequence: self.last_applied_sequence,
            session_id: self.session_id,
            lane_id: self.lane_id,
            accepted: self.accepted.as_ref(),
            phase: self.phase,
            cycle: self.cycle,
            current_turn: self.current_turn.as_ref(),
            messages: &self.messages,
            pending_model_effect: self.pending_model_effect.as_ref(),
            terminal_candidate: self.terminal_candidate.as_ref(),
            stage_settlements: stage_hash_entries(&self.stage_settlements),
            model_settlements: model_hash_entries(&self.model_settlements),
            completion_identities: completion_hash_entries(&self.completion_identities),
            terminal: self.terminal.as_ref(),
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for KernelState {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = KernelStateWireOwned::deserialize(deserializer)?;
        if wire.state_version != 1 {
            return Err(de::Error::custom("unsupported kernel state_version"));
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
        Ok(Self {
            state_version: wire.state_version,
            last_applied_sequence: wire.last_applied_sequence,
            session_id: wire.session_id,
            lane_id: wire.lane_id,
            accepted: wire.accepted,
            phase: wire.phase,
            cycle: wire.cycle,
            current_turn: wire.current_turn,
            messages: wire.messages.into_inner().into(),
            pending_model_effect: wire.pending_model_effect,
            terminal_candidate: wire.terminal_candidate,
            stage_settlements,
            model_settlements,
            completion_identities,
            terminal: wire.terminal,
        })
    }
}

fn stage_hash_entries(entries: &BTreeMap<StageCursor, Digest>) -> Vec<StageSettlementHashEntryV1> {
    let mut values = entries
        .iter()
        .map(|(cursor, digest)| StageSettlementHashEntryV1 {
            cycle: cursor.cycle,
            stage: cursor.stage,
            settlement_digest: *digest,
        })
        .collect::<Vec<_>>();
    values.sort_by(|left, right| {
        left.cycle
            .cmp(&right.cycle)
            .then_with(|| stage_name(left.stage).cmp(stage_name(right.stage)))
    });
    values
}

fn model_hash_entries(
    entries: &BTreeMap<EffectId, ModelSettlementFingerprint>,
) -> Vec<ModelSettlementHashEntryV1> {
    entries
        .iter()
        .map(|(effect_id, settlement)| ModelSettlementHashEntryV1 {
            effect_id: *effect_id,
            kind: settlement.kind,
            settlement_digest: settlement.digest,
        })
        .collect()
}

fn completion_hash_entries(
    entries: &BTreeMap<Arc<str>, CompletionIdentity>,
) -> Vec<CompletionIdentityHashEntryV1> {
    entries
        .iter()
        .map(|(completion_id, identity)| CompletionIdentityHashEntryV1 {
            completion_id: Arc::clone(completion_id),
            effect_id: identity.effect_id,
            settlement_digest: identity.settlement_digest,
        })
        .collect()
}

const fn stage_name(stage: Stage) -> &'static str {
    match stage {
        Stage::BeforeRun => "before_run",
        Stage::PrepareContext => "prepare_context",
        Stage::BeforeModel => "before_model",
        Stage::AfterModel => "after_model",
        Stage::BeforeToolBatch => "before_tool_batch",
        Stage::AfterToolBatch => "after_tool_batch",
        Stage::BeforeFinalize => "before_finalize",
    }
}
