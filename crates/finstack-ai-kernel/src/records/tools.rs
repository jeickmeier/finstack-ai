//! Tool-call planning, durable batch records, and replay state.
//!
//! [`ToolCallPlan`] and [`ValidatedToolCall`] describe source-ordered calls.
//! [`ToolBatchOpened`], [`ToolCallSettled`], and [`ToolBatchClosed`] are
//! durable records. [`ActiveToolBatch`] and [`ToolCallIdentity`] are the
//! replay-derived in-flight state used by the reducer.

use std::collections::BTreeMap;
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
#[allow(
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
    /// Domain-separated `tool-batch-plan` digest.
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
    /// Domain-separated `tool-settlement` digest.
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
    /// Domain-separated `tool-batch-close` digest.
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
#[derive(Debug, Clone, Serialize)]
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
    /// Immutable `effect_id →` source-order call index, built at batch open.
    #[serde(skip)]
    effect_index: BTreeMap<EffectId, u32>,
    /// Open (Requested or Undispatched) calls in [`Self::current_group`].
    #[serde(skip)]
    current_group_open: u32,
    /// Calls that are not yet Settled.
    #[serde(skip)]
    unsettled_count: u32,
    /// Calls still waiting for an effect request.
    #[serde(skip)]
    undispatched_count: u32,
}

impl PartialEq for ActiveToolBatch {
    fn eq(&self, other: &Self) -> bool {
        self.opened == other.opened
            && self.calls == other.calls
            && self.current_group == other.current_group
            && self.next_source_index == other.next_source_index
            && self.result_message_ids == other.result_message_ids
            && self.fatal_error == other.fatal_error
    }
}

impl Eq for ActiveToolBatch {}

impl ActiveToolBatch {
    /// Build replay state and derived settlement indexes from opened calls.
    #[must_use]
    pub fn new(
        opened: ToolBatchOpened,
        calls: impl Into<Arc<[ActiveToolCall]>>,
        current_group: u32,
        next_source_index: u32,
        result_message_ids: impl Into<Arc<[MessageId]>>,
        fatal_error: Option<ErrorDescriptor>,
    ) -> Self {
        let mut batch = Self {
            opened,
            calls: calls.into(),
            current_group,
            next_source_index,
            result_message_ids: result_message_ids.into(),
            fatal_error,
            effect_index: BTreeMap::new(),
            current_group_open: 0,
            unsettled_count: 0,
            undispatched_count: 0,
        };
        batch.reindex();
        batch
    }

    /// Look up the source-order index for a batch effect.
    #[must_use]
    pub fn call_index(&self, effect_id: EffectId) -> Option<usize> {
        self.effect_index
            .get(&effect_id)
            .copied()
            .map(|index| index as usize)
    }

    /// Borrow the active call for `effect_id`.
    #[must_use]
    pub fn call(&self, effect_id: EffectId) -> Option<&ActiveToolCall> {
        self.call_index(effect_id)
            .and_then(|index| self.calls.get(index))
    }

    /// Whether every call in `group` is Buffered or Settled.
    #[must_use]
    pub fn group_is_terminal(&self, group: u32) -> bool {
        if group == self.current_group {
            return self.current_group_open == 0;
        }
        self.calls.iter().all(|call| {
            call.assigned.group_index != group
                || matches!(
                    call.status,
                    ActiveToolCallStatus::Buffered { .. } | ActiveToolCallStatus::Settled { .. }
                )
        })
    }

    /// Whether every source call has a committed result message.
    #[must_use]
    pub const fn all_settled(&self) -> bool {
        self.unsettled_count == 0
    }

    /// First remaining executable group, if any Undispatched Execute call exists.
    #[must_use]
    pub fn next_executable_group(&self) -> Option<u32> {
        if self.undispatched_count == 0 {
            return None;
        }
        self.calls.iter().find_map(|call| {
            (matches!(call.status, ActiveToolCallStatus::Undispatched)
                && matches!(call.assigned.plan, ToolCallPlan::Execute(_)))
            .then_some(call.assigned.group_index)
        })
    }

    /// Record IDs needed after buffering `target` without allocating a status vector.
    #[must_use]
    pub fn predicted_settlement_counts(&self, target: usize, fatal: bool) -> (usize, usize, usize) {
        let current_complete = self.current_group_completes_after(target);
        let abort_undispatched = fatal && current_complete;
        let start = usize::try_from(self.next_source_index).unwrap_or(self.calls.len());
        let mut messages = 0_usize;
        for (index, call) in self.calls.iter().enumerate().skip(start) {
            if predicted_buffered(index, target, abort_undispatched, &call.status) {
                messages += 1;
            } else {
                break;
            }
        }
        let requests = if !fatal && current_complete {
            next_group_request_count(&self.calls, target)
        } else {
            0
        };
        let remaining = usize::try_from(self.unsettled_count).unwrap_or(0);
        let close = usize::from(remaining == messages);
        (messages, requests, close)
    }

    pub(crate) fn reindex(&mut self) {
        self.effect_index.clear();
        self.current_group_open = 0;
        self.unsettled_count = 0;
        self.undispatched_count = 0;
        for (index, call) in self.calls.iter().enumerate() {
            if let Ok(index) = u32::try_from(index) {
                self.effect_index.insert(call.assigned.effect_id, index);
            }
            let kind = status_kind(&call.status);
            bump(&mut self.undispatched_count, false, kind.undispatched);
            bump(&mut self.unsettled_count, false, kind.unsettled);
            let open = kind.open && call.assigned.group_index == self.current_group;
            bump(&mut self.current_group_open, false, open);
        }
    }

    pub(crate) fn set_call_status(&mut self, index: usize, status: ActiveToolCallStatus) {
        let next = status_kind(&status);
        let (group, previous) = {
            let Some(call) = Arc::make_mut(&mut self.calls).get_mut(index) else {
                return;
            };
            let previous = status_kind(&call.status);
            let group = call.assigned.group_index;
            call.status = status;
            (group, previous)
        };
        self.adjust_counters(group, previous, next);
    }

    pub(crate) fn set_current_group(&mut self, group: u32) {
        self.current_group = group;
        self.current_group_open = self
            .calls
            .iter()
            .filter(|call| call.assigned.group_index == group && status_kind(&call.status).open)
            .count()
            .try_into()
            .unwrap_or(u32::MAX);
    }

    fn current_group_completes_after(&self, target: usize) -> bool {
        let Some(call) = self.calls.get(target) else {
            return self.current_group_open == 0;
        };
        let target_open =
            call.assigned.group_index == self.current_group && status_kind(&call.status).open;
        if target_open {
            self.current_group_open <= 1
        } else {
            self.current_group_open == 0
        }
    }

    fn adjust_counters(&mut self, group: u32, previous: StatusKind, next: StatusKind) {
        bump(
            &mut self.undispatched_count,
            previous.undispatched,
            next.undispatched,
        );
        bump(
            &mut self.unsettled_count,
            previous.unsettled,
            next.unsettled,
        );
        let in_group = group == self.current_group;
        bump(
            &mut self.current_group_open,
            in_group && previous.open,
            in_group && next.open,
        );
    }
}

/// Saturating `+1`/`-1` on a counter when a membership flag flips.
fn bump(counter: &mut u32, was: bool, now: bool) {
    if was != now {
        *counter = if now {
            counter.saturating_add(1)
        } else {
            counter.saturating_sub(1)
        };
    }
}

#[derive(Clone, Copy)]
struct StatusKind {
    undispatched: bool,
    unsettled: bool,
    open: bool,
}

fn status_kind(status: &ActiveToolCallStatus) -> StatusKind {
    StatusKind {
        undispatched: matches!(status, ActiveToolCallStatus::Undispatched),
        unsettled: !matches!(status, ActiveToolCallStatus::Settled { .. }),
        open: matches!(
            status,
            ActiveToolCallStatus::Undispatched | ActiveToolCallStatus::Requested { .. }
        ),
    }
}

fn predicted_buffered(
    index: usize,
    target: usize,
    abort_undispatched: bool,
    status: &ActiveToolCallStatus,
) -> bool {
    index == target
        || matches!(status, ActiveToolCallStatus::Buffered { .. })
        || (abort_undispatched && matches!(status, ActiveToolCallStatus::Undispatched))
}

fn next_group_request_count(calls: &[ActiveToolCall], target: usize) -> usize {
    let next_group = calls.iter().enumerate().find_map(|(index, call)| {
        (index != target
            && matches!(call.status, ActiveToolCallStatus::Undispatched)
            && matches!(call.assigned.plan, ToolCallPlan::Execute(_)))
        .then_some(call.assigned.group_index)
    });
    next_group.map_or(0, |group| {
        calls
            .iter()
            .enumerate()
            .filter(|(index, call)| {
                *index != target
                    && matches!(call.status, ActiveToolCallStatus::Undispatched)
                    && call.assigned.group_index == group
                    && matches!(call.assigned.plan, ToolCallPlan::Execute(_))
            })
            .count()
    })
}

impl<'de> Deserialize<'de> for ActiveToolBatch {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            opened: ToolBatchOpened,
            calls: Arc<[ActiveToolCall]>,
            current_group: u32,
            next_source_index: u32,
            result_message_ids: Arc<[MessageId]>,
            #[serde(default)]
            fatal_error: Option<ErrorDescriptor>,
        }
        let wire = Wire::deserialize(deserializer)?;
        Ok(Self::new(
            wire.opened,
            wire.calls,
            wire.current_group,
            wire.next_source_index,
            wire.result_message_ids,
            wire.fatal_error,
        ))
    }
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
    /// Domain-separated `tool-settlement` digest.
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

    fn id<T: crate::IdTag>(ordinal: u8) -> crate::Id<T> {
        let mut bytes = [0_u8; 16];
        bytes[6] = 0x70;
        bytes[8] = 0x80;
        bytes[15] = ordinal;
        crate::Id::from_bytes(bytes)
    }

    fn synthetic_call(effect_id: EffectId, status: ActiveToolCallStatus) -> ActiveToolCall {
        ActiveToolCall {
            assigned: AssignedToolCall {
                source_index: 0,
                group_index: 0,
                effect_id,
                plan: ToolCallPlan::SyntheticClosure(SyntheticToolClosure {
                    call: crate::ToolCallBlock::try_new(
                        id(3),
                        "t",
                        crate::RawJson::parse("{}").expect("json"),
                    )
                    .expect("call"),
                    execution: ToolExecutionMode::Sequential,
                    failure_policy: ToolFailurePolicy::ReturnToModel,
                    error: crate::ErrorDescriptor::new("x", "x", crate::ErrorCategory::Tool, false)
                        .expect("error"),
                }),
            },
            status,
        }
    }

    #[test]
    fn active_batch_indexes_effect_ids_and_counts() {
        let effect_a = id(1);
        let effect_b = id(2);
        let opened = ToolBatchOpened {
            cycle: 0,
            turn_id: id(4),
            tool_batch_id: id(5),
            source_message_id: id(6),
            calls: Arc::from([]),
            continuation: ToolBatchContinuation::Finalize,
            plan_digest: crate::Digest::raw_json(b"plan"),
        };
        let batch = ActiveToolBatch::new(
            opened,
            vec![
                synthetic_call(
                    effect_a,
                    ActiveToolCallStatus::Buffered {
                        result: crate::ToolResultBlock::try_new(
                            id(3),
                            vec![crate::ContentBlock::Json(crate::JsonBlock::new(
                                crate::RawJson::parse("{}").expect("json"),
                            ))],
                            true,
                        )
                        .expect("result"),
                        settlement_digest: crate::Digest::raw_json(b"s"),
                        synthetic: true,
                        error: None,
                    },
                ),
                synthetic_call(effect_b, ActiveToolCallStatus::Undispatched),
            ],
            0,
            0,
            Arc::from([]),
            None,
        );
        assert_eq!(batch.call_index(effect_a), Some(0));
        assert_eq!(batch.call_index(effect_b), Some(1));
        assert!(!batch.all_settled());
        assert!(!batch.group_is_terminal(0));
        let (messages, requests, close) = batch.predicted_settlement_counts(0, false);
        assert_eq!((messages, requests, close), (1, 0, 0));
    }

    #[test]
    fn predicted_settlement_counts_are_the_tool_settlement_shape_owner() {
        // `apply/shapes.rs::tool_settlement_shape` consumes these counts for
        // expected settlement, request, and close cardinality. Keep the two
        // paths on one owner; do not reintroduce a parallel closer.
        let opened = ToolBatchOpened {
            cycle: 0,
            turn_id: id(4),
            tool_batch_id: id(5),
            source_message_id: id(6),
            calls: Arc::from([]),
            continuation: ToolBatchContinuation::Finalize,
            plan_digest: crate::Digest::raw_json(b"plan"),
        };
        let buffered = synthetic_call(
            id(1),
            ActiveToolCallStatus::Buffered {
                result: crate::ToolResultBlock::try_new(
                    id(3),
                    vec![crate::ContentBlock::Json(crate::JsonBlock::new(
                        crate::RawJson::parse("{}").expect("json"),
                    ))],
                    true,
                )
                .expect("result"),
                settlement_digest: crate::Digest::raw_json(b"s"),
                synthetic: true,
                error: None,
            },
        );
        let requested = synthetic_call(id(2), ActiveToolCallStatus::Undispatched);
        let batch =
            ActiveToolBatch::new(opened, vec![buffered, requested], 0, 0, Arc::from([]), None);
        assert_eq!(batch.predicted_settlement_counts(0, false), (1, 0, 0));
        assert_eq!(batch.predicted_settlement_counts(1, true), (2, 0, 1));
        assert_eq!(batch.predicted_settlement_counts(1, false), (2, 0, 1));
    }
}
