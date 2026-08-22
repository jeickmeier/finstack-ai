//! model-port contract provider-neutral Model port and runtime acceptance proofs.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::time::Duration as StdDuration;

use finstack_ai_kernel::{
    AcceptRun, AllocatedIds, CancelRequested, CancellationInitiator, CancellationRequestTag,
    ComponentId, ContentBlock, Digest, Duration as KernelDuration, EffectInput,
    EffectOutputContract, EffectOutputKind, ErrorCategory, ExternalHandleRef, KernelInput, LaneTag,
    Metadata, ProviderIds, RawJson, ReconciliationPolicy, ReducerStageOutcome, RetryClassification,
    RetryDirective, RetrySafety, RunEventBody, RunEventKind, RunPhase, Sensitivity, SessionTag,
    Stage, TextBlock, TransitionEnv, Usage,
};
use finstack_ai_runtime::commit::{CommitCoordinator, CommitCoordinatorError};
use finstack_ai_runtime::events::{
    EventBatchConfig, EventFilter, EventHubConfig, EventLagPolicy, EventSubscriptionCloseReason,
    EventSubscriptionConfig, ProgressCoalescing,
};
use finstack_ai_runtime::ids::ExternalClock;
use finstack_ai_runtime::ports::journal::{JournalStore, LoadRequest, StoreError};
use finstack_ai_runtime::ports::model::{
    ApprovalGrantMode, CancellationSignal, MODEL_RECONCILIATION_UNSUPPORTED,
    MODEL_RESPONSE_MISMATCH, MODEL_STREAM_DUPLICATE_COMPLETION,
    MODEL_STREAM_ERROR_AFTER_COMPLETION, MODEL_STREAM_ITEM_AFTER_COMPLETION,
    MODEL_STREAM_MISSING_COMPLETION, MODEL_TOOL_CALL_ARGUMENTS_INVALID,
    MODEL_TOOL_CALL_DELTA_INVALID, MODEL_TOOL_CALL_INCOMPLETE, MODEL_USAGE_INVALID, Model,
    ModelDeferral, ModelError, ModelProgress, ModelReconcileResult, ModelResponse,
    ModelResumeAction, ModelStreamAssembler, ModelStreamItem, ModelStreamLimits, ModelTerminal,
    ModelToolCall, ModelWarmupContext, OpaqueProviderEvent, ReasoningDelta, TextDelta,
    ToolCallDelta, UsageDelta, model_resume_action,
};
use finstack_ai_runtime::run::{
    ModelTaskConfig, RunHandleError, RunStatus, RunTaskConfig, RunTaskOwner,
    SameIdentityRetryPolicy, ShutdownOutcome,
};
use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};
use finstack_ai_test::{FixedClock, ScriptedModel, ScriptedModelAction, ScriptedModelPlan};

mod helpers;
use helpers::*;

include!("port_stream.rs");
include!("runtime/progress.rs");
include!("runtime/chunks.rs");
include!("runtime/failures.rs");
include!("runtime/restart.rs");
include!("runtime/cancel.rs");
include!("runtime/shutdown.rs");
include!("runtime/stress.rs");
include!("resume.rs");
include!("cancel.rs");
