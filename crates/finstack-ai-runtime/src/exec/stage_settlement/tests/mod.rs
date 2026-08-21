use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use finstack_ai_kernel::{
    AcceptRun, AllocatedIds, AppendBatchId, AppendRequest, BudgetPropagation,
    CancellationPropagation, CommittedBatch, ContentBlock, DeadlinePropagation, Digest, EntryId,
    ErrorCategory, ErrorDescriptor, Id, IdTag, KernelInput, LaneTag, Message, MessageRole,
    Metadata, PrincipalPropagation, PrincipalRef, ProviderIds, RawJson, RecordEnvelope,
    ReducerStageOutcome, RunAccepted, RunLimits, RunPhase, RunPropagationPolicy, RunRelation,
    RunRelationKind, RunSecurityContext, RunTag, Sensitivity, SessionTag, Stage, StageCursor,
    StageSettled, TextBlock, Timestamp, ToolId, TransitionEnv, Version,
};

use super::apply::{apply_context_prepared, apply_fold, apply_model_draft};
use super::codec::{canonical_draft, canonical_message};
use super::driver::{component_input, folds_at, run_stage_chain, settle_facade_stage};
use super::input::{stage_input, trailing_role_run};
use super::submit::{folded_allocation_error, limit_crossing_allocation};
use super::tool_batch::tool_batch_policy;
use super::*;
use crate::context::{
    ContextAuthority, ContextCallContext, ContextContribution, ContextError, ContextItem,
    ContextItemKind, ContextProvenance, ContextProvider, ContextProviderDescriptor, ContextRequest,
};
use crate::middleware::{
    BeforeModelInput, MiddlewareDescriptor, MiddlewareOrder, MiddlewareRegistration,
    MiddlewareRole, OrderTier, ResolvedMiddlewareChain, StageInput, StageMask, StageOutcome,
};
use crate::middleware_driver::{
    MIDDLEWARE_STAGE_BOUNDS_EXCEEDED, MIDDLEWARE_STAGE_UNLANDABLE, StageDriver, StageFold,
    StageTerminal,
};
use crate::settlement::SettlementSources;
use crate::{
    CancellationSignal, CommitCoordinator, ExternalClock, IdGenerationError, JournalStore,
    LoadRequest, LoadedSession, PortFuture, RandomSource, SnapshotReceipt, SnapshotRequest,
    StoreError, StoreHealth,
};

include!("fixtures.rs");
include!("passthrough.rs");
include!("before_model.rs");
include!("compaction.rs");
include!("context_providers.rs");
include!("prepare_context.rs");
include!("recovery.rs");
include!("relation_depth.rs");
include!("stage_input.rs");
include!("terminal_fold.rs");
