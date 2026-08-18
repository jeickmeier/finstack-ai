use std::sync::atomic::{AtomicU64, Ordering};

use finstack_ai_kernel::{
    AllocatedIds, BudgetPropagation, CancellationPropagation, ComponentId, ContentBlock,
    DeadlinePropagation, Digest, EffectCompleted, EffectOutputKind, ErrorCategory, ErrorDescriptor,
    Id, IdTag, Kernel, KernelInput, KernelState, LaneTag, Message, MessageRole, Metadata,
    ModelSettled, ModelSettlement, OutputConfiguration, OutputSpec, PrincipalPropagation,
    PrincipalRef, ProviderIds, RawJson, ReducerStageOutcome, RetrySafety, RunAccepted, RunPhase,
    RunPropagationPolicy, RunRelation, RunSecurityContext, RunTag, SessionTag, Stage, StageCursor,
    StageSettled, TerminalCandidate, TextBlock, Timestamp, ToolBatchContinuation, ToolCallBlock,
    ToolCallId, ToolCallPlan, ToolFailurePolicy, TransitionEnv, Version,
};

use super::stage::{
    TOOL_PLAN_COVERAGE_MISMATCH, assert_plan_coverage, run_deadline_outcome, stage_ids,
};
use super::*;
use crate::coordinator::CommitCoordinator;
use crate::{CancellationSignal, ExternalClock, ResolvedToolCatalog};

include!("allocation_unit.rs");
include!("allocation_table.rs");
include!("fixtures.rs");
include!("tool_batch.rs");
include!("deferral.rs");
