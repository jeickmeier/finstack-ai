//! Tool-call planning, durable batch records, and replay state.
//!
//! [`ToolCallPlan`] and [`ValidatedToolCall`] describe source-ordered calls.
//! [`ToolBatchOpened`], [`ToolCallSettled`], and [`ToolBatchClosed`] are
//! durable records. [`ActiveToolBatch`] and [`ToolCallIdentity`] are the
//! replay-derived in-flight state used by the reducer.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::content::{ToolCallBlock, ToolResultBlock};
use crate::conversation::Message;
use crate::effects::{
    ComponentInvocation, EffectDeferred, EffectOutputContract, EffectRequested, RetrySafety,
};
use crate::primitives::Digest;
use crate::primitives::ErrorDescriptor;
use crate::primitives::Timestamp;
use crate::primitives::{BoundedVec, SEMANTIC_ARRAY_MAX_ITEMS};
use crate::primitives::{EffectId, MessageId, ToolBatchId, ToolCallId, ToolId, TurnId};

/// Scheduling mode frozen on one source tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolExecutionMode {
    /// Consecutive parallel calls share one execution group.
    Parallel,
    /// The call executes alone.
    Sequential,
    /// The call executes alone and forms an explicit scheduling boundary.
    Barrier,
}

/// Framework failure policy frozen on one tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolFailurePolicy {
    /// Return a framework-authored error result to the model.
    ReturnToModel,
    /// Drain dispatched work, close undispatched calls, and fail the run.
    FailRun,
}

/// Continuation after a completely settled tool batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolBatchContinuation {
    /// Start a fresh model cycle with tool results in canonical history.
    ContinueModel,
    /// Enter `before_finalize` with the originating assistant candidate.
    Finalize,
}

/// Fully resolved executable tool-call plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValidatedToolCall {
    /// Original assistant tool-call block.
    pub call: ToolCallBlock,
    /// Stable resolved tool identity.
    pub tool_id: ToolId,
    /// Optional resolved component invocation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component: Option<ComponentInvocation>,
    /// Required tool-result output contract.
    pub output_contract: EffectOutputContract,
    /// Retry-safety classification.
    pub retry_safety: RetrySafety,
    /// Optional semantic deadline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline: Option<Timestamp>,
    /// Deterministic execution mode.
    pub execution: ToolExecutionMode,
    /// Framework failure policy.
    pub failure_policy: ToolFailurePolicy,
}

/// Framework-authored call closure that never dispatches a tool effect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SyntheticToolClosure {
    /// Original assistant tool-call block.
    pub call: ToolCallBlock,
    /// Deterministic execution mode used only for grouping.
    pub execution: ToolExecutionMode,
    /// Framework failure policy.
    pub failure_policy: ToolFailurePolicy,
    /// Safe model-visible framework error.
    pub error: ErrorDescriptor,
}

/// One source-ordered executable or synthetic call plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
#[expect(
    clippy::large_enum_variant,
    reason = "the frozen public wire contract stores both plan payloads by value"
)]
pub enum ToolCallPlan {
    /// Execute a resolved tool.
    Execute(ValidatedToolCall),
    /// Close the call without dispatching a tool.
    SyntheticClosure(SyntheticToolClosure),
}

impl ToolCallPlan {
    /// Original assistant call.
    #[must_use]
    pub const fn call(&self) -> &ToolCallBlock {
        match self {
            Self::Execute(value) => &value.call,
            Self::SyntheticClosure(value) => &value.call,
        }
    }

    /// Deterministic execution mode.
    #[must_use]
    pub const fn execution(&self) -> ToolExecutionMode {
        match self {
            Self::Execute(value) => value.execution,
            Self::SyntheticClosure(value) => value.execution,
        }
    }

    /// Framework failure policy.
    #[must_use]
    pub const fn failure_policy(&self) -> ToolFailurePolicy {
        match self {
            Self::Execute(value) => value.failure_policy,
            Self::SyntheticClosure(value) => value.failure_policy,
        }
    }
}

/// Reducer-assigned identity and group for one source call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssignedToolCall {
    /// Zero-based source position.
    pub source_index: u32,
    /// Zero-based execution-group position.
    pub group_index: u32,
    /// Stable effect identity allocated at batch opening.
    pub effect_id: EffectId,
    /// Complete validated or synthetic plan.
    pub plan: ToolCallPlan,
}

/// Durable opening of one complete source-ordered batch plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ToolBatchOpened {
    /// Model cycle that produced the calls.
    pub cycle: u64,
    /// Originating turn.
    pub turn_id: TurnId,
    /// Stable batch identity.
    pub tool_batch_id: ToolBatchId,
    /// Assistant message that contained the calls.
    pub source_message_id: MessageId,
    /// Complete source-ordered assigned plan.
    pub calls: Arc<[AssignedToolCall]>,
    /// Frozen continuation after closure.
    pub continuation: ToolBatchContinuation,
    /// `tool-batch-plan` schema-1 digest.
    pub plan_digest: Digest,
}

impl<'de> Deserialize<'de> for ToolBatchOpened {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            cycle: u64,
            turn_id: TurnId,
            tool_batch_id: ToolBatchId,
            source_message_id: MessageId,
            calls: BoundedVec<AssignedToolCall, SEMANTIC_ARRAY_MAX_ITEMS>,
            continuation: ToolBatchContinuation,
            plan_digest: Digest,
        }
        let wire = Wire::deserialize(deserializer)?;
        Ok(Self {
            cycle: wire.cycle,
            turn_id: wire.turn_id,
            tool_batch_id: wire.tool_batch_id,
            source_message_id: wire.source_message_id,
            calls: wire.calls.into_inner().into(),
            continuation: wire.continuation,
            plan_digest: wire.plan_digest,
        })
    }
}

/// Durable source-order finalization of one tool call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolCallSettled {
    /// Model cycle that produced the call.
    pub cycle: u64,
    /// Originating turn.
    pub turn_id: TurnId,
    /// Owning batch.
    pub tool_batch_id: ToolBatchId,
    /// Source call identity.
    pub tool_call_id: ToolCallId,
    /// Stable call effect identity.
    pub effect_id: EffectId,
    /// Canonical tool-role result message.
    pub message: Message,
    /// `tool-settlement` schema-1 digest.
    pub settlement_digest: Digest,
    /// Whether the framework authored the result.
    pub synthetic: bool,
    /// Safe framework error for a synthetic result.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorDescriptor>,
}

/// Semantic outcome of a completely settled tool batch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
#[expect(
    clippy::large_enum_variant,
    reason = "the frozen public wire contract stores ErrorDescriptor by value"
)]
pub enum ToolBatchOutcome {
    /// Continue with a new model cycle.
    ContinueModel,
    /// Finalize the originating assistant candidate.
    Finalize,
    /// Fail after all dispatched work has been accounted for.
    Failed {
        /// Safe failure descriptor.
        error: ErrorDescriptor,
    },
}

/// Durable closure of one fully settled tool batch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolBatchClosed {
    /// Model cycle that produced the calls.
    pub cycle: u64,
    /// Originating turn.
    pub turn_id: TurnId,
    /// Stable batch identity.
    pub tool_batch_id: ToolBatchId,
    /// Assistant message that contained the calls.
    pub source_message_id: MessageId,
    /// Source-ordered tool-result messages.
    pub result_message_ids: Arc<[MessageId]>,
    /// Semantic post-batch outcome.
    pub outcome: ToolBatchOutcome,
    /// `tool-batch-close` schema-1 digest.
    pub close_digest: Digest,
}

/// Replay state for one source call in an active batch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActiveToolCall {
    /// Frozen assigned plan.
    pub assigned: AssignedToolCall,
    /// Current replay-derived status.
    pub status: ActiveToolCallStatus,
}

/// Replay-derived lifecycle of one active source call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub enum ActiveToolCallStatus {
    /// No request record has been committed.
    Undispatched,
    /// Request committed, with an optional external deferral.
    Requested {
        /// Original effect request.
        requested: EffectRequested,
        /// Retained deferral handle.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        deferred: Option<EffectDeferred>,
    },
    /// Terminal effect data is buffered until the source prefix is contiguous.
    Buffered {
        /// Normalized result block.
        result: ToolResultBlock,
        /// Source-discriminated settlement digest.
        settlement_digest: Digest,
        /// Whether the framework authored the result.
        synthetic: bool,
        /// Safe framework error for synthetic results.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<ErrorDescriptor>,
    },
    /// Canonical result message has been committed.
    Settled {
        /// Result message identity.
        result_message_id: MessageId,
        /// Source-discriminated settlement digest.
        settlement_digest: Digest,
    },
}

/// Replay-derived state of the active source-ordered batch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActiveToolBatch {
    /// Durable opening record.
    pub opened: ToolBatchOpened,
    /// Source-ordered call states.
    pub calls: Arc<[ActiveToolCall]>,
    /// Current eligible execution group.
    pub current_group: u32,
    /// Next source position eligible for canonical finalization.
    pub next_source_index: u32,
    /// Finalized result messages in source order.
    pub result_message_ids: Arc<[MessageId]>,
    /// First fail-run framework error, when any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fatal_error: Option<ErrorDescriptor>,
}

/// Persistent source call identity retained for duplicate prevention.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolCallIdentity {
    /// Model cycle that produced the call.
    pub cycle: u64,
    /// Originating turn.
    pub turn_id: TurnId,
    /// Assistant source message.
    pub source_message_id: MessageId,
    /// Assigned batch identity after planning.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_batch_id: Option<ToolBatchId>,
    /// Assigned effect identity after planning.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect_id: Option<EffectId>,
    /// Exact source call.
    pub call: ToolCallBlock,
}

/// Terminal tool settlement kind retained for replay classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolSettlementKind {
    /// Tool effect completed successfully.
    Completed,
    /// Tool effect failed at the framework boundary.
    Failed,
    /// Framework-authored closure without a tool effect.
    Synthetic,
}

/// Digest retained for one terminal tool settlement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolSettlementFingerprint {
    /// Terminal settlement kind.
    pub kind: ToolSettlementKind,
    /// `tool-settlement` schema-1 digest.
    pub digest: Digest,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::SEMANTIC_ARRAY_MAX_ITEMS;
    use serde_json::json;

    #[test]
    fn opened_calls_reject_over_semantic_array_max_before_element_decode() {
        let calls = vec![json!(null); SEMANTIC_ARRAY_MAX_ITEMS + 1];
        let opened = json!({
            "cycle": 0,
            "turn_id": "01234567-89ab-7cde-89ab-0123456789ad",
            "tool_batch_id": "01234567-89ab-7cde-89ab-0123456789ae",
            "source_message_id": "01234567-89ab-7cde-89ab-0123456789af",
            "calls": calls,
            "continuation": "finalize",
            "plan_digest": "00".repeat(32),
        });
        let error = serde_json::from_value::<ToolBatchOpened>(opened).expect_err("oversized calls");
        assert!(
            error.to_string().contains("array item count"),
            "expected item-ceiling error, got {error}"
        );
    }
}
