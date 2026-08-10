//! Authoritative replay-derived kernel state.

mod env;
mod hash_projection;

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::de;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::agent::{FinalResultRecorded, OutputConfiguration};
use crate::bounds::{BoundedVec, SEMANTIC_ARRAY_MAX_ITEMS, SEMANTIC_MAP_MAX_ENTRIES};
use crate::capabilities::ActiveCapability;
use crate::content::{BoundedString, ContentBlock, LABEL_MAX_BYTES, ToolCallBlock};
use crate::digest::Digest;
use crate::effects::{EffectDeferred, EffectInput, EffectKind, EffectOutputKind, EffectRequested};
use crate::entries::{
    ContextPrepared, RetryScheduled, RunCancelled, RunCompleted, RunFailed, RunSuspended, Stage,
    StageCursor, TimerFired,
};
use crate::error::ErrorDescriptor;
use crate::ids::{EffectId, LaneId, MessageId, ModelRequestId, SessionId, ToolCallId, TurnId};
use crate::limits::{LimitReached, LimitUsage};
use crate::message::Message;
use crate::reducer::KernelError;
use crate::run::{CancellationRequest, RunAccepted};
use crate::time::Timestamp;
use crate::tools::{
    ActiveToolBatch, ActiveToolCallStatus, ToolBatchClosed, ToolCallIdentity, ToolCallPlan,
    ToolSettlementFingerprint, ToolSettlementKind,
};
use crate::validation::OutputValidationFailed;

use hash_projection::{KernelStateHashV1, KernelStateHashV2};

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
    /// Cancelled terminal.
    Cancelled(RunCancelled),
}

/// Replay-derived cancellation control state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CancellationState {
    /// Winning request.
    pub request: CancellationRequest,
    /// Phase held when cancellation gained control.
    pub prior_phase: RunPhase,
    /// Cumulative completed effects.
    pub completed_effects: Arc<[EffectId]>,
    /// Cumulative cancelled effects.
    pub cancelled_effects: Arc<[EffectId]>,
    /// Cumulative uncertain effects.
    pub uncertain_effects: Arc<[EffectId]>,
    /// Effects still awaiting reconciliation.
    pub outstanding_effects: Arc<[EffectId]>,
}

/// Replay-derived semantic retry state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetryState {
    /// Number of committed additional attempts.
    pub attempts: u32,
    /// Timer currently gating a retry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending: Option<RetryScheduled>,
    /// Terminal timer firings retained for duplicate/conflict classification.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub timer_firings: BTreeMap<EffectId, TimerFired>,
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

/// Sorted state-hash projection entry for one persistent tool call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolCallIdentityHashEntryV2 {
    /// Persistent call identity.
    pub tool_call_id: ToolCallId,
    /// Model cycle that produced the call.
    pub cycle: u64,
    /// Originating turn.
    pub turn_id: TurnId,
    /// Assistant source message.
    pub source_message_id: MessageId,
    /// Assigned batch identity after planning.
    #[serde(default)]
    pub tool_batch_id: Option<crate::ToolBatchId>,
    /// Assigned effect identity after planning.
    #[serde(default)]
    pub effect_id: Option<EffectId>,
    /// Exact source call.
    pub call: crate::ToolCallBlock,
}

/// Sorted state-hash projection entry for one tool settlement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolSettlementHashEntryV2 {
    /// Settled effect identity.
    pub effect_id: EffectId,
    /// Settlement kind.
    pub kind: ToolSettlementKind,
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
    /// Compares the canonical state projection rather than two `serde_json`
    /// DOM trees.
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
            active_tool_batch: None,
            tool_calls: BTreeMap::new(),
            tool_settlements: BTreeMap::new(),
            last_tool_batch: None,
            output_configuration: None,
            active_capabilities: Arc::from([]),
            resolved_plan_digest: None,
            final_result: None,
            validation_failure: None,
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
    /// Validate v1 collection ceilings for a programmatically assembled state.
    ///
    /// # Errors
    ///
    /// Returns [`KernelError::InvalidInputPayload`] when a semantic collection
    /// exceeds its frozen v1 ceiling.
    #[expect(
        clippy::too_many_lines,
        reason = "state validation keeps all cross-field replay invariants in one fail-closed boundary"
    )]
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
            ("tool_calls", self.tool_calls.len()),
            ("tool_settlements", self.tool_settlements.len()),
            ("timer_firings", self.retry.timer_firings.len()),
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
        let has_tool_state = self.active_tool_batch.is_some()
            || !self.tool_calls.is_empty()
            || !self.tool_settlements.is_empty()
            || self.last_tool_batch.is_some();
        let has_control_state = self.cancellation.is_some()
            || self.retry != RetryState::default()
            || self.last_limit.is_some()
            || self.suspension.is_some()
            || matches!(self.terminal, Some(TerminalState::Cancelled(_)));
        let has_structured_state = self.output_configuration.is_some()
            || !self.active_capabilities.is_empty()
            || self.resolved_plan_digest.is_some()
            || self.final_result.is_some()
            || self.validation_failure.is_some();
        if !matches!(self.state_version, 1..=4)
            || (self.state_version == 1 && has_tool_state)
            || (self.state_version < 3 && has_control_state)
            || (self.state_version < 4 && has_structured_state)
        {
            return Err(KernelError::InvalidInputPayload {
                field: "state_version",
                reason_code: "unsupported_or_inconsistent",
            });
        }
        if self.state_version >= 2 && has_tool_state {
            self.validate_tool_state()?;
        }
        if self.state_version >= 3 {
            if self.accepted.is_some() != self.accepted_at.is_some() {
                return Err(KernelError::InvalidInputPayload {
                    field: "accepted_at",
                    reason_code: "inconsistent",
                });
            }
            if self.cancellation.is_some()
                != matches!(
                    self.phase,
                    Some(RunPhase::Cancelling | RunPhase::Suspended | RunPhase::Cancelled)
                )
                && self.cancellation.is_some()
            {
                return Err(KernelError::InvalidInputPayload {
                    field: "cancellation",
                    reason_code: "inconsistent",
                });
            }
            if self.limit_usage.extension_counters.len() > crate::RunLimits::MAX_EXTENSION_COUNTERS
                || self.accepted.as_ref().is_some_and(|accepted| {
                    self.limit_usage
                        .extension_counters
                        .keys()
                        .any(|key| !accepted.limits().extension_counters.contains_key(key))
                })
            {
                return Err(KernelError::InvalidInputPayload {
                    field: "limit_usage.extension_counters",
                    reason_code: "unregistered_or_too_many_entries",
                });
            }
            if self
                .last_limit
                .as_ref()
                .is_some_and(|limit| limit.validate().is_err())
            {
                return Err(KernelError::InvalidInputPayload {
                    field: "last_limit",
                    reason_code: "invalid_value_kind",
                });
            }
            if self.retry.pending.as_ref().is_some_and(|pending| {
                pending.attempt != self.retry.attempts || self.phase != Some(RunPhase::Sleeping)
            }) || (self.phase == Some(RunPhase::Sleeping) && self.retry.pending.is_none())
            {
                return Err(KernelError::InvalidInputPayload {
                    field: "retry",
                    reason_code: "inconsistent",
                });
            }
            if let Some(cancellation) = &self.cancellation {
                for (field, values) in [
                    (
                        "cancellation.completed_effects",
                        cancellation.completed_effects.as_ref(),
                    ),
                    (
                        "cancellation.cancelled_effects",
                        cancellation.cancelled_effects.as_ref(),
                    ),
                    (
                        "cancellation.uncertain_effects",
                        cancellation.uncertain_effects.as_ref(),
                    ),
                    (
                        "cancellation.outstanding_effects",
                        cancellation.outstanding_effects.as_ref(),
                    ),
                ] {
                    if values.len() > SEMANTIC_ARRAY_MAX_ITEMS
                        || values.windows(2).any(|pair| pair[0] >= pair[1])
                    {
                        return Err(KernelError::InvalidInputPayload {
                            field,
                            reason_code: "invalid_effect_set",
                        });
                    }
                }
                let classified = cancellation
                    .completed_effects
                    .iter()
                    .chain(cancellation.cancelled_effects.iter())
                    .chain(cancellation.uncertain_effects.iter())
                    .copied()
                    .collect::<std::collections::BTreeSet<_>>();
                let classified_len = cancellation.completed_effects.len()
                    + cancellation.cancelled_effects.len()
                    + cancellation.uncertain_effects.len();
                if classified.len() != classified_len
                    || cancellation
                        .outstanding_effects
                        .iter()
                        .any(|id| classified.contains(id))
                {
                    return Err(KernelError::InvalidInputPayload {
                        field: "cancellation",
                        reason_code: "overlapping_effect_sets",
                    });
                }
            }
            let terminal_phase_matches = matches!(
                (&self.terminal, self.phase),
                (Some(TerminalState::Completed(_)), Some(RunPhase::Completed))
                    | (Some(TerminalState::Failed(_)), Some(RunPhase::Failed))
                    | (Some(TerminalState::Cancelled(_)), Some(RunPhase::Cancelled))
                    | (None, _)
            );
            if !terminal_phase_matches {
                return Err(KernelError::InvalidInputPayload {
                    field: "terminal",
                    reason_code: "inconsistent_phase",
                });
            }
        }
        if self.state_version == 4 {
            if self
                .output_configuration
                .as_ref()
                .is_some_and(|configuration| configuration.validate().is_err())
            {
                return Err(KernelError::InvalidInputPayload {
                    field: "output_configuration",
                    reason_code: "invalid",
                });
            }
            if self.active_capabilities.len() > SEMANTIC_ARRAY_MAX_ITEMS
                || self
                    .active_capabilities
                    .windows(2)
                    .any(|pair| pair[0].capability_id >= pair[1].capability_id)
                || self
                    .active_capabilities
                    .iter()
                    .any(|item| item.source == crate::CapabilityActivationSource::Model)
            {
                return Err(KernelError::InvalidInputPayload {
                    field: "active_capabilities",
                    reason_code: "invalid_capability_set",
                });
            }
            if self.final_result.is_some() && self.validation_failure.is_some() {
                return Err(KernelError::InvalidInputPayload {
                    field: "structured_output",
                    reason_code: "conflicting_outcomes",
                });
            }
            if self
                .final_result
                .as_ref()
                .is_some_and(|result| result.validate().is_err())
                || self
                    .validation_failure
                    .as_ref()
                    .is_some_and(|failure| failure.validate().is_err())
            {
                return Err(KernelError::InvalidInputPayload {
                    field: "structured_output",
                    reason_code: "invalid",
                });
            }
            if let Some(result) = self.final_result.as_ref() {
                let configuration_matches = matches!(
                    self.output_configuration.as_ref(),
                    Some(OutputConfiguration {
                        output: crate::OutputSpec::JsonSchema { schema },
                        end_strategy,
                    }) if schema == &result.schema && end_strategy == &result.end_strategy
                );
                let candidate_matches = matches!(
                    self.terminal_candidate.as_ref(),
                    Some(TerminalCandidate::Completed {
                        cycle,
                        turn_id,
                        model_request_id,
                        effect_id,
                        message_id,
                        result_digest,
                    }) if cycle == &result.cycle
                        && turn_id == &result.turn_id
                        && model_request_id == &result.model_request_id
                        && effect_id == &result.effect_id
                        && message_id == &result.message_id
                        && result_digest == &result.value_digest
                );
                if !configuration_matches || !candidate_matches {
                    return Err(KernelError::InvalidInputPayload {
                        field: "final_result",
                        reason_code: "configuration_mismatch",
                    });
                }
            }
            if let Some(failure) = self.validation_failure.as_ref() {
                let schema_matches = matches!(
                    self.output_configuration.as_ref(),
                    Some(OutputConfiguration {
                        output: crate::OutputSpec::JsonSchema { schema },
                        ..
                    }) if schema == &failure.schema
                );
                let candidate_matches = matches!(
                    self.terminal_candidate.as_ref(),
                    Some(TerminalCandidate::Failed {
                        cycle,
                        turn_id: Some(turn_id),
                        model_request_id: Some(model_request_id),
                        effect_id: Some(effect_id),
                        error,
                    }) if cycle == &failure.cycle
                        && turn_id == &failure.turn_id
                        && model_request_id == &failure.model_request_id
                        && effect_id == &failure.effect_id
                        && error == &failure.error
                );
                if !schema_matches || !candidate_matches {
                    return Err(KernelError::InvalidInputPayload {
                        field: "validation_failure",
                        reason_code: "configuration_mismatch",
                    });
                }
            }
            if (self.final_result.is_some() || self.validation_failure.is_some())
                && !matches!(
                    self.output_configuration,
                    Some(OutputConfiguration {
                        output: crate::OutputSpec::JsonSchema { .. },
                        ..
                    })
                )
            {
                return Err(KernelError::InvalidInputPayload {
                    field: "structured_output",
                    reason_code: "missing_configuration",
                });
            }
        }
        Ok(())
    }

    fn validate_tool_state(&self) -> Result<(), KernelError> {
        let invalid = || KernelError::InvalidInputPayload {
            field: "tool_state",
            reason_code: "inconsistent",
        };
        if self.tool_calls.is_empty()
            || (self.active_tool_batch.is_some() && self.last_tool_batch.is_some())
        {
            return Err(invalid());
        }

        // One pass over messages builds the authorship index, so each tool call
        // costs a lookup instead of a full message-and-block rescan. The nested
        // form was O(tool_calls x messages x blocks) and ran on every apply and
        // every deserialize, making it a decode-path denial-of-service surface.
        let mut authored: BTreeMap<&ToolCallId, Vec<(&MessageId, &ToolCallBlock)>> =
            BTreeMap::new();
        for message in self.messages.iter() {
            if message.role() != crate::MessageRole::Assistant {
                continue;
            }
            for block in message.content() {
                if let ContentBlock::ToolCall(call) = block {
                    authored
                        .entry(call.tool_call_id())
                        .or_default()
                        .push((message.id(), call));
                }
            }
        }

        let mut effect_ids = std::collections::BTreeSet::new();
        for (tool_call_id, identity) in &self.tool_calls {
            if identity.call.tool_call_id() != tool_call_id
                || identity.tool_batch_id.is_some() != identity.effect_id.is_some()
            {
                return Err(invalid());
            }
            let source_matches = authored.get(tool_call_id).is_some_and(|authorships| {
                authorships.iter().any(|(message_id, call)| {
                    **message_id == identity.source_message_id && *call == &identity.call
                })
            });
            if !source_matches {
                return Err(invalid());
            }
            if let Some(effect_id) = identity.effect_id
                && !effect_ids.insert(effect_id)
            {
                return Err(invalid());
            }
        }
        if self
            .tool_settlements
            .keys()
            .any(|effect_id| !effect_ids.contains(effect_id))
        {
            return Err(invalid());
        }

        if let Some(batch) = &self.active_tool_batch {
            self.validate_active_tool_batch(batch)
                .map_err(|()| invalid())?;
        } else if matches!(self.phase, Some(RunPhase::AwaitingTools))
            || (self.phase == Some(RunPhase::AwaitingExternal)
                && self.pending_model_effect.is_none())
        {
            return Err(invalid());
        }

        if let Some(closed) = &self.last_tool_batch {
            let allowed_phase = matches!(
                self.phase,
                Some(
                    RunPhase::AfterToolBatch
                        | RunPhase::BeforeFinalize
                        | RunPhase::Completed
                        | RunPhase::Failed
                        | RunPhase::Cancelling
                        | RunPhase::Suspended
                        | RunPhase::Cancelled
                )
            );
            let assigned_count = self
                .tool_calls
                .values()
                .filter(|identity| identity.tool_batch_id == Some(closed.tool_batch_id))
                .count();
            // Hoisted out of the membership test below, which was O(results x messages).
            let tool_message_ids = self
                .messages
                .iter()
                .filter(|message| message.role() == crate::MessageRole::Tool)
                .map(crate::Message::id)
                .collect::<std::collections::BTreeSet<_>>();
            if !allowed_phase
                || closed.cycle > self.cycle
                || assigned_count == 0
                || assigned_count != closed.result_message_ids.len()
                || closed
                    .result_message_ids
                    .iter()
                    .any(|message_id| !tool_message_ids.contains(message_id))
            {
                return Err(invalid());
            }
        }
        Ok(())
    }

    #[expect(
        clippy::too_many_lines,
        reason = "the active-batch state machine is validated as one source-ordered invariant"
    )]
    fn validate_active_tool_batch(&self, batch: &ActiveToolBatch) -> Result<(), ()> {
        if !matches!(
            self.phase,
            Some(
                RunPhase::AwaitingTools
                    | RunPhase::AwaitingExternal
                    | RunPhase::Cancelling
                    | RunPhase::Suspended
            )
        ) || self.pending_model_effect.is_some()
            || batch.opened.cycle != self.cycle
            || batch.calls.is_empty()
            || batch.calls.len() != batch.opened.calls.len()
            || usize::try_from(batch.next_source_index).map_err(|_| ())? > batch.calls.len()
            || batch.result_message_ids.len()
                != usize::try_from(batch.next_source_index).map_err(|_| ())?
            || self
                .current_turn
                .as_ref()
                .is_none_or(|turn| turn.turn_id != batch.opened.turn_id)
        {
            return Err(());
        }

        let expected_external = batch.calls.iter().any(|call| {
            matches!(
                call.status,
                ActiveToolCallStatus::Requested {
                    deferred: Some(_),
                    ..
                }
            )
        });
        if matches!(
            self.phase,
            Some(RunPhase::AwaitingTools | RunPhase::AwaitingExternal)
        ) && (self.phase == Some(RunPhase::AwaitingExternal)) != expected_external
        {
            return Err(());
        }

        let finalized = usize::try_from(batch.next_source_index).map_err(|_| ())?;
        let mut has_current_request = false;
        for (index, (call, opened_call)) in batch
            .calls
            .iter()
            .zip(batch.opened.calls.iter())
            .enumerate()
        {
            if &call.assigned != opened_call
                || call.assigned.source_index != u32::try_from(index).map_err(|_| ())?
            {
                return Err(());
            }
            let expected_group = if index == 0 {
                0
            } else {
                let prior = &batch.opened.calls[index - 1];
                if prior.plan.execution() == crate::ToolExecutionMode::Parallel
                    && call.assigned.plan.execution() == crate::ToolExecutionMode::Parallel
                {
                    prior.group_index
                } else {
                    prior.group_index.checked_add(1).ok_or(())?
                }
            };
            if call.assigned.group_index != expected_group {
                return Err(());
            }
            let identity = self
                .tool_calls
                .get(call.assigned.plan.call().tool_call_id())
                .ok_or(())?;
            if identity.call != *call.assigned.plan.call()
                || identity.tool_batch_id != Some(batch.opened.tool_batch_id)
                || identity.effect_id != Some(call.assigned.effect_id)
            {
                return Err(());
            }

            match (&call.assigned.plan, &call.status) {
                (
                    ToolCallPlan::SyntheticClosure(_),
                    ActiveToolCallStatus::Undispatched | ActiveToolCallStatus::Requested { .. },
                ) => {
                    return Err(());
                }
                (
                    ToolCallPlan::Execute(plan),
                    ActiveToolCallStatus::Requested {
                        requested,
                        deferred,
                    },
                ) => {
                    if call.assigned.group_index != batch.current_group
                        || requested.effect_id() != call.assigned.effect_id
                        || requested.kind() != EffectKind::Tool
                        || requested.relation().is_some()
                        || requested.component() != plan.component.as_ref()
                        || requested.pipeline().is_some()
                        || requested.output_contract() != &plan.output_contract
                        || requested.output_contract().kind != EffectOutputKind::ToolResult
                        || !matches!(requested.input(), EffectInput::Tool { call } if call == &plan.call)
                        || requested.retry_safety() != plan.retry_safety
                        || requested.deadline() != plan.deadline
                        || deferred
                            .as_ref()
                            .is_some_and(|value| value.validate_against(requested).is_err())
                    {
                        return Err(());
                    }
                    has_current_request = true;
                }
                (ToolCallPlan::Execute(_), ActiveToolCallStatus::Undispatched)
                    if call.assigned.group_index <= batch.current_group =>
                {
                    return Err(());
                }
                (
                    ToolCallPlan::Execute(_),
                    ActiveToolCallStatus::Buffered { .. } | ActiveToolCallStatus::Settled { .. },
                ) if call.assigned.group_index > batch.current_group
                    && !matches!(self.phase, Some(RunPhase::Cancelling | RunPhase::Suspended)) =>
                {
                    return Err(());
                }
                (
                    _,
                    ActiveToolCallStatus::Buffered {
                        result,
                        settlement_digest,
                        synthetic,
                        error,
                    },
                ) => {
                    if result.tool_call_id() != call.assigned.plan.call().tool_call_id()
                        || *synthetic != error.is_some()
                        || (*synthetic && !result.is_error())
                    {
                        return Err(());
                    }
                    if matches!(call.assigned.plan, ToolCallPlan::Execute(_))
                        && self
                            .tool_settlements
                            .get(&call.assigned.effect_id)
                            .is_none_or(|entry| entry.digest != *settlement_digest)
                    {
                        return Err(());
                    }
                }
                (
                    _,
                    ActiveToolCallStatus::Settled {
                        result_message_id,
                        settlement_digest,
                    },
                ) => {
                    if self
                        .tool_settlements
                        .get(&call.assigned.effect_id)
                        .is_none_or(|entry| entry.digest != *settlement_digest)
                    {
                        return Err(());
                    }
                    if index >= finalized
                        || batch.result_message_ids.get(index) != Some(result_message_id)
                    {
                        return Err(());
                    }
                }
                _ => {}
            }
            if index < finalized && !matches!(call.status, ActiveToolCallStatus::Settled { .. }) {
                return Err(());
            }
            if index >= finalized && matches!(call.status, ActiveToolCallStatus::Settled { .. }) {
                return Err(());
            }
        }
        let has_fail_run_settlement = batch.calls.iter().any(|call| {
            call.assigned.plan.failure_policy() == crate::ToolFailurePolicy::FailRun
                && self
                    .tool_settlements
                    .get(&call.assigned.effect_id)
                    .is_some_and(|entry| entry.kind == ToolSettlementKind::Failed)
        });
        if batch.fatal_error.is_some() != has_fail_run_settlement {
            return Err(());
        }
        if !has_current_request {
            return Err(());
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
        // Streamed into the hasher rather than canonicalized into a `Vec`: the
        // state projection is the largest single canonical payload the kernel
        // produces, and buffering it grew a fresh allocation every call.
        let mut writer =
            crate::digest::DigestWriter::new("kernel-state", u32::from(self.state_version))
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
        } else {
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
        }
        .map_err(|_| KernelError::StateHashFailed)?;
        Ok(writer.finish().0)
    }
}

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
    terminal: Option<TerminalState>,
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
        } else {
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
        if !matches!(wire.state_version, 1..=4) {
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
        if matches!(wire.state_version, 2..=4) && !all_tool_fields_present {
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
        if wire.state_version == 4 && !all_structured_fields_present {
            return Err(de::Error::custom(
                "v4 kernel state is missing structured-output fields",
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
        let state = Self {
            state_version: wire.state_version,
            last_applied_sequence: wire.last_applied_sequence,
            session_id: wire.session_id,
            lane_id: wire.lane_id,
            accepted: wire.accepted,
            accepted_at: match wire.accepted_at {
                NullableField::Missing => None,
                NullableField::Present(value) => value,
            },
            phase: wire.phase,
            cycle: wire.cycle,
            current_turn: wire.current_turn,
            messages: wire.messages.into_inner().into(),
            pending_model_effect: wire.pending_model_effect,
            terminal_candidate: wire.terminal_candidate,
            stage_settlements,
            model_settlements,
            completion_identities,
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
            limit_usage: match wire.limit_usage {
                RequiredField::Missing => LimitUsage::default(),
                RequiredField::Present(value) => value,
            },
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
            terminal: wire.terminal,
        };
        state.validate().map_err(de::Error::custom)?;
        Ok(state)
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

fn tool_call_hash_entries(
    entries: &BTreeMap<ToolCallId, ToolCallIdentity>,
) -> Vec<ToolCallIdentityHashEntryV2> {
    entries
        .iter()
        .map(|(tool_call_id, identity)| ToolCallIdentityHashEntryV2 {
            tool_call_id: *tool_call_id,
            cycle: identity.cycle,
            turn_id: identity.turn_id,
            source_message_id: identity.source_message_id,
            tool_batch_id: identity.tool_batch_id,
            effect_id: identity.effect_id,
            call: identity.call.clone(),
        })
        .collect()
}

fn tool_settlement_hash_entries(
    entries: &BTreeMap<EffectId, ToolSettlementFingerprint>,
) -> Vec<ToolSettlementHashEntryV2> {
    entries
        .iter()
        .map(|(effect_id, settlement)| ToolSettlementHashEntryV2 {
            effect_id: *effect_id,
            kind: settlement.kind,
            settlement_digest: settlement.digest,
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
