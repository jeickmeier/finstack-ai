//! Runtime ports and effect execution for `finstack-ai`.
//!
//! Owns the six primary port contracts and effect execution. Target drivers
//! are selected by the non-default `native-tokio` and `wasm-host` features.
//!
//! PR-014 adds the journal-store boundary and the authoritative commit loop.
//! PR-015 adds the provider-neutral model port and deterministic stream driver.

#![warn(missing_docs)]

pub use finstack_ai_kernel::{
    BudgetScopeId, ContentBlock, Digest, EffectId, ErrorCategory, ExternalHandleRef, Message,
    Metadata, ModelRequestId, OperationLocator, OutputSpec, PendingModelEffect, PrincipalRef,
    ProviderIds, RawJson, ReconciliationPolicy, RetrySafety, Timestamp, ToolExecutionMode, ToolId,
    Usage,
};

mod audit;
mod coordinator;
mod error;
mod id_generation;
mod journal;
mod model;
mod ports;

#[cfg(feature = "native-tokio")]
mod task;

#[cfg(feature = "native-tokio")]
mod model_runtime;

#[cfg(feature = "native-tokio")]
mod ingress;

pub use audit::{
    SecurityAuditCategory, SecurityAuditError, SecurityAuditEvent, SecurityAuditHealth,
    SecurityAuditReceipt, SecurityAuditSink,
};

pub use coordinator::{CommitCoordinator, CommitCoordinatorError, CommitOutcome, RunFault};

pub use error::FrameworkError;
pub use id_generation::{Clock, IdGenerationError, RandomSource, UuidV7Generator};
pub use journal::{
    JournalStore, LoadRequest, LoadedSession, OpaqueSnapshot, SnapshotReceipt, SnapshotRequest,
    StoreCommitTimestamp, StoreError, StoreHealth,
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
pub use ports::{PortFuture, PortObject, PortStream};

#[cfg(feature = "native-tokio")]
pub use task::{
    ModelTaskConfig, RunHandle, RunHandleError, RunStatus, RunTaskConfig, RunTaskOwner,
};

#[cfg(feature = "native-tokio")]
pub use audit::{SecurityAuditGate, SecurityAuditGateError};

#[cfg(feature = "native-tokio")]
pub use ingress::{
    ExternalCompletionRouter, ExternalRouteError, ExternalRouteOutcome, InteractionRouter,
};

#[cfg(feature = "native-tokio")]
pub use id_generation::{OsRandomSource, SystemClock};
