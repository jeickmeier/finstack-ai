//! PR-016 Toolset port, validation, scheduler, ordering, and panic proofs.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration as StdDuration;

use finstack_ai_kernel::{
    ActiveToolCallStatus, CancelRequested, CancellationInitiator, ContentBlock, EffectOutputKind,
    KernelInput, Message, Metadata, RawJson, RecordBody, ReducerStageOutcome, RunEventBody,
    RunEventKind, RunPhase, Sensitivity, SessionTag, Stage, ToolCallId, ToolCallPlan,
    ToolExecutionMode, ToolFailurePolicy, ToolProgress, ToolResultBlock, Usage, ValidationOutcome,
};
use finstack_ai_runtime::{
    ApprovalRequirement, CommitCoordinator, CommitCoordinatorError, EventBatchConfig, EventFilter,
    EventHubConfig, EventLagPolicy, EventSubscriptionConfig, JournalStore,
    JsonSchemaToolValidatorCompiler, LoadRequest, Model, ModelStreamLimits, ModelTaskConfig,
    ProgressCoalescing, ResolvedToolCatalog, RunHandleError, RunStatus, RunTaskConfig,
    RunTaskOwner, SameIdentityRetryPolicy, StoreError, TOOL_RECONCILIATION_UNSUPPORTED,
    ToolDeferralSupport, ToolError, ToolExecutionPolicy, ToolPolicyDecision, ToolReconcileResult,
    ToolResult, ToolResumeAction, ToolStreamAssembler, ToolStreamItem, ToolStreamLimits,
    ToolTaskConfig, ToolTerminal, ToolValidator, ToolValidatorCompiler, Toolset,
    ToolsetRegistration, UsageDelta, tool_resume_action,
};
use finstack_ai_test::{
    FixedClock, ScriptedModel, ScriptedToolAction, ScriptedToolPlan, ScriptedToolset,
};

mod helpers;
use helpers::*;

include!("catalog.rs");
include!("stream.rs");
include!("runtime.rs");
include!("resume.rs");
include!("deferral_first_pass.rs");
include!("deferral_poll.rs");
