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
// Process confinement talks to Landlock, Seatbelt, and Windows job objects.
#![warn(clippy::float_cmp)]
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
#![deny(clippy::panic)]
#![deny(clippy::unreachable)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::indexing_slicing,
        clippy::float_cmp,
    )
)]
// Allow expect() in doc tests (they are test code)
#![doc(test(attr(allow(clippy::expect_used))))]

pub use bytes::Bytes;
#[allow(
    unused_imports,
    reason = "feature-gated modules consume different kernel names"
)]
pub(crate) use finstack_ai_kernel::{
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
#[allow(
    unused_imports,
    reason = "feature-gated modules consume different kernel names"
)]
pub(crate) use finstack_ai_kernel::{
    ModelTextDelta as RunEventModelTextDelta, ProviderHeartbeat as RunEventProviderHeartbeat,
    QueueDepthWarning as RunEventQueueDepthWarning, ReasoningDelta as RunEventReasoningDelta,
};

mod driver;
mod error;
mod exec;
mod ports;
mod services;
#[cfg(feature = "native-tokio")]
#[doc(hidden)]
pub mod testing;

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
#[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
pub(crate) use exec::middleware_driver;

pub use coordinator::{CommitCoordinator, CommitCoordinatorError, CommitOutcome, RunFault};
pub use interaction::{InteractionResumeAction, interaction_resume_action};
pub use services::agent_invoker::{
    AGENT_INVOKE_INVALID_ACCEPTANCE, AgentInvokeError, AgentInvoker, AgentRef, ChildRunContext,
    ChildRunHandle, ChildRunPolicy, ChildRunRequest, ChildRunStatus,
};
pub use services::artifact::{
    ARTIFACT_CAPACITY_EXCEEDED, ARTIFACT_INTEGRITY_FAILURE, ARTIFACT_SCOPE_MISMATCH, ArtifactError,
    ArtifactGcReport, ArtifactMetadata, ArtifactOwnerId, ArtifactPersistence, ArtifactRead,
    ArtifactScope, ArtifactStore, ArtifactStoreDescriptor, ArtifactStoreLimits,
    DEFAULT_ARTIFACT_ORPHAN_GRACE_MS, MAX_ARTIFACT_BYTES, MAX_ARTIFACT_GC_BATCH,
    MAX_ARTIFACT_OWNERS, MAX_ARTIFACTS, MAX_TOTAL_ARTIFACT_BYTES, artifact_storage_key,
    build_artifact_ref, get_required_artifact, stage_required_artifact, validate_artifact_scope,
    validate_retrieved_artifact, validate_staged_artifact,
};
#[cfg(feature = "native-tokio")]
pub use services::audit::SecurityAuditGateHealth;
pub use services::audit::{
    SecurityAuditCategory, SecurityAuditError, SecurityAuditEvent, SecurityAuditHealth,
    SecurityAuditReceipt, SecurityAuditSink,
};
pub use services::budget::{BudgetError, BudgetLedger, BudgetReservationState};
#[cfg(feature = "native-tokio")]
pub use services::child_starter::{ChildRunStartRequest, ChildRunStarter};
pub use services::composition::{
    BudgetCoordinator, BudgetOperationIds, ChildCoordinationIds, ChildRunCoordinator,
    CompositionError, child_relation_digest,
};
pub use services::identity_map::{
    ExternalIdentityKey, ExternalIdentityMap, IdentityMapError, MemoryExternalIdentityMap,
};
pub use services::object::{
    MAX_OBJECT_KEY_BYTES, OBJECT_CONFLICT, OBJECT_INTEGRITY_FAILURE, OBJECT_INVALID_KEY,
    OBJECT_INVALID_METADATA, OBJECT_IO_FAILURE, OBJECT_NOT_FOUND, OBJECT_SCOPE_MISMATCH,
    OBJECT_TOO_LARGE, OBJECT_UNAVAILABLE, OBJECT_UNSUPPORTED, ObjectEntry, ObjectError, ObjectKey,
    ObjectMetadata, ObjectPage, ObjectRef, ObjectScope, ObjectStore, ObjectStoreLimits, PageToken,
    PresignedUrl, PutPayload, physical_object_key, validate_object_metadata,
};
#[cfg(not(target_arch = "wasm32"))]
pub use services::process_confinement::{
    CONFINEMENT_UNAVAILABLE, ConfinedChild, ConfinementBackend, ConfinementError,
    ConfinementProfile, ProcessConfinement, WindowsLpacProfile,
};
#[cfg(not(target_arch = "wasm32"))]
pub use services::process_confinement::{configure_process_tree, terminate_process_tree};
pub use session::{
    LaneAppendIds, LaneCreateIds, LaneInspect, SessionCreateIds, SessionError, SessionRuntime,
};

pub use context::{
    AssembledContext, CONTEXT_BUDGET_EXCEEDED, CONTEXT_COMMIT_REQUIRED,
    CONTEXT_CONFIGURATION_INVALID, CONTEXT_CONTRIBUTION_INVALID, CommittedContextCall,
    ContextAuthority, ContextBudget, ContextCallContext, ContextContribution, ContextError,
    ContextItem, ContextItemKind, ContextOverflowPolicy, ContextProvenance, ContextProvider,
    ContextProviderDescriptor, ContextReconcileResult, ContextRequest, ContextTruncationDiagnostic,
    InvocationResumeAction, PendingContextEffect, RecordedContextContribution, assemble_context,
};

pub use error::PortErrorInvalid;
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
    SnapshotSchedule, StateSnapshotRequest, StoreError, StoreHealth, StoreLimits,
    WriteMetadataRequest,
};
pub use middleware::{
    BeforeModelInput, BeforeToolBatchInput, COMPACTION_BUDGET_EXCEEDED,
    COMPACTION_MODEL_NOT_AUTHORIZED, COMPACTION_RESULT_INVALID, CompactedSummary,
    CompactionCheckpoint, CompactionEvidence, CompactionModelRequest, CompactionModelResume,
    CompactionResult, CompactionSourceEntry, MIDDLEWARE_OUTCOME_NOT_ALLOWED, Middleware,
    MiddlewareContext, MiddlewareDescriptor, MiddlewareError, MiddlewareOrder,
    MiddlewareRegistration, MiddlewareRole, OrderTier, PromptCacheImpact, ResolvedMiddleware,
    ResolvedMiddlewareChain, StageInput, StageMask, StageOutcome,
    authorize_compaction_model_request, compaction_checkpoint_compatible,
    compaction_projection_digest, compaction_protected_set_digest, compaction_source_digest,
    compaction_summary_digest, validate_compaction_result, validate_stage_outcome,
};
#[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
pub(crate) use model::MODEL_PROFILE_INVALID;
pub use model::{
    ApprovalGrantMode, ApprovalMetadata, ApprovalRequirement, AssembledModelStream, Authentication,
    AuthorizationContext, CancellationSignal, CredentialReference, CredentialRejected,
    CredentialStore, InputCapabilities, LockedModelContextProfile,
    MODEL_RECONCILIATION_UNSUPPORTED, MODEL_REQUEST_INVALID, MODEL_RESPONSE_MISMATCH,
    MODEL_STREAM_DUPLICATE_COMPLETION, MODEL_STREAM_ERROR_AFTER_COMPLETION,
    MODEL_STREAM_ITEM_AFTER_COMPLETION, MODEL_STREAM_LIMIT_EXCEEDED,
    MODEL_STREAM_MISSING_COMPLETION, MODEL_TOOL_CALL_ARGUMENTS_INVALID,
    MODEL_TOOL_CALL_DELTA_INVALID, MODEL_TOOL_CALL_INCOMPLETE, MODEL_USAGE_INVALID,
    MediaResolveError, MediaResolveKind, MediaResolver, Model, ModelCallContext, ModelCapabilities,
    ModelContextProfile, ModelContextProfileOverride, ModelDeferral, ModelDescriptor, ModelError,
    ModelEventStream, ModelName, ModelProgress, ModelReconcileResult, ModelRequest,
    ModelRequestDraft, ModelRequestLimits, ModelResponse, ModelResumeAction, ModelSettings,
    ModelStreamAssembler, ModelStreamItem, ModelStreamLimits, ModelTerminal, ModelTokenEstimate,
    ModelToolCall, ModelWarmupContext, OpaqueProviderEvent, ReadyModel, ReasoningDelta,
    ReconcileContext, ResolveDraftMediaError, ResolvedMedia, RunCallContext, SECRET_MAX_BYTES,
    SecretRejected, SecretString, SideEffectClass, StructuredOutputCapability, TextDelta,
    TokenEstimatorRef, TokenEstimatorSource, ToolCallDelta, ToolDeferralSupport, ToolSpec,
    UsageDelta, map_model_reconcile_result, model_resume_action, model_retry_allowed,
    resolve_draft_media, resolve_model_context_profile, secret_is_valid, thinking_level_budget,
};
#[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
pub(crate) use model::{parse_committed_model_request, stable_model_dispatch_code};
pub use observer::export::{journal_export_jsonl, observer_events_jsonl, support_bundle_versions};
pub use observer::{
    NoopObserver, OBSERVER_DELIVERY_FAILED, OBSERVER_SHUTDOWN_TIMEOUT,
    OBSERVER_SUBSCRIPTION_FAILED, Observer, ObserverDescriptor, ObserverDiagnostic,
    ObserverDiagnostics, ObserverError, ObserverEventView, ObserverPayloadMode,
};
pub use ports::{PortFuture, PortObject, PortStream};
pub use tool::{
    ApprovalState, AssembledToolStream, JsonSchemaToolValidatorCompiler, MCP_SAMPLING_REQUIRED,
    MCP_SAMPLING_UNAVAILABLE, NestedSample, PendingToolEffect, ResolvedTool, ResolvedToolCatalog,
    TOOL_CANCELLED, TOOL_DEADLINE_EXCEEDED, TOOL_DEFERRAL_EXPIRED, TOOL_INTERACTION_REQUIRED,
    TOOL_OUTPUT_INVALID, TOOL_RECONCILIATION_UNSUPPORTED, ToolCallContext, ToolCatalogPlan,
    ToolDeferral, ToolError, ToolEventStream, ToolExecutionPolicy, ToolPolicyDecision,
    ToolReconcileResult, ToolResult, ToolResumeAction, ToolStreamAssembler, ToolStreamItem,
    ToolStreamLimits, ToolTerminal, ToolValidator, ToolValidatorCompiler, Toolset,
    ToolsetDescriptor, ToolsetRegistration, map_tool_reconcile_result, normalize_tool_result,
    tool_resume_action, tool_retry_allowed, verify_authority,
};
#[cfg(feature = "native-tokio")]
#[cfg(feature = "native-tokio")]
pub(crate) use tool::{TOOL_PANICKED, TOOL_STREAM_INVALID};

#[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
pub use run_types::{
    ModelTaskConfig, RetryBackoffPolicy, RunHandleError, RunStatus, RunTaskConfig,
    SameIdentityRetryPolicy, ShutdownOutcome, ShutdownReport, TimerDiagnostics, ToolTaskConfig,
};

#[cfg(feature = "native-tokio")]
pub use task::{RunHandle, RunTaskOwner};

#[cfg(all(feature = "wasm-host", not(feature = "native-tokio")))]
pub use host_task::{RunHandle, RunTaskOwner};

#[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
pub use event_hub::EventSubscription;

#[cfg(feature = "native-tokio")]
pub use native::time::{DeadlineDiagnostic, MonotonicDeadline, RuntimeTimeError};

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

#[cfg(feature = "native-tokio")]
pub use id_generation::{OsRandomSource, SystemClock};
