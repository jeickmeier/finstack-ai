//! Reducer decisions, committed batches, actions, and stable errors.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::StageCursor;
use crate::bounds::BoundedVec;
use crate::ids::{AppendBatchId, EffectId};
use crate::records::{APPEND_BATCH_MAX_RECORDS, RecordDraft, RecordEnvelope};
use crate::refs::Diagnostic;
use crate::state::RunPhase;

/// Pure reducer proposal before atomic append.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Decision {
    /// First sequence the store must assign to `records`.
    pub expected_sequence: u64,
    /// Ordered durable record drafts.
    pub records: Vec<RecordDraft>,
    /// Ordered actions authorized only after commit and apply.
    pub actions: Vec<PostCommitAction>,
    /// Non-semantic decision diagnostics.
    pub diagnostics: Vec<Diagnostic>,
}

impl Decision {
    pub(crate) fn duplicate(expected_sequence: u64, diagnostic: Diagnostic) -> Self {
        Self {
            expected_sequence,
            records: Vec::new(),
            actions: Vec::new(),
            diagnostics: vec![diagnostic],
        }
    }
}

/// Action authorized by an already-committed decision batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub enum PostCommitAction {
    /// Execute the matching preceding `EffectRequested` record.
    ExecuteEffect {
        /// Committed effect identity.
        effect_id: EffectId,
    },
}

/// Atomic store result supplied to [`crate::Kernel::apply`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CommittedBatch {
    /// Runtime-owned append attempt identity.
    pub batch_id: AppendBatchId,
    /// First assigned sequence.
    pub first_sequence: u64,
    /// Last assigned sequence.
    pub last_sequence: u64,
    /// Ordered committed records.
    pub records: Arc<[RecordEnvelope]>,
}

impl CommittedBatch {
    /// Construct a committed batch under the atomic record-count ceiling.
    ///
    /// # Errors
    ///
    /// Returns [`KernelError::InvalidInputPayload`] when the record collection
    /// exceeds [`APPEND_BATCH_MAX_RECORDS`].
    pub fn try_new(
        batch_id: AppendBatchId,
        first_sequence: u64,
        last_sequence: u64,
        records: Vec<RecordEnvelope>,
    ) -> Result<Self, KernelError> {
        if records.len() > APPEND_BATCH_MAX_RECORDS {
            return Err(KernelError::InvalidInputPayload {
                field: "records",
                reason_code: "too_many_items",
            });
        }
        Ok(Self {
            batch_id,
            first_sequence,
            last_sequence,
            records: records.into(),
        })
    }
}

impl<'de> Deserialize<'de> for CommittedBatch {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            batch_id: AppendBatchId,
            first_sequence: u64,
            last_sequence: u64,
            records: BoundedVec<RecordEnvelope, APPEND_BATCH_MAX_RECORDS>,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(
            wire.batch_id,
            wire.first_sequence,
            wire.last_sequence,
            wire.records.into_inner(),
        )
        .map_err(serde::de::Error::custom)
    }
}

/// Stable reducer failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum KernelError {
    /// Structurally invalid normalized payload.
    #[error("invalid input payload at {field}: {reason_code}")]
    InvalidInputPayload {
        /// Invalid field.
        field: &'static str,
        /// Stable reason code.
        reason_code: &'static str,
    },
    /// Accepted run identities or lineage are inconsistent.
    #[error("invalid run acceptance")]
    InvalidRunAcceptance,
    /// No transition exists for this phase and input family.
    #[error("input {input} is invalid in phase {phase:?}")]
    InvalidPhaseInput {
        /// Current phase.
        phase: Option<RunPhase>,
        /// Stable input family name.
        input: &'static str,
    },
    /// Aggregate stage cursor does not match the exact expected cursor.
    #[error("stage cursor mismatch: expected {expected:?}, got {actual:?}")]
    StageCursorMismatch {
        /// Expected cursor.
        expected: StageCursor,
        /// Actual cursor.
        actual: StageCursor,
    },
    /// Prepared model request violates its output or effect contract.
    #[error("model request contract mismatch")]
    ModelRequestContractMismatch,
    /// Model settlement does not match the outstanding request.
    #[error("model settlement mismatch")]
    ModelSettlementMismatch,
    /// Assistant source tool-call identity was duplicated or reused.
    #[error("duplicate tool call")]
    DuplicateToolCall,
    /// Prepared tool plan does not exactly cover the assistant source calls.
    #[error("tool batch plan mismatch")]
    ToolBatchPlanMismatch,
    /// Tool effect kind or output contract is invalid.
    #[error("tool effect contract mismatch")]
    ToolEffectContractMismatch,
    /// Tool settlement does not match the active planned call.
    #[error("tool settlement mismatch")]
    ToolSettlementMismatch,
    /// Tool output is not an exactly associated result block.
    #[error("tool result mismatch")]
    ToolResultMismatch,
    /// External outcome and assistant-message presence disagree.
    #[error("assistant message presence mismatch")]
    AssistantMessagePresenceMismatch,
    /// Final assistant message does not match allocated identity or settlement metadata.
    #[error("assistant message mismatch")]
    AssistantMessageMismatch,
    /// Durable settlement record does not match its canonical fingerprint.
    #[error("settlement digest mismatch")]
    SettlementDigestMismatch,
    /// Prepared context digest is invalid.
    #[error("context digest mismatch")]
    ContextDigestMismatch,
    /// One external completion identity was reused inconsistently.
    #[error("conflicting completion id")]
    ConflictingCompletionId,
    /// Model cycle increment overflowed.
    #[error("model cycle overflow")]
    CycleOverflow,
    /// A required preallocated ID was absent.
    #[error("allocated ids exhausted for {kind}")]
    AllocatedIdsExhausted {
        /// ID queue name.
        kind: &'static str,
    },
    /// A state-changing decision supplied an unused kernel-owned ID.
    #[error("unused allocated ids for {kind}")]
    UnusedAllocatedIds {
        /// ID queue name.
        kind: &'static str,
    },
    /// A hard authoritative-state collection capacity would be exceeded.
    #[error("state capacity exceeded for {field}")]
    StateCapacityExceeded {
        /// Authoritative state field at capacity.
        field: &'static str,
    },
    /// Committed batch range does not describe its records.
    #[error("committed batch range mismatch")]
    CommittedBatchRangeMismatch,
    /// Committed record sequences are not contiguous.
    #[error("non-contiguous record sequence")]
    NonContiguousRecordSequence,
    /// Committed record identity does not match the accepted run.
    #[error("record identity mismatch")]
    RecordIdentityMismatch,
    /// Committed sibling records are absent, mismatched, duplicated, or reordered.
    #[error("invalid record order")]
    InvalidRecordOrder,
    /// Effect is not the outstanding deferred effect.
    #[error("effect {effect_id} is not pending")]
    EffectNotPending {
        /// Rejected effect identity.
        effect_id: EffectId,
    },
    /// A prior settlement identity was reused with unequal content.
    #[error("conflicting settlement")]
    ConflictingSettlement,
    /// Terminal state cannot be changed.
    #[error("terminal state is immutable")]
    TerminalStateImmutable,
    /// Canonical state-hash projection failed.
    #[error("state hash failed")]
    StateHashFailed,
    /// Replay-derived state contradicts reducer invariants.
    #[error("kernel invariant violation")]
    InvariantViolation,
}

impl KernelError {
    /// Stable lowercase error code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidInputPayload { .. } => "invalid_input_payload",
            Self::InvalidRunAcceptance => "invalid_run_acceptance",
            Self::InvalidPhaseInput { .. } => "invalid_phase_input",
            Self::StageCursorMismatch { .. } => "stage_cursor_mismatch",
            Self::ModelRequestContractMismatch => "model_request_contract_mismatch",
            Self::ModelSettlementMismatch => "model_settlement_mismatch",
            Self::DuplicateToolCall => "duplicate_tool_call",
            Self::ToolBatchPlanMismatch => "tool_batch_plan_mismatch",
            Self::ToolEffectContractMismatch => "tool_effect_contract_mismatch",
            Self::ToolSettlementMismatch => "tool_settlement_mismatch",
            Self::ToolResultMismatch => "tool_result_mismatch",
            Self::AssistantMessagePresenceMismatch => "assistant_message_presence_mismatch",
            Self::AssistantMessageMismatch => "assistant_message_mismatch",
            Self::SettlementDigestMismatch => "settlement_digest_mismatch",
            Self::ContextDigestMismatch => "context_digest_mismatch",
            Self::ConflictingCompletionId => "conflicting_completion_id",
            Self::CycleOverflow => "cycle_overflow",
            Self::AllocatedIdsExhausted { .. } => "allocated_ids_exhausted",
            Self::UnusedAllocatedIds { .. } => "unused_allocated_ids",
            Self::StateCapacityExceeded { .. } => "state_capacity_exceeded",
            Self::CommittedBatchRangeMismatch => "committed_batch_range_mismatch",
            Self::NonContiguousRecordSequence => "non_contiguous_record_sequence",
            Self::RecordIdentityMismatch => "record_identity_mismatch",
            Self::InvalidRecordOrder => "invalid_record_order",
            Self::EffectNotPending { .. } => "effect_not_pending",
            Self::ConflictingSettlement => "conflicting_settlement",
            Self::TerminalStateImmutable => "terminal_state_immutable",
            Self::StateHashFailed => "state_hash_failed",
            Self::InvariantViolation => "invariant_violation",
        }
    }
}
