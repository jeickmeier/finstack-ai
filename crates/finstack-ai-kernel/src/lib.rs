//! Deterministic, synchronous, I/O-free semantic kernel for `finstack-ai`.
//!
//! This crate owns semantic IDs, raw JSON/metadata, digests, time primitives,
//! stable error descriptors, content blocks, blob references, messages, run
//! lineage, effect/interaction envelopes, journal drafts, and runtime events. It
//! must not perform network, filesystem, database, clock, environment, process,
//! or host-language I/O.
//!
//! # Examples
//!
//! ```
//! use finstack_ai_kernel::{
//!     ContentBlock, Message, MessageId, MessageRole, Metadata, ProviderIds, RawJson, RunId,
//!     TextBlock, Timestamp,
//! };
//!
//! let run = RunId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("id");
//! assert_eq!(run.to_canonical_string(), "01234567-89ab-7cde-89ab-0123456789ab");
//!
//! let json = RawJson::parse(r#"{"b":1,"a":2}"#).expect("json");
//! assert_eq!(json.as_str(), r#"{"a":2,"b":1}"#);
//!
//! let message = Message::try_new(
//!     MessageId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("id"),
//!     MessageRole::User,
//!     vec![ContentBlock::Text(TextBlock::try_new("hello").expect("text"))],
//!     Timestamp::from_unix_ms(0).expect("epoch"),
//!     None,
//!     ProviderIds::empty(),
//!     Metadata::empty(),
//! )
//! .expect("message");
//! assert_eq!(message.role(), MessageRole::User);
//! ```
//!
//! # Model-only decide, commit, and apply
//!
//! ```
//! # use finstack_ai_kernel::*;
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! # let run_id = RunId::parse("01234567-89ab-7cde-89ab-0123456789ab")?;
//! # let accepted = RunAccepted::try_new(
//! #     run_id, RunRelation::root(run_id)?,
//! #     RunSecurityContext::try_new(
//! #         "tenant", PrincipalRef::try_new("issuer", "subject", Some("tenant"))?,
//! #         "oidc", "high", "policy-v1", "decision-v1", None,
//! #     )?,
//! #     None, RunLimits::empty(),
//! #     RunPropagationPolicy {
//! #         cancellation: CancellationPropagation::Cascade,
//! #         deadline: DeadlinePropagation::MinimumOfParentAndChild,
//! #         budget: BudgetPropagation::SharedScope,
//! #         principal: PrincipalPropagation::Inherit,
//! #     },
//! #     Digest::raw_json(br#"{"agent":"example"}"#), None,
//! # )?;
//! # let record_id = RecordId::parse("01234567-89ab-7cde-89ab-0123456789ac")?;
//! # let event_id = EventId::parse("01234567-89ab-7cde-89ab-0123456789ad")?;
//! # let now = Timestamp::from_unix_ms(1_000)?;
//! let mut kernel = Kernel::default();
//! let env = TransitionEnv {
//!     now,
//!     ids: AllocatedIds::try_new(
//!         vec![record_id], vec![event_id], vec![], vec![], vec![], vec![],
//!         vec![], vec![], vec![], vec![], vec![],
//!     )?,
//! };
//! let decision = kernel.decide(
//!     &env,
//!     KernelInput::AcceptRun(AcceptRun {
//!         session_id: SessionId::parse("01234567-89ab-7cde-89ab-0123456789ae")?,
//!         lane_id: LaneId::parse("01234567-89ab-7cde-89ab-0123456789af")?,
//!         accepted,
//!     }),
//! )?;
//!
//! // The store atomically assigns the sequence and returns a committed envelope.
//! let draft = &decision.records[0];
//! let envelope = RecordEnvelope::try_new(
//!     draft.format_version(), draft.kind_version(), draft.record_id(),
//!     draft.session_id(), draft.lane_id(), draft.run_id(),
//!     decision.expected_sequence, draft.timestamp(), Some(now),
//!     Digest::raw_json(b"payload"), None, Digest::raw_json(b"checksum"),
//!     draft.derived_event_ids().to_vec(), draft.body().clone(),
//! )?;
//! let batch = CommittedBatch::try_new(
//!     AppendBatchId::parse("01234567-89ab-7cde-89ab-0123456789b0")?,
//!     decision.expected_sequence,
//!     decision.expected_sequence,
//!     vec![envelope],
//! )?;
//! let events = kernel.apply(&batch, 0)?;
//! assert_eq!(events.len(), 1);
//! assert_eq!(kernel.state().phase, Some(RunPhase::BeforeRun));
//! # Ok(())
//! # }
//! ```

#![warn(missing_docs)]

mod agent;
mod bounds;
mod capabilities;
mod content;
mod digest;
mod effects;
mod entries;
mod error;
mod events;
mod external;
mod ids;
mod limits;
mod message;
mod projection;
mod raw_json;
mod records;
mod reducer;
mod refs;
mod run;
mod state;
mod time;
mod tools;
mod transcode;
mod validation;

pub use agent::{
    FinalResultRecorded, INTERNAL_TOOL_NAMESPACE, JsonSchemaDraft, LOAD_CAPABILITY_TOOL,
    OutputConfiguration, OutputEndStrategy, OutputSpec, SUBMIT_FINAL_OUTPUT_TOOL, SchemaRef,
    StructuredResultSource, is_internal_tool_name,
};
pub use bounds::{SEMANTIC_ARRAY_MAX_ITEMS, SEMANTIC_MAP_MAX_ENTRIES};
pub use capabilities::{ActiveCapability, CapabilitiesActivated, CapabilityActivationSource};
pub use content::{
    BlobRef, CONTENT_MAX_ITEMS, ContentBlock, ContentError, JsonBlock, LABEL_MAX_BYTES, MediaRef,
    OpaqueBlock, OpaquePayload, TEXT_MAX_BYTES, TextBlock, ToolCallBlock, ToolResultBlock,
};
pub use digest::{
    AGENT_SPEC_DIGEST_SCHEMA_VERSION, BLOB_CONTENT_DIGEST_SCHEMA_VERSION, DOMAIN_AGENT_SPEC,
    DOMAIN_BLOB_CONTENT, DOMAIN_EFFECT_INPUT, DOMAIN_EFFECT_OUTPUT, DOMAIN_MIDDLEWARE_CHAIN,
    DOMAIN_RAW_JSON, DOMAIN_RECORD_PAYLOAD, DOMAIN_SNAPSHOT_STATE, Digest, DigestError,
    EFFECT_INPUT_DIGEST_SCHEMA_VERSION, EFFECT_OUTPUT_DIGEST_SCHEMA_VERSION,
    MIDDLEWARE_CHAIN_DIGEST_SCHEMA_VERSION, RAW_JSON_DIGEST_SCHEMA_VERSION,
    RECORD_PAYLOAD_DIGEST_SCHEMA_VERSION, SNAPSHOT_STATE_DIGEST_SCHEMA_VERSION,
};
pub use effects::{
    ComponentInvocation, EffectCancelled, EffectCompleted, EffectDeferred, EffectError,
    EffectFailed, EffectInput, EffectKind, EffectOutputContract, EffectOutputKind, EffectPurpose,
    EffectRelation, EffectRequested, InteractionCancelled, InteractionExpired, InteractionKind,
    InteractionRequest, InteractionResolution, InvocationRecovery, PipelinePosition,
    ReconciliationPolicy, RetrySafety,
};
pub use entries::{
    ContextPrepared, ContextPreparedError, EntryAppended, EntryError, RetryClassification,
    RetryDirective, RetryScheduled, RunCancelled, RunCompleted, RunFailed, RunSuspended, Stage,
    StageCursor, StageDisposition, StageOutcomeRecorded, TimerFired,
};
pub use error::{
    ErrorCategory, ErrorCode, ErrorCodeError, ErrorDescriptor, ErrorDescriptorError,
    ErrorIdentifiers,
};
pub use events::{
    EventError, ModelTextDelta, ProviderHeartbeat, QueueDepthWarning, RUN_EVENT_KIND_VERSION,
    RUN_EVENT_SCHEMA_VERSION, ReasoningDelta, RunEvent, RunEventBody, RunEventClass, RunEventKind,
    ToolProgress, derived_event_kind,
};
pub use external::{
    ExternalCommandError, ExternalCommandKind, ExternalCommandRejected, ExternalCommandTarget,
    ExternalEffectCompletionCommand, InteractionResolutionCommand, OperationLocator,
    RecordExternalCommandRejected,
};
pub use ids::{
    AgentId, AgentTag, AppendBatchId, AppendBatchTag, ArtifactId, ArtifactTag, BudgetReservationId,
    BudgetReservationTag, BudgetScopeId, BudgetScopeTag, BundleId, BundleTag,
    CancellationRequestId, CancellationRequestTag, CapabilityId, CapabilityTag, ComponentId,
    ComponentTag, EffectId, EffectOutputKey, EffectOutputTag, EffectTag, EntryId, EntryTag,
    EventId, EventTag, Id, IdParseError, IdTag, InteractionId, InteractionTag, KEY_MAX_BYTES, Key,
    KeyParseError, KeyParseErrorKind, KeyTag, LaneId, LaneTag, LimitKey, LimitTag, MessageId,
    MessageTag, ModelRequestId, ModelRequestTag, RecordId, RecordTag, RunId, RunTag, SessionId,
    SessionTag, ToolBatchId, ToolBatchTag, ToolCallId, ToolCallTag, ToolId, ToolTag, TurnId,
    TurnTag,
};
pub use limits::{
    CostLimit, LimitDimension, LimitReached, LimitUsage, LimitValue, LimitsError, RunLimits,
    UnknownUsagePolicy,
};
pub use message::{
    MODEL_CONTEXT_LENGTH_MAX, Message, MessageError, MessageRole, ModelRef, ProviderIds,
    ThinkingLevel,
};
pub use raw_json::{
    METADATA_MAX_BYTES, METADATA_MAX_DEPTH, METADATA_MAX_KEY_BYTES, METADATA_MAX_MEMBERS, Metadata,
    RAW_JSON_MAX_BYTES, RAW_JSON_MAX_DEPTH, RawJson, RawJsonError,
};
pub use records::{
    APPEND_BATCH_MAX_RECORDS, AppendRequest, RECORD_FORMAT_VERSION, RECORD_KIND_VERSION,
    RecordBody, RecordDraft, RecordEnvelope, RecordError,
};
pub use reducer::{
    AcceptRun, CancelRequested, CancellationReconciledInput, CommittedBatch, Decision,
    ExternalEffectCompletedInput, ExternalEffectCompletion, ExternalEffectOutcome, Kernel,
    KernelError, KernelInput, ModelSettled, ModelSettlement, PostCommitAction, ReducerStageOutcome,
    StageSettled, TimerFiredInput, ToolBatchSettled, ToolSettlement,
};
pub use refs::{
    AllocatedIds, ArtifactRef, AssigneeHint, AuthorizationEvidence, ComponentRef, CostAmount,
    Diagnostic, DiagnosticSeverity, ExternalHandleRef, MiddlewareRef, PrincipalRef, RefsError,
    Sensitivity, Usage, Version,
};
pub use run::{
    BudgetPropagation, CancellationInitiator, CancellationPropagation, CancellationReconciled,
    CancellationRequest, CancellationRequested, DeadlinePropagation, MAX_RUN_RELATION_DEPTH,
    PrincipalPropagation, RunAccepted, RunError, RunPropagationPolicy, RunRelation,
    RunRelationKind, RunSecurityContext,
};
pub use state::{
    CancellationState, CompletionIdentity, CompletionIdentityHashEntryV1, CurrentTurn, KernelState,
    ModelSettlementFingerprint, ModelSettlementHashEntryV1, ModelSettlementKind,
    PendingModelEffect, RetryState, RunPhase, StageSettlementHashEntryV1, TerminalCandidate,
    TerminalState, ToolCallIdentityHashEntryV2, ToolSettlementHashEntryV2, TransitionEnv,
};
pub use time::{
    DURATION_JS_SAFE_MAX_MS, Duration, TIMESTAMP_MAX_MS, TIMESTAMP_MIN_MS, TimeError, Timestamp,
};
pub use tools::{
    ActiveToolBatch, ActiveToolCall, ActiveToolCallStatus, AssignedToolCall, SyntheticToolClosure,
    ToolBatchClosed, ToolBatchContinuation, ToolBatchOpened, ToolBatchOutcome, ToolCallIdentity,
    ToolCallPlan, ToolCallSettled, ToolExecutionMode, ToolFailurePolicy, ToolSettlementFingerprint,
    ToolSettlementKind, ValidatedToolCall,
};
pub use validation::{OutputValidated, OutputValidationFailed, ValidationIssue, ValidationOutcome};
