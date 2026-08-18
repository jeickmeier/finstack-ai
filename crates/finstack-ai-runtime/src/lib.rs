//! Runtime ports and effect execution for `finstack-ai`.
//!
//! Owns the six primary port contracts and effect execution. Target drivers
//! are selected by the non-default `native-tokio` and `wasm-host` features.
//! `wasm-host` selects the local `PortObject`, `PortFuture`, and `PortStream`
//! aliases. The browser executor and wasm-bindgen surface live in the
//! `finstack-ai-wasm` binding crate.
//!
//! # Module map
//!
//! - `ports` — six primary ports (`model`, `tool`, `context`, `middleware`,
//!   `journal`, `observer`) and shared `PortObject` bounds
//! - `services` — host-owned session, id generation, interaction, agent
//!   invoke, artifact, audit, budget, identity, and composition
//! - `exec` — commit-before-effect, stage fold, run-task owners, and the
//!   event hub
//! - `driver` — target I/O: native-tokio drivers, wasm-host driver, ingress,
//!   workflow, and the SDK native-driver facade

#![warn(missing_docs)]

pub use bytes::Bytes;
pub use finstack_ai_kernel::{
    AGENT_SPEC_DIGEST_SCHEMA_VERSION, AgentId, AppendBatchId, ArtifactId, ArtifactRef, BlobRef,
    BudgetChargeReceipt, BudgetChargeRecorded, BudgetChargeRequest, BudgetReleaseReceipt,
    BudgetReleaseRequest, BudgetRequest, BudgetReservationId, BudgetReservationReceipt,
    BudgetReservationReleased, BudgetReservationReplay, BudgetReservationRequested,
    BudgetReservationSettled, BudgetReserveRequest, BudgetScopeId, BundleId, CapabilityId,
    ChildPlacement, ChildRunLocator, ChildRunPrepared, ComponentId, ComponentInvocation,
    ComponentRef, ContentBlock, ConversationEntry, ConversationError, CostLimit, DOMAIN_AGENT_SPEC,
    Digest, EffectCompleted, EffectDeferred, EffectId, EffectInput, EffectKind,
    EffectOutputContract, EffectOutputKind, EffectPurpose, EffectRelation, EffectRequested,
    EntryBody, EntryId, ErrorCategory, ErrorCode, ExternalEffectCompletionCommand,
    ExternalHandleRef, FinalResultRecorded, InteractionKind, InteractionRequest,
    InteractionResolution, InteractionResolutionCommand, InvocationRecovery, JsonBlock,
    JsonSchemaDraft, KernelState, LaneId, LimitKey, Message, MessageId, MessageRole, Metadata,
    MiddlewareRef, ModelRequestId, OpaqueBlock, OpaquePayload, OperationLocator, OutputEndStrategy,
    OutputSpec, PendingModelEffect, PipelinePosition, PrincipalRef, ProviderIds,
    RECORD_FORMAT_VERSION, RECORD_KIND_VERSION, RawJson, ReconciliationPolicy, RecordBody,
    RecordDraft, RecordEnvelope, RecordId, RemoteRouteRef, RetryDirective, RetrySafety, RunEvent,
    RunEventBody, RunEventClass, RunEventKind, RunId, RunLimits, RunPhase,
    SUBMIT_FINAL_OUTPUT_TOOL, SchemaRef, Sensitivity, SessionId, SessionProjection, Stage,
    StructuredResultSource, TextBlock, Timestamp, ToolBatchId, ToolCallBlock, ToolCallId,
    ToolCallPlan, ToolExecutionMode, ToolFailurePolicy, ToolId, ToolProgress, ToolResultBlock,
    TurnId, Usage, ValidatedToolCall, ValidationIssue, ValidationOutcome, Version,
};
pub use finstack_ai_kernel::{
    ModelTextDelta as RunEventModelTextDelta, ProviderHeartbeat as RunEventProviderHeartbeat,
    QueueDepthWarning as RunEventQueueDepthWarning, ReasoningDelta as RunEventReasoningDelta,
};

mod driver;
mod error;
mod exec;
mod ports;
mod services;

#[cfg(feature = "native-tokio")]
pub(crate) use driver::{ingress, native, workflow};
#[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
pub(crate) use exec::compaction_driver;
#[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
pub(crate) use exec::context_driver;
#[cfg(all(feature = "wasm-host", not(feature = "native-tokio")))]
pub(crate) use exec::host_task;
#[cfg(feature = "native-tokio")]
pub(crate) use exec::task;
pub(crate) use exec::{coordinator, event_hub};
#[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
pub(crate) use exec::{run_types, settlement, stage_settlement};
pub(crate) use ports::{context, journal, middleware, model, observer, tool};
pub(crate) use services::{id_generation, interaction, session};

#[cfg(feature = "wasm-host")]
pub use driver::host_driver;
#[cfg(feature = "native-tokio")]
pub use driver::sdk as native_driver;
pub use exec::middleware_driver;

pub use coordinator::{CommitCoordinator, CommitCoordinatorError, CommitOutcome, RunFault};
pub use interaction::{InteractionResumeAction, interaction_resume_action};
pub use services::agent_invoker::{
    AGENT_INVOKE_CONFLICT, AGENT_INVOKE_INVALID_ACCEPTANCE, AGENT_INVOKE_UNAVAILABLE,
    AgentInvokeError, AgentInvoker, AgentRef, ChildRunContext, ChildRunHandle, ChildRunRequest,
};
pub use services::artifact::{
    ARTIFACT_INTEGRITY_FAILURE, ARTIFACT_INVALID_METADATA, ARTIFACT_NOT_FOUND,
    ARTIFACT_SCOPE_MISMATCH, ARTIFACT_TOO_LARGE, ARTIFACT_UNAVAILABLE, ArtifactError,
    ArtifactMetadata, ArtifactScope, ArtifactStore, MAX_ARTIFACT_BYTES, stage_required_artifact,
    validate_staged_artifact,
};
pub use services::audit::{
    SecurityAuditCategory, SecurityAuditError, SecurityAuditEvent, SecurityAuditHealth,
    SecurityAuditReceipt, SecurityAuditSink,
};
pub use services::budget::{
    BUDGET_CONFLICT, BUDGET_INVALID_RECEIPT, BUDGET_UNAVAILABLE, BUDGET_UNKNOWN, BudgetError,
    BudgetLedger, BudgetReservationState,
};
pub use services::composition::{
    BudgetCoordinator, BudgetOperationIds, ChildCoordinationIds, ChildRunCoordinator,
    CompositionError, child_relation_digest,
};
pub use services::identity_map::{
    ExternalIdentityKey, ExternalIdentityMap, IdentityMapError, MemoryExternalIdentityMap,
};
#[cfg(not(target_arch = "wasm32"))]
pub use services::process_confinement::{
    CONFINEMENT_DENIED, CONFINEMENT_IO, CONFINEMENT_UNAVAILABLE, ConfinedChild, ConfinementBackend,
    ConfinementError, ConfinementProfile, ProcessConfinement,
};
pub use session::{
    LaneAppendIds, LaneCreateIds, LaneInspect, LaneOwner, SessionCreateIds, SessionError,
    SessionRuntime,
};

pub use context::{
    AssembledContext, CONTEXT_BUDGET_EXCEEDED, CONTEXT_COMMIT_REQUIRED,
    CONTEXT_CONFIGURATION_INVALID, CONTEXT_CONTRIBUTION_INVALID, CONTEXT_RECOVERY_UNCERTAIN,
    CapabilityContext, CommittedContextCall, ContextAuthority, ContextBudget, ContextCallContext,
    ContextContribution, ContextError, ContextItem, ContextItemKind, ContextOverflowPolicy,
    ContextProjectionInput, ContextProjectionItem, ContextProjectionSource, ContextProvenance,
    ContextProvider, ContextProviderDescriptor, ContextReconcileResult, ContextRequest,
    ContextTruncationDiagnostic, InvocationResumeAction, PendingContextEffect,
    RecordedContextContribution, assemble_context, assemble_context_projection,
    context_resume_action, map_context_reconcile_result,
};

pub use error::{FrameworkError, PortErrorInvalid};
pub use event_hub::{
    EventBatch, EventBatchConfig, EventDeliveryStats, EventFilter, EventHubConfig, EventLagPolicy,
    EventSubscriptionCloseReason, EventSubscriptionConfig, EventSubscriptionError,
    EventSubscriptionStatus, ProgressCoalescing,
};
pub use id_generation::{Clock, ExternalClock, IdGenerationError, RandomSource, UuidV7Generator};
pub use journal::{
    AcceleratedRestore, IdempotencyHorizon, JournalStore, LoadFromRequest, LoadRequest, LoadWindow,
    LoadedSession, MetadataReceipt, OpaqueSnapshot, PruneReceipt, PruneRequest,
    SCAN_PAGE_MAX_RECORDS, ScanPage, ScanRequest, SnapshotReceipt, SnapshotRequest,
    SnapshotSchedule, StateSnapshotRequest, StoreCommitTimestamp, StoreError, StoreHealth,
    StoreLimits, WriteMetadataRequest,
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
    middleware_resume_action, stage_name as middleware_stage_name,
    validate_compaction_model_effect, validate_compaction_result, validate_stage_outcome,
};
#[cfg(feature = "native-tokio")]
#[doc(hidden)]
pub use native::manual_drive::{
    ManualDriveAction, ManualDriveController, ManualDriveEffect, ManualDriveError,
    ManualDrivePermit,
};
// The chain driver's supported entry points, re-exported so
// `tools/compat/public_items.py` tracks them: it scrapes braced `pub use`
// blocks only, so the rest of `middleware_driver`'s `pub` surface is public
// through `pub mod` but unfrozen. See the module contract.
pub use middleware_driver::{MiddlewareStageContext, invoke_middleware_stage};
pub use model::{
    AnthropicMessagesAssembly, ApprovalMetadata, ApprovalRequirement, AssembledModelStream,
    AuthorizationContext, CancellationSignal, InputCapabilities, LockedModelContextProfile,
    MODEL_CONTEXT_LIMIT_EXCEEDED, MODEL_ESTIMATOR_MISMATCH, MODEL_PROFILE_INVALID,
    MODEL_PROFILE_OVERRIDE_NOT_ALLOWED, MODEL_PROFILE_RELAXATION, MODEL_RECONCILIATION_UNSUPPORTED,
    MODEL_REQUEST_INVALID, MODEL_RESPONSE_MISMATCH, MODEL_STREAM_DUPLICATE_COMPLETION,
    MODEL_STREAM_ERROR_AFTER_COMPLETION, MODEL_STREAM_ITEM_AFTER_COMPLETION,
    MODEL_STREAM_LIMIT_EXCEEDED, MODEL_STREAM_MISSING_COMPLETION,
    MODEL_TOOL_CALL_ARGUMENTS_INVALID, MODEL_TOOL_CALL_DELTA_INVALID, MODEL_TOOL_CALL_INCOMPLETE,
    MODEL_USAGE_INVALID, Model, ModelCallContext, ModelCapabilities, ModelContextProfile,
    ModelContextProfileOverride, ModelDeferral, ModelDescriptor, ModelError, ModelEventStream,
    ModelName, ModelProgress, ModelReconcileResult, ModelRequest, ModelRequestDraft,
    ModelRequestLimits, ModelRequestValidation, ModelResponse, ModelResumeAction, ModelSettings,
    ModelStreamAssembler, ModelStreamItem, ModelStreamLimits, ModelTerminal, ModelTokenEstimate,
    ModelToolCall, ModelWarmupContext, NdjsonError, NdjsonParser, OllamaChatAssembly,
    OllamaReplayEntry, OpaqueProviderEvent, OpenAiChatAssembly, OpenAiResponsesAssembly,
    ReasoningDelta, ReconcileContext, RunCallContext, SECRET_MAX_BYTES, SideEffectClass, SseEvent,
    SseEventParser, SseFrameError, SseFrameParser, SseParseError, StreamNormError, StreamNormKind,
    StructuredOutputCapability, TextDelta, TokenEstimatorRef, TokenEstimatorSource, ToolCallDelta,
    ToolDeferralSupport, ToolSpec, UsageDelta, map_model_reconcile_result, model_resume_action,
    model_retry_allowed, resolve_model_context_profile, secret_is_valid, validate_model_request,
};
pub use observer::export::{
    diagnostic_contains, journal_export_jsonl, observer_events_jsonl, support_bundle_versions,
};
pub use observer::queue::{
    OBSERVER_QUEUE_OVERFLOW, ObserverBackpressure, ObserverDiagnostic, ObserverQueue,
    ObserverQueuePush,
};
pub use observer::{
    NoopObserver, OBSERVER_CAPACITY_EXCEEDED, OBSERVER_CONFIGURATION_INVALID, OBSERVER_UNAVAILABLE,
    Observer, ObserverDescriptor, ObserverError, ObserverEventView, ObserverPayloadMode,
    ReferenceObserver,
};
pub use ports::{PortFuture, PortObject, PortStream};
pub use tool::{
    AssembledToolStream, JsonSchemaToolValidatorCompiler, MCP_SAMPLING_REQUIRED,
    MCP_SAMPLING_UNAVAILABLE, MCP_SAMPLING_UNSUPPORTED, NestedSample, PendingToolEffect,
    ResolvedTool, ResolvedToolCatalog, TOOL_APPROVAL_REQUIRED, TOOL_ARGUMENTS_INVALID,
    TOOL_CANCELLED, TOOL_DEADLINE_EXCEEDED, TOOL_DEFERRAL_EXPIRED, TOOL_DEFERRAL_INVALID,
    TOOL_DEFERRAL_NOT_DECLARED, TOOL_INTERACTION_REQUIRED, TOOL_OUTPUT_INVALID, TOOL_PANICKED,
    TOOL_POLICY_DENIED, TOOL_RECONCILIATION_UNSUPPORTED, TOOL_REGISTRATION_INVALID,
    TOOL_RESULT_LIMIT_EXCEEDED, TOOL_STREAM_INVALID, TOOL_STREAM_LIMIT_EXCEEDED, ToolCallContext,
    ToolCatalogPlan, ToolDeferral, ToolError, ToolEventStream, ToolExecutionPolicy,
    ToolPolicyDecision, ToolReconcileResult, ToolResult, ToolResumeAction, ToolStreamAssembler,
    ToolStreamItem, ToolStreamLimits, ToolTerminal, ToolValidator, ToolValidatorCompiler, Toolset,
    ToolsetDescriptor, ToolsetRegistration, UNKNOWN_TOOL, map_tool_reconcile_result,
    normalize_tool_result, tool_resume_action, tool_retry_allowed,
};

#[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
pub use run_types::{
    ModelTaskConfig, RunHandleError, RunStatus, RunTaskConfig, SameIdentityRetryPolicy,
    ShutdownOutcome, ShutdownReport, TimerDiagnostics, ToolTaskConfig,
};

#[cfg(feature = "native-tokio")]
pub use task::{RunHandle, RunTaskOwner};

#[cfg(all(feature = "wasm-host", not(feature = "native-tokio")))]
pub use host_task::{RunHandle, RunTaskOwner};

#[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
pub use event_hub::EventSubscription;

#[cfg(feature = "native-tokio")]
pub use native::time::{
    DeadlineDiagnostic, MonotonicDeadline, NoRetryJitter, RetryJitterSource, RuntimeTimeError,
    retry_backoff_with_jitter,
};

#[cfg(feature = "native-tokio")]
pub use services::audit::{SecurityAuditGate, SecurityAuditGateError};

#[cfg(feature = "native-tokio")]
pub use ingress::{
    ExternalCompletionRouter, ExternalRouteError, ExternalRouteOutcome, InteractionRouter,
};

#[cfg(feature = "native-tokio")]
pub use workflow::{
    WorkflowCheckpoint, WorkflowDriverError, WorkflowRetryDecision, WorkflowSession, WorkflowWait,
    classify_wait, resolve_checkpoint_sequence, retry_decision,
};

/// In-process reference name for [`WorkflowSession`].
#[cfg(feature = "native-tokio")]
pub type LocalWorkflowDriver = WorkflowSession;

#[cfg(feature = "native-tokio")]
pub use id_generation::{OsRandomSource, SystemClock};
