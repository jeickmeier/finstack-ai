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
pub mod ports;

/// `bytes::Bytes` appears in the port signatures this crate defines
/// (`ArtifactStore::stage_put`, the model and tool streams), so it is
/// re-exported to guarantee every implementor agrees on the same version.
pub use bytes::Bytes;
mod services;
#[cfg(feature = "native-tokio")]
#[doc(hidden)]
pub mod testing;

#[cfg(feature = "native-tokio")]
pub(crate) use driver::native;
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
pub(crate) use services::{id_generation, interaction};

#[cfg(feature = "native-tokio")]
pub use driver::sdk as native_driver;
#[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
pub(crate) use exec::middleware_driver;

pub use error::PortErrorInvalid;
#[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
pub(crate) use model::MODEL_PROFILE_INVALID;
#[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
pub(crate) use model::{parse_committed_model_request, stable_model_dispatch_code};
#[cfg(feature = "native-tokio")]
#[cfg(feature = "native-tokio")]
pub(crate) use tool::{TOOL_PANICKED, TOOL_STREAM_INVALID};

// ---- audience-scoped modules (canonical paths) ----

/// Run lifecycle: handles, task configuration, retry and shutdown policy, live state and deadlines.
pub mod run {
    #[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
    pub use crate::exec::live_state::LiveRunState;
    #[cfg(all(feature = "wasm-host", not(feature = "native-tokio")))]
    pub use crate::host_task::{RunHandle, RunTaskOwner};
    #[cfg(feature = "native-tokio")]
    pub use crate::native::time::{DeadlineDiagnostic, MonotonicDeadline, RuntimeTimeError};
    #[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
    pub use crate::run_types::{
        ModelTaskConfig, RetryBackoffPolicy, RunHandleError, RunStatus, RunTaskConfig,
        SameIdentityRetryPolicy, ShutdownOutcome, ShutdownReport, TimerDiagnostics, ToolTaskConfig,
    };
    #[cfg(feature = "native-tokio")]
    pub use crate::task::{RunHandle, RunTaskOwner};
}

/// Child-run composition: invoking a child agent, starting it, and coordinating budget and lineage.
pub mod child {
    pub use crate::services::agent_invoker::{
        AGENT_INVOKE_INVALID_ACCEPTANCE, AgentInvokeError, AgentInvoker, AgentRef, ChildRunContext,
        ChildRunHandle, ChildRunPolicy, ChildRunRequest, ChildRunStatus,
    };
    #[cfg(feature = "native-tokio")]
    pub use crate::services::child_starter::{ChildRunStartRequest, ChildRunStarter};
    pub use crate::services::composition::{
        BudgetCoordinator, BudgetOperationIds, ChildCoordinationIds, ChildRunCoordinator,
        CompositionError, child_relation_digest,
    };
}

/// Scoped artifact storage, the one storage concept a host wires.
pub mod artifact {
    pub use crate::services::artifact::{
        ARTIFACT_CAPACITY_EXCEEDED, ARTIFACT_INTEGRITY_FAILURE, ARTIFACT_SCOPE_MISMATCH,
        ArtifactError, ArtifactGcReport, ArtifactMetadata, ArtifactOwnerId, ArtifactPersistence,
        ArtifactRead, ArtifactScope, ArtifactStore, ArtifactStoreDescriptor, ArtifactStoreLimits,
        DEFAULT_ARTIFACT_ORPHAN_GRACE_MS, MAX_ARTIFACT_BYTES, MAX_ARTIFACT_GC_BATCH,
        MAX_ARTIFACT_OWNERS, MAX_ARTIFACTS, MAX_TOTAL_ARTIFACT_BYTES, artifact_storage_key,
        build_artifact_ref, get_required_artifact, stage_required_artifact,
        validate_artifact_scope, validate_retrieved_artifact, validate_staged_artifact,
    };
}

/// The run-event hub: subscriptions, batching, filters and lag policy.
pub mod events {
    #[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
    pub use crate::event_hub::EventSubscription;
    pub use crate::event_hub::{
        EventBatch, EventBatchConfig, EventDeliveryStats, EventFilter, EventHubConfig,
        EventLagPolicy, EventSubscriptionCloseReason, EventSubscriptionConfig,
        EventSubscriptionError, EventSubscriptionStatus, ProgressCoalescing,
    };
}

/// Session and lane management, plus external-identity binding.
pub mod session {
    pub use crate::services::identity_map::{
        ExternalIdentityKey, ExternalIdentityMap, IdentityMapError, MemoryExternalIdentityMap,
    };
    pub use crate::services::session::{
        LaneAppendIds, LaneCreateIds, LaneInspect, LaneRunContext, SessionCreateIds, SessionError,
        SessionHeadUpdate, SessionRuntime,
    };
}

/// Security-audit sink and the gate that fails closed without it.
pub mod audit {
    #[cfg(feature = "native-tokio")]
    pub use crate::services::audit::SecurityAuditGateHealth;
    pub use crate::services::audit::{
        SecurityAuditCategory, SecurityAuditError, SecurityAuditEvent, SecurityAuditHealth,
        SecurityAuditReceipt, SecurityAuditSink,
    };
    #[cfg(feature = "native-tokio")]
    pub use crate::services::audit::{SecurityAuditGate, SecurityAuditGateError};
}

/// Process confinement profiles and backends for spawned children.
pub mod confinement {
    #[cfg(not(target_arch = "wasm32"))]
    pub use crate::services::process_confinement::{
        CONFINEMENT_UNAVAILABLE, ConfinedChild, ConfinementBackend, ConfinementError,
        ConfinementProfile, ProcessConfinement, WindowsLpacProfile,
    };
    #[cfg(not(target_arch = "wasm32"))]
    pub use crate::services::process_confinement::{
        configure_process_tree, terminate_process_tree,
    };
}

/// Shared-budget reservation and settlement.
pub mod budget {
    pub use crate::services::budget::{BudgetError, BudgetLedger, BudgetReservationState};
}

/// Durable ingress: external completion and interaction routing.
pub mod ingress {
    #[cfg(feature = "native-tokio")]
    pub use crate::driver::ingress::{
        ExternalCompletionRouter, ExternalRouteError, ExternalRouteOutcome, InteractionRouter,
    };
    pub use crate::interaction::{InteractionResumeAction, interaction_resume_action};
}

/// Workflow sessions, checkpoints and retry decisions.
pub mod workflow {
    #[cfg(feature = "native-tokio")]
    pub use crate::driver::workflow::{
        WorkflowCheckpoint, WorkflowDriverError, WorkflowRetryDecision, WorkflowSession,
        WorkflowWait, classify_wait, resolve_checkpoint_sequence, retry_decision,
    };
}

/// Deterministic id generation: clocks, random sources, UUIDv7.
pub mod ids {
    pub use crate::id_generation::{
        Clock, ExternalClock, IdGenerationError, RandomSource, UuidV7Generator,
    };
    #[cfg(feature = "native-tokio")]
    pub use crate::id_generation::{OsRandomSource, SystemClock};
}

/// The commit coordinator: commit-before-effect ordering and outcomes.
pub mod commit {
    pub use crate::coordinator::{
        CommitCoordinator, CommitCoordinatorError, CommitOutcome, RunFault,
    };
}
