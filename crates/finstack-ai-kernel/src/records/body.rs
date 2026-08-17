//! Owned record bodies.

use crate::conversation::ConversationEntry;
use crate::effects::{
    EffectCancelled, EffectCompleted, EffectDeferred, EffectFailed, EffectRequested,
    InteractionCancelled, InteractionExpired, InteractionRequest, InteractionResolution,
};
use crate::lifecycle::{
    ContextPrepared, EntryAppended, RetryScheduled, RunCancelled, RunCompleted, RunFailed,
    RunSuspended, StageOutcomeRecorded, TimerFired,
};
use crate::policy::CapabilitiesActivated;
use crate::policy::LimitReached;
use crate::policy::OutputValidationFailed;
use crate::policy::{
    BudgetChargeRecorded, BudgetReservationReleased, BudgetReservationRequested,
    BudgetReservationSettled,
};
use crate::policy::{FinalResultRecorded, OutputConfiguration};
use crate::records::{LaneCreated, LaneMoved, SessionCreated, SnapshotWritten};
use crate::run::ExternalCommandRejected;
use crate::run::{CancellationReconciled, CancellationRequested, ChildRunPrepared, RunAccepted};
use crate::tools::{ToolBatchClosed, ToolBatchOpened, ToolCallSettled};
use serde::{Deserialize, Serialize};

use super::RECORD_KIND_VERSION;
use super::error::RecordError;

/// Complete record bodies owned through PR-022.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub enum RecordBody {
    /// Run accepted.
    RunAccepted(RunAccepted),
    /// Effect requested.
    EffectRequested(EffectRequested),
    /// Effect deferred.
    EffectDeferred(EffectDeferred),
    /// Effect completed.
    EffectCompleted(EffectCompleted),
    /// Effect failed.
    EffectFailed(EffectFailed),
    /// Effect cancelled.
    EffectCancelled(EffectCancelled),
    /// Interaction requested.
    InteractionRequested(InteractionRequest),
    /// Interaction resolved.
    InteractionResolved(InteractionResolution),
    /// Interaction expired.
    InteractionExpired(InteractionExpired),
    /// Interaction cancelled.
    InteractionCancelled(InteractionCancelled),
    /// Aggregate stage outcome.
    StageOutcomeRecorded(StageOutcomeRecorded),
    /// Prepared model context.
    ContextPrepared(ContextPrepared),
    /// Final assistant message append.
    EntryAppended(EntryAppended),
    /// Complete source-ordered tool-batch plan.
    ToolBatchOpened(ToolBatchOpened),
    /// Source-order tool result finalization.
    ToolCallSettled(ToolCallSettled),
    /// Complete tool-batch closure.
    ToolBatchClosed(ToolBatchClosed),
    /// Durable cancellation intent.
    CancellationRequested(CancellationRequested),
    /// Cumulative cancellation reconciliation.
    CancellationReconciled(CancellationReconciled),
    /// Hard limit crossing.
    LimitReached(LimitReached),
    /// Semantic retry and timer intent.
    RetryScheduled(RetryScheduled),
    /// Semantic timer firing.
    TimerFired(TimerFired),
    /// Non-terminal suspension.
    RunSuspended(RunSuspended),
    /// Successful run terminal.
    RunCompleted(RunCompleted),
    /// Failed run terminal.
    RunFailed(RunFailed),
    /// Cancelled run terminal.
    RunCancelled(RunCancelled),
    /// Frozen run-level output configuration.
    OutputConfigured(OutputConfiguration),
    /// Complete immutable resolved capability plan activation.
    CapabilitiesActivated(CapabilitiesActivated),
    /// Validator-independent valid final structured result.
    FinalResultRecorded(FinalResultRecorded),
    /// Validator-independent invalid structured result and retry feedback.
    OutputValidationFailed(OutputValidationFailed),
    /// Known authorized external command rejected by semantic validation.
    ExternalCommandRejected(ExternalCommandRejected),
    /// Complete parent-owned child locator committed before invocation.
    ChildRunPrepared(ChildRunPrepared),
    /// Shared-budget reservation intent committed with child preparation.
    BudgetReservationRequested(BudgetReservationRequested),
    /// Shared-budget reservation receipt committed before child acceptance.
    BudgetReservationSettled(BudgetReservationSettled),
    /// Shared-budget charge receipt committed after an idempotent charge.
    BudgetChargeRecorded(BudgetChargeRecorded),
    /// Shared-budget release receipt committed after terminal release intent.
    BudgetReservationReleased(BudgetReservationReleased),
    /// Session created.
    SessionCreated(SessionCreated),
    /// Lane created.
    LaneCreated(LaneCreated),
    /// Lane leaf moved.
    LaneMoved(LaneMoved),
    /// Disposable snapshot written.
    SnapshotWritten(SnapshotWritten),
    /// Immutable conversation-tree entry (TDD §24.1).
    ConversationEntry(ConversationEntry),
}

impl RecordBody {
    /// Number of derived durable events for `kind_version`.
    ///
    /// # Errors
    ///
    /// Returns [`RecordError::UnsupportedKindVersion`] when unknown.
    pub fn derived_event_count(&self, kind_version: u16) -> Result<usize, RecordError> {
        if kind_version != RECORD_KIND_VERSION {
            return Err(RecordError::UnsupportedKindVersion { kind_version });
        }
        Ok(match self {
            Self::StageOutcomeRecorded(_)
            | Self::ContextPrepared(_)
            | Self::ToolBatchOpened(_)
            | Self::ToolBatchClosed(_)
            | Self::CancellationRequested(_)
            | Self::CancellationReconciled(_)
            | Self::RetryScheduled(_)
            | Self::TimerFired(_)
            | Self::OutputConfigured(_)
            | Self::CapabilitiesActivated(_)
            | Self::FinalResultRecorded(_)
            | Self::OutputValidationFailed(_)
            | Self::ExternalCommandRejected(_)
            | Self::ChildRunPrepared(_)
            | Self::BudgetReservationRequested(_)
            | Self::BudgetReservationSettled(_)
            | Self::BudgetChargeRecorded(_)
            | Self::BudgetReservationReleased(_)
            | Self::SessionCreated(_)
            | Self::LaneCreated(_)
            | Self::LaneMoved(_)
            | Self::SnapshotWritten(_)
            | Self::ConversationEntry(_) => 0,
            Self::ToolCallSettled(_) => 2,
            _ => 1,
        })
    }

    /// Stable body kind name.
    #[must_use]
    pub const fn kind_name(&self) -> &'static str {
        match self {
            Self::RunAccepted(_) => "run_accepted",
            Self::EffectRequested(_) => "effect_requested",
            Self::EffectDeferred(_) => "effect_deferred",
            Self::EffectCompleted(_) => "effect_completed",
            Self::EffectFailed(_) => "effect_failed",
            Self::EffectCancelled(_) => "effect_cancelled",
            Self::InteractionRequested(_) => "interaction_requested",
            Self::InteractionResolved(_) => "interaction_resolved",
            Self::InteractionExpired(_) => "interaction_expired",
            Self::InteractionCancelled(_) => "interaction_cancelled",
            Self::StageOutcomeRecorded(_) => "stage_outcome_recorded",
            Self::ContextPrepared(_) => "context_prepared",
            Self::EntryAppended(_) => "entry_appended",
            Self::ToolBatchOpened(_) => "tool_batch_opened",
            Self::ToolCallSettled(_) => "tool_call_settled",
            Self::ToolBatchClosed(_) => "tool_batch_closed",
            Self::CancellationRequested(_) => "cancellation_requested",
            Self::CancellationReconciled(_) => "cancellation_reconciled",
            Self::LimitReached(_) => "limit_reached",
            Self::RetryScheduled(_) => "retry_scheduled",
            Self::TimerFired(_) => "timer_fired",
            Self::RunSuspended(_) => "run_suspended",
            Self::RunCompleted(_) => "run_completed",
            Self::RunFailed(_) => "run_failed",
            Self::RunCancelled(_) => "run_cancelled",
            Self::OutputConfigured(_) => "output_configured",
            Self::CapabilitiesActivated(_) => "capabilities_activated",
            Self::FinalResultRecorded(_) => "final_result_recorded",
            Self::OutputValidationFailed(_) => "output_validation_failed",
            Self::ExternalCommandRejected(_) => "external_command_rejected",
            Self::ChildRunPrepared(_) => "child_run_prepared",
            Self::BudgetReservationRequested(_) => "budget_reservation_requested",
            Self::BudgetReservationSettled(_) => "budget_reservation_settled",
            Self::BudgetChargeRecorded(_) => "budget_charge_recorded",
            Self::BudgetReservationReleased(_) => "budget_reservation_released",
            Self::SessionCreated(_) => "session_created",
            Self::LaneCreated(_) => "lane_created",
            Self::LaneMoved(_) => "lane_moved",
            Self::SnapshotWritten(_) => "snapshot_written",
            Self::ConversationEntry(_) => "conversation_entry",
        }
    }

    /// Session, lane, and snapshot bodies that omit `run_id` and do not mutate run state.
    #[must_use]
    pub const fn is_structural(&self) -> bool {
        matches!(
            self,
            Self::SessionCreated(_)
                | Self::LaneCreated(_)
                | Self::LaneMoved(_)
                | Self::SnapshotWritten(_)
                | Self::ConversationEntry(_)
        )
    }
}
