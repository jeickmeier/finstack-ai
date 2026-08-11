//! Runtime ports and effect execution for `finstack-ai`.
//!
//! Owns the six primary port contracts and effect execution. Target drivers
//! are selected by the non-default `native-tokio` and `wasm-host` features.
//!
//! PR-014 adds the journal-store boundary and the authoritative commit loop.
//! PR-015 adds the provider-neutral model port and deterministic stream driver.
//! PR-017 adds bounded native event delivery, batching, and backpressure.
//! PR-018 completes the six-port surface with context, middleware, and observer contracts.

#![warn(missing_docs)]

pub use finstack_ai_kernel::{
    ArtifactRef, BudgetScopeId, CapabilityId, ComponentId, ComponentInvocation, ComponentRef,
    ContentBlock, Digest, EffectCompleted, EffectId, EffectInput, EffectKind, EffectOutputContract,
    EffectOutputKind, EffectPurpose, EffectRelation, EntryId, ErrorCategory, ErrorCode,
    ExternalHandleRef, InteractionKind, InteractionRequest, InvocationRecovery, JsonBlock, LaneId,
    Message, MessageId, Metadata, ModelRequestId, OperationLocator, OutputSpec, PendingModelEffect,
    PipelinePosition, PrincipalRef, ProviderIds, RawJson, ReconciliationPolicy, RecordBody,
    RecordEnvelope, RetryDirective, RetrySafety, RunEvent, RunEventBody, RunEventClass,
    RunEventKind, RunId, Sensitivity, SessionId, Stage, Timestamp, ToolBatchId, ToolCallBlock,
    ToolCallId, ToolCallPlan, ToolExecutionMode, ToolFailurePolicy, ToolId, ToolProgress,
    ToolResultBlock, Usage, ValidatedToolCall, ValidationIssue, ValidationOutcome, Version,
};
pub use finstack_ai_kernel::{
    ModelTextDelta as RunEventModelTextDelta, ProviderHeartbeat as RunEventProviderHeartbeat,
    QueueDepthWarning as RunEventQueueDepthWarning, ReasoningDelta as RunEventReasoningDelta,
};

mod audit;
mod context;
mod coordinator;
mod error;
mod event_hub;
mod id_generation;
mod journal;
mod middleware;
mod model;
mod observer;
mod ports;
mod tool;

#[cfg(feature = "native-tokio")]
mod task;

#[cfg(feature = "native-tokio")]
mod model_runtime;

#[cfg(feature = "native-tokio")]
mod tool_runtime;

#[cfg(feature = "native-tokio")]
mod ingress;

pub use audit::{
    SecurityAuditCategory, SecurityAuditError, SecurityAuditEvent, SecurityAuditHealth,
    SecurityAuditReceipt, SecurityAuditSink,
};

pub use coordinator::{CommitCoordinator, CommitCoordinatorError, CommitOutcome, RunFault};

pub use context::{
    AssembledContext, CONTEXT_BUDGET_EXCEEDED, CONTEXT_COMMIT_REQUIRED,
    CONTEXT_CONFIGURATION_INVALID, CONTEXT_CONTRIBUTION_INVALID, CONTEXT_RECOVERY_UNCERTAIN,
    CapabilityContext, CommittedContextCall, ContextAuthority, ContextBudget, ContextCallContext,
    ContextContribution, ContextError, ContextItem, ContextItemKind, ContextOverflowPolicy,
    ContextProjectionInput, ContextProjectionItem, ContextProjectionSource, ContextProvenance,
    ContextProvider, ContextProviderDescriptor, ContextReconcileResult, ContextRequest,
    ContextTruncationDiagnostic, InvocationResumeAction, PendingContextEffect,
    RecordedContextContribution, assemble_context, assemble_context_projection,
    context_resume_action,
};

pub use error::FrameworkError;
pub use event_hub::{
    EventBatch, EventBatchConfig, EventDeliveryStats, EventFilter, EventHubConfig, EventLagPolicy,
    EventSubscriptionCloseReason, EventSubscriptionConfig, EventSubscriptionError,
    EventSubscriptionStatus, ProgressCoalescing,
};
pub use id_generation::{Clock, IdGenerationError, RandomSource, UuidV7Generator};
pub use journal::{
    JournalStore, LoadRequest, LoadedSession, OpaqueSnapshot, SnapshotReceipt, SnapshotRequest,
    StoreCommitTimestamp, StoreError, StoreHealth,
};
pub use middleware::{
    BeforeModelInput, COMPACTION_BUDGET_EXCEEDED, COMPACTION_MODEL_NOT_AUTHORIZED,
    COMPACTION_RESULT_INVALID, CommittedMiddlewareCall, CompactedSummary, CompactionCheckpoint,
    CompactionEvidence, CompactionModelRequest, CompactionModelResume, CompactionResult,
    CompactionSourceEntry, MIDDLEWARE_COMMIT_REQUIRED, MIDDLEWARE_ORDER_CYCLE,
    MIDDLEWARE_OUTCOME_NOT_ALLOWED, MIDDLEWARE_RESOLUTION_INVALID, Middleware, MiddlewareContext,
    MiddlewareDescriptor, MiddlewareError, MiddlewareOrder, MiddlewareReconcileResult,
    MiddlewareRegistration, MiddlewareRole, OrderTier, PendingMiddlewareEffect, PromptCacheImpact,
    RecordedMiddlewareOutcome, ResolvedMiddleware, ResolvedMiddlewareChain, StageInput, StageMask,
    StageOutcome, compaction_checkpoint_compatible, compaction_projection_digest,
    compaction_protected_set_digest, compaction_source_digest, compaction_summary_digest,
    middleware_resume_action, validate_compaction_model_effect, validate_compaction_result,
    validate_stage_outcome,
};
pub use model::{
    ApprovalMetadata, ApprovalRequirement, AssembledModelStream, AuthorizationContext,
    CancellationSignal, InputCapabilities, LockedModelContextProfile, MODEL_CONTEXT_LIMIT_EXCEEDED,
    MODEL_ESTIMATOR_MISMATCH, MODEL_PROFILE_INVALID, MODEL_PROFILE_OVERRIDE_NOT_ALLOWED,
    MODEL_PROFILE_RELAXATION, MODEL_REQUEST_INVALID, MODEL_RESPONSE_MISMATCH,
    MODEL_STREAM_DUPLICATE_COMPLETION, MODEL_STREAM_ERROR_AFTER_COMPLETION,
    MODEL_STREAM_ITEM_AFTER_COMPLETION, MODEL_STREAM_LIMIT_EXCEEDED,
    MODEL_STREAM_MISSING_COMPLETION, MODEL_TOOL_CALL_ARGUMENTS_INVALID,
    MODEL_TOOL_CALL_DELTA_INVALID, MODEL_TOOL_CALL_INCOMPLETE, MODEL_USAGE_INVALID, Model,
    ModelCallContext, ModelCapabilities, ModelContextProfile, ModelContextProfileOverride,
    ModelDeferral, ModelDescriptor, ModelError, ModelEventStream, ModelName, ModelProgress,
    ModelReconcileResult, ModelRequest, ModelRequestDraft, ModelRequestLimits,
    ModelRequestValidation, ModelResponse, ModelSettings, ModelStreamAssembler, ModelStreamItem,
    ModelStreamLimits, ModelTerminal, ModelTokenEstimate, ModelToolCall, ModelWarmupContext,
    OpaqueProviderEvent, ReasoningDelta, ReconcileContext, RunCallContext, SideEffectClass,
    StructuredOutputCapability, TextDelta, TokenEstimatorRef, TokenEstimatorSource, ToolCallDelta,
    ToolSpec, UsageDelta, resolve_model_context_profile, validate_model_request,
};
pub use observer::{
    NoopObserver, OBSERVER_CAPACITY_EXCEEDED, OBSERVER_CONFIGURATION_INVALID, OBSERVER_UNAVAILABLE,
    Observer, ObserverDescriptor, ObserverError, ObserverEventView, ObserverPayloadMode,
    ReferenceObserver,
};
pub use ports::{PortFuture, PortObject, PortStream};
pub use tool::{
    AssembledToolStream, JsonSchemaToolValidatorCompiler, PendingToolEffect, ResolvedTool,
    ResolvedToolCatalog, TOOL_APPROVAL_REQUIRED, TOOL_ARGUMENTS_INVALID, TOOL_CANCELLED,
    TOOL_DEADLINE_EXCEEDED, TOOL_OUTPUT_INVALID, TOOL_PANICKED, TOOL_POLICY_DENIED,
    TOOL_REGISTRATION_INVALID, TOOL_RESULT_LIMIT_EXCEEDED, TOOL_STREAM_INVALID,
    TOOL_STREAM_LIMIT_EXCEEDED, ToolCallContext, ToolDeferral, ToolError, ToolEventStream,
    ToolExecutionPolicy, ToolPolicyDecision, ToolReconcileResult, ToolResult, ToolStreamAssembler,
    ToolStreamItem, ToolStreamLimits, ToolValidator, ToolValidatorCompiler, Toolset,
    ToolsetDescriptor, ToolsetRegistration, UNKNOWN_TOOL, normalize_tool_result,
};

#[cfg(feature = "native-tokio")]
pub use task::{
    ModelTaskConfig, RunHandle, RunHandleError, RunStatus, RunTaskConfig, RunTaskOwner,
};

#[cfg(feature = "native-tokio")]
pub use event_hub::EventSubscription;

#[cfg(feature = "native-tokio")]
pub use tool_runtime::ToolTaskConfig;

#[cfg(feature = "native-tokio")]
pub use audit::{SecurityAuditGate, SecurityAuditGateError};

#[cfg(feature = "native-tokio")]
pub use ingress::{
    ExternalCompletionRouter, ExternalRouteError, ExternalRouteOutcome, InteractionRouter,
};

#[cfg(feature = "native-tokio")]
pub use id_generation::{OsRandomSource, SystemClock};
