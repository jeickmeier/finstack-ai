//! tool-port contract Toolset port, validation, scheduler, ordering, and panic proofs.

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
use finstack_ai_runtime::commit::{CommitCoordinator, CommitCoordinatorError};
use finstack_ai_runtime::events::{
    EventBatchConfig, EventFilter, EventHubConfig, EventLagPolicy, EventSubscriptionConfig,
    ProgressCoalescing,
};
use finstack_ai_runtime::ports::journal::{JournalStore, LoadRequest, StoreError};
use finstack_ai_runtime::ports::model::{
    ApprovalGrantMode, ApprovalRequirement, Model, ModelStreamLimits, ToolDeferralSupport,
    UsageDelta,
};
use finstack_ai_runtime::ports::tool::{
    JsonSchemaToolValidatorCompiler, ResolvedToolCatalog, TOOL_RECONCILIATION_UNSUPPORTED,
    ToolError, ToolExecutionPolicy, ToolPolicyDecision, ToolReconcileResult, ToolResult,
    ToolResumeAction, ToolStreamAssembler, ToolStreamItem, ToolStreamLimits, ToolTerminal,
    ToolValidator, ToolValidatorCompiler, Toolset, ToolsetRegistration, tool_resume_action,
};
use finstack_ai_runtime::run::{
    ModelTaskConfig, RunHandleError, RunStatus, RunTaskConfig, RunTaskOwner,
    SameIdentityRetryPolicy, ToolTaskConfig,
};
use finstack_ai_test::{
    FixedClock, ManualClock, ScriptedModel, ScriptedToolAction, ScriptedToolPlan, ScriptedToolset,
};

mod helpers;
use helpers::*;

include!("catalog.rs");
include!("stream.rs");
include!("runtime.rs");
include!("resume.rs");
include!("deferral_first_pass.rs");
include!("deferral_poll.rs");
