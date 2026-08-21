use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use finstack_ai_kernel::{
    ActiveToolBatch, ActiveToolCall, ActiveToolCallStatus, AllocatedIds, AuthorizationEvidence,
    BudgetPropagation, CancellationPropagation, ComponentId, ContentBlock, DeadlinePropagation,
    Digest, EffectCompleted, EffectDeferred, EffectOutputKind, ErrorCategory, ErrorDescriptor,
    ExternalHandleRef, Id, IdTag, InteractionKind, InteractionResolution, InteractionSettled,
    Kernel, KernelInput, KernelState, LaneTag, Message, MessageRole, Metadata, ModelSettled,
    ModelSettlement, OutputConfiguration, OutputSpec, PrincipalPropagation, PrincipalRef,
    ProviderIds, RawJson, ReconciliationPolicy, ReducerStageOutcome, RetrySafety, RunAccepted,
    RunPhase, RunPropagationPolicy, RunRelation, RunSecurityContext, RunTag, SessionTag, Stage,
    StageCursor, StageSettled, TerminalCandidate, TextBlock, Timestamp, ToolBatchContinuation,
    ToolCallBlock, ToolCallId, ToolCallPlan, ToolFailurePolicy, TransitionEnv, ValidatedToolCall,
    Version,
};

use super::stage::{
    TOOL_PLAN_COVERAGE_MISMATCH, assert_plan_coverage, run_deadline_outcome, stage_ids,
};
use super::*;
use crate::coordinator::CommitCoordinator;
use crate::{ApprovalGrantMode, CancellationSignal, ExternalClock, ResolvedToolCatalog};

include!("allocation_unit.rs");
include!("allocation_table.rs");
include!("fixtures.rs");
include!("tool_batch.rs");
include!("approval.rs");
include!("deferral.rs");
include!("poll.rs");
mod artifact_ownership;

#[tokio::test]
async fn cancellation_without_outstanding_effects_reaches_a_durable_terminal() {
    let store = Arc::new(MemoryStore::new());
    let mut coordinator =
        CommitCoordinator::new(Arc::clone(&store) as Arc<dyn crate::JournalStore>);
    coordinator
        .submit(
            env(1_000, &[1], &[1], &[], &[], &[], &[], 101),
            KernelInput::AcceptRun(finstack_ai_kernel::AcceptRun {
                session_id: fixed_id::<SessionTag>(1),
                lane_id: fixed_id::<LaneTag>(2),
                accepted: acceptance(None),
            }),
        )
        .await
        .expect("accept run");
    let sources = sources_at(1_100);
    let input = KernelInput::CancelRequested(finstack_ai_kernel::CancelRequested {
        initiator: finstack_ai_kernel::CancellationInitiator::RuntimeShutdown,
        reason: None,
    });
    let ids = super::ids::allocate_for_runtime_input(
        &coordinator,
        fixed_timestamp(1_100),
        &input,
        &sources,
    )
    .expect("cancellation ids");
    coordinator
        .submit(
            TransitionEnv {
                now: fixed_timestamp(1_100),
                ids,
            },
            input,
        )
        .await
        .expect("request cancellation");

    drain_idle_cancellation(&mut coordinator, &sources, false)
        .await
        .expect("reconcile cancellation");

    assert!(matches!(
        coordinator.state().terminal,
        Some(finstack_ai_kernel::TerminalState::Cancelled(_))
    ));
}
