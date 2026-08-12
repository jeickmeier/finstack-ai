//! Runtime ports and effect execution for `finstack-ai`.
//!
//! Owns the six primary port contracts and effect execution. Target drivers
//! are selected by the non-default `native-tokio` and `wasm-host` features.
//!
//! PR-014 adds the journal-store boundary and the authoritative commit loop.
//! PR-015 adds the provider-neutral model port and deterministic stream driver.
//! PR-017 adds bounded native event delivery, batching, and backpressure.
//! PR-018 completes the six-port surface with context, middleware, and observer contracts.
//! PR-019 connects durable cancellation, deadlines, retry timers, and task shutdown.
//! PR-020 adds deterministic pre-dispatch manual drive and the native runtime gate proofs.

#![warn(missing_docs)]

pub use bytes::Bytes;
pub use finstack_ai_kernel::{
    AGENT_SPEC_DIGEST_SCHEMA_VERSION, AgentId, AppendBatchId, ArtifactId, ArtifactRef, BlobRef,
    BudgetChargeReceipt, BudgetChargeRecorded, BudgetChargeRequest, BudgetReleaseReceipt,
    BudgetReleaseRequest, BudgetRequest, BudgetReservationId, BudgetReservationReceipt,
    BudgetReservationReleased, BudgetReservationReplay, BudgetReservationRequested,
    BudgetReservationSettled, BudgetReserveRequest, BudgetScopeId, BundleId, CapabilityId,
    ChildPlacement, ChildRunLocator, ChildRunPrepared, ComponentId, ComponentInvocation,
    ComponentRef, ContentBlock, CostLimit, DOMAIN_AGENT_SPEC, Digest, EffectCompleted, EffectId,
    EffectInput, EffectKind, EffectOutputContract, EffectOutputKind, EffectPurpose, EffectRelation,
    EntryId, ErrorCategory, ErrorCode, ExternalEffectCompletionCommand, ExternalHandleRef,
    FinalResultRecorded, InteractionKind, InteractionRequest, InteractionResolutionCommand,
    InvocationRecovery, JsonBlock, JsonSchemaDraft, LaneId, LimitKey, Message, MessageId,
    MessageRole, Metadata, MiddlewareRef, ModelRequestId, OperationLocator, OutputEndStrategy,
    OutputSpec, PendingModelEffect, PipelinePosition, PrincipalRef, ProviderIds,
    RECORD_FORMAT_VERSION, RECORD_KIND_VERSION, RawJson, ReconciliationPolicy, RecordBody,
    RecordDraft, RecordEnvelope, RecordId, RemoteRouteRef, RetryDirective, RetrySafety, RunEvent,
    RunEventBody, RunEventClass, RunEventKind, RunId, RunLimits, SUBMIT_FINAL_OUTPUT_TOOL,
    SchemaRef, Sensitivity, SessionId, Stage, StructuredResultSource, TextBlock, Timestamp,
    ToolBatchId, ToolCallBlock, ToolCallId, ToolCallPlan, ToolExecutionMode, ToolFailurePolicy,
    ToolId, ToolProgress, ToolResultBlock, TurnId, Usage, ValidatedToolCall, ValidationIssue,
    ValidationOutcome, Version,
};
pub use finstack_ai_kernel::{
    ModelTextDelta as RunEventModelTextDelta, ProviderHeartbeat as RunEventProviderHeartbeat,
    QueueDepthWarning as RunEventQueueDepthWarning, ReasoningDelta as RunEventReasoningDelta,
};

mod agent_invoker;
mod artifact;
mod audit;
mod budget;
mod composition;
mod context;
mod coordinator;
mod error;
mod event_hub;
mod id_generation;
mod journal;
#[cfg(feature = "native-tokio")]
mod manual_drive;
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
mod time;

#[cfg(feature = "native-tokio")]
mod timer_runtime;

#[cfg(feature = "native-tokio")]
mod ingress;

pub use agent_invoker::{
    AGENT_INVOKE_CONFLICT, AGENT_INVOKE_INVALID_ACCEPTANCE, AGENT_INVOKE_UNAVAILABLE,
    AgentInvokeError, AgentInvoker, AgentRef, ChildRunContext, ChildRunHandle, ChildRunRequest,
};
pub use artifact::{
    ARTIFACT_INTEGRITY_FAILURE, ARTIFACT_INVALID_METADATA, ARTIFACT_NOT_FOUND,
    ARTIFACT_SCOPE_MISMATCH, ARTIFACT_TOO_LARGE, ARTIFACT_UNAVAILABLE, ArtifactError,
    ArtifactMetadata, ArtifactScope, ArtifactStore, MAX_ARTIFACT_BYTES, stage_required_artifact,
    validate_staged_artifact,
};
pub use audit::{
    SecurityAuditCategory, SecurityAuditError, SecurityAuditEvent, SecurityAuditHealth,
    SecurityAuditReceipt, SecurityAuditSink,
};
pub use budget::{
    BUDGET_CONFLICT, BUDGET_INVALID_RECEIPT, BUDGET_UNAVAILABLE, BUDGET_UNKNOWN, BudgetError,
    BudgetLedger, BudgetReservationState,
};
pub use composition::{
    BudgetCoordinator, BudgetOperationIds, ChildCoordinationIds, ChildRunCoordinator,
    CompositionError, child_relation_digest,
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
#[cfg(feature = "native-tokio")]
pub use manual_drive::{
    ManualDriveAction, ManualDriveController, ManualDriveEffect, ManualDriveError,
    ManualDrivePermit,
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
    ShutdownOutcome, ShutdownReport, TimerDiagnostics,
};

#[cfg(feature = "native-tokio")]
pub use event_hub::EventSubscription;

#[cfg(feature = "native-tokio")]
pub use tool_runtime::ToolTaskConfig;

#[cfg(feature = "native-tokio")]
pub use time::{
    DeadlineDiagnostic, MonotonicDeadline, NoRetryJitter, RetryJitterSource, RuntimeTimeError,
    retry_backoff_with_jitter,
};

#[cfg(feature = "native-tokio")]
pub use audit::{SecurityAuditGate, SecurityAuditGateError};

#[cfg(feature = "native-tokio")]
pub use ingress::{
    ExternalCompletionRouter, ExternalRouteError, ExternalRouteOutcome, InteractionRouter,
};

#[cfg(feature = "native-tokio")]
pub use id_generation::{OsRandomSource, SystemClock};

/// Native driver utilities used by the SDK facade without exposing Tokio
/// types in its public API.
#[cfg(feature = "native-tokio")]
pub mod native_driver {
    use std::future::Future;
    use std::sync::Arc;
    use std::time::Duration;

    use crate::PortFuture;

    /// No native runtime is active for a requested driver operation.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub struct DriverUnavailable;

    impl std::fmt::Display for DriverUnavailable {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("native driver is unavailable")
        }
    }

    impl std::error::Error for DriverUnavailable {}

    /// The native driver deadline elapsed before the future completed.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub struct TimeoutElapsed;

    impl std::fmt::Display for TimeoutElapsed {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("native driver deadline elapsed")
        }
    }

    impl std::error::Error for TimeoutElapsed {}

    /// Cloneable one-way notification used by native SDK state machines.
    #[derive(Clone, Default)]
    pub struct Signal {
        inner: Arc<tokio::sync::Notify>,
    }

    impl Signal {
        /// Create an empty notification signal.
        #[must_use]
        pub fn new() -> Self {
            Self::default()
        }

        /// Register a waiter before checking its guarded state.
        pub fn notified(&self) -> impl Future<Output = ()> + '_ {
            self.inner.notified()
        }

        /// Wake every waiter registered before this call.
        pub fn notify_waiters(&self) {
            self.inner.notify_waiters();
        }
    }

    /// Await a future until the native driver deadline elapses.
    ///
    /// # Errors
    ///
    /// Returns [`TimeoutElapsed`] when the future does not complete before the
    /// requested duration.
    pub async fn timeout<F>(duration: Duration, future: F) -> Result<F::Output, TimeoutElapsed>
    where
        F: Future,
    {
        tokio::time::timeout(duration, future)
            .await
            .map_err(|_| TimeoutElapsed)
    }

    /// Spawn one detached SDK driver future on the active native runtime.
    ///
    /// # Errors
    ///
    /// Returns [`DriverUnavailable`] when the caller is not inside the native
    /// runtime context.
    pub fn spawn(future: PortFuture<()>) -> Result<(), DriverUnavailable> {
        let handle = tokio::runtime::Handle::try_current().map_err(|_| DriverUnavailable)?;
        handle.spawn(future);
        Ok(())
    }

    /// Cooperatively yield one turn to the native driver.
    pub async fn yield_now() {
        tokio::task::yield_now().await;
    }
}
