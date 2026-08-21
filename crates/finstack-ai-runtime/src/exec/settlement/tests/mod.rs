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
