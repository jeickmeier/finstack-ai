use std::collections::BTreeMap;
use std::sync::Arc;

use serde::de;
use serde::{Deserialize, Deserializer, Serialize};

use crate::content::{BoundedString, LABEL_MAX_BYTES};
use crate::effects::{EffectDeferred, EffectRequested, InteractionKind, InteractionRequest};
use crate::primitives::Digest;
use crate::primitives::ErrorDescriptor;
use crate::primitives::{EffectId, InteractionId, MessageId, ModelRequestId, ToolCallId, TurnId};
use crate::records::lifecycle::{
    ContextPrepared, RetryScheduled, RunCancelled, RunCompleted, RunFailed, Stage, StageCursor,
    TimerFired,
};
use crate::records::policy::{
    BudgetReleaseReceipt, BudgetReservationReceipt, BudgetReserveRequest,
};
use crate::records::run::CancellationRequest;
use crate::records::tools::ToolSettlementKind;

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

/// Outstanding context-provider or middleware effect reconstructed from the journal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PendingExtensionEffect {
    /// Exact stage invocation held while the extension executes.
    pub cursor: StageCursor,
    /// Original committed effect request.
    pub requested: EffectRequested,
}

/// Terminal kind retained for a context-provider or middleware settlement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionSettlementKind {
    /// Successful completion.
    Completed,
    /// Failed completion.
    Failed,
}

/// Replay-derived terminal identity for an extension effect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtensionSettlementFingerprint {
    /// Terminal settlement kind.
    pub kind: ExtensionSettlementKind,
    /// Domain-separated canonical settlement digest.
    pub digest: Digest,
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
    /// Domain-separated `model-settlement` digest.
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

/// Outstanding typed interaction reconstructed from the journal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PendingInteraction {
    /// Committed request envelope.
    pub request: InteractionRequest,
    /// Phase held when the request was committed.
    pub prior_phase: RunPhase,
    /// Unsettled stage cursor that must remain available after resume.
    pub cursor: StageCursor,
}

/// Replay-derived identity for one interaction resolution command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolutionIdentity {
    /// Interaction identified by the resolution.
    pub interaction_id: InteractionId,
    /// Normalized resolution digest.
    pub settlement_digest: Digest,
}

/// Terminal outcome of the most recently settled interaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractionTerminalOutcome {
    /// Schema-valid granting resolution, or a non-approval success.
    Granted,
    /// Schema-valid approval denial.
    Denied,
    /// Request expired before a valid resolution.
    Expired,
    /// Cancelled while waiting.
    Cancelled,
}

/// Last settled interaction retained so approval policy can release a cursor once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionTerminal {
    /// Settled interaction identity.
    pub interaction_id: InteractionId,
    /// Requested kind.
    pub kind: InteractionKind,
    /// Stage cursor that requested the interaction.
    pub cursor: StageCursor,
    /// Terminal classification.
    pub outcome: InteractionTerminalOutcome,
}

/// Applied terminal run state.
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

/// Replay-derived lifecycle of one shared-budget reservation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetReservationReplay {
    /// Durable reservation request committed with child preparation.
    pub request: BudgetReserveRequest,
    /// Exact settled ledger receipt, when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settlement: Option<BudgetReservationReceipt>,
    /// Exact release receipt, when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release: Option<BudgetReleaseReceipt>,
}

/// Sorted state-hash projection entry for one stage settlement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StageSettlementHashEntryV1 {
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
pub(crate) struct ModelSettlementHashEntryV1 {
    /// Settled effect identity.
    pub effect_id: EffectId,
    /// Settlement kind.
    pub kind: ModelSettlementKind,
    /// Settlement digest.
    pub settlement_digest: Digest,
}

/// Sorted state-hash projection entry for an extension settlement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExtensionSettlementHashEntryV7 {
    /// Settled effect identity.
    pub effect_id: EffectId,
    /// Settlement kind.
    pub kind: ExtensionSettlementKind,
    /// Settlement digest.
    pub settlement_digest: Digest,
}

/// Sorted state-hash projection entry for one external completion identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct CompletionIdentityHashEntryV1 {
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
pub(crate) struct ToolCallIdentityHashEntryV2 {
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

/// Borrowed hash/wire projection of one persistent tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) struct ToolCallIdentityHashRef<'a> {
    pub(crate) tool_call_id: ToolCallId,
    pub(crate) cycle: u64,
    pub(crate) turn_id: TurnId,
    pub(crate) source_message_id: MessageId,
    pub(crate) tool_batch_id: Option<crate::ToolBatchId>,
    pub(crate) effect_id: Option<EffectId>,
    pub(crate) call: &'a crate::ToolCallBlock,
}

/// Sorted state-hash projection entry for one tool settlement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ToolSettlementHashEntryV2 {
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
        Ok(Self {
            completion_id: identity_label(wire.completion_id, "invalid completion_id")?,
            effect_id: wire.effect_id,
            settlement_digest: wire.settlement_digest,
        })
    }
}

/// Reject empty or NUL-bearing identity labels (length is bounded by the wire type).
fn identity_label<E: de::Error>(
    label: BoundedString<LABEL_MAX_BYTES>,
    invalid: &'static str,
) -> Result<Arc<str>, E> {
    let label = label.into_inner();
    if label.is_empty() || label.as_bytes().contains(&0) {
        return Err(de::Error::custom(invalid));
    }
    Ok(label.into())
}

/// Sorted state-hash projection entry for one interaction resolution identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ResolutionIdentityHashEntryV6 {
    /// External resolution identity.
    pub resolution_id: Arc<str>,
    /// Settled interaction identity.
    pub interaction_id: InteractionId,
    /// Normalized resolution digest.
    pub settlement_digest: Digest,
}

impl<'de> Deserialize<'de> for ResolutionIdentityHashEntryV6 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            resolution_id: BoundedString<LABEL_MAX_BYTES>,
            interaction_id: InteractionId,
            settlement_digest: Digest,
        }
        let wire = Wire::deserialize(deserializer)?;
        Ok(Self {
            resolution_id: identity_label(wire.resolution_id, "invalid resolution_id")?,
            interaction_id: wire.interaction_id,
            settlement_digest: wire.settlement_digest,
        })
    }
}
