//! Deterministic, synchronous, I/O-free semantic kernel for `finstack-ai`.
//!
//! This crate owns semantic IDs, raw JSON/metadata, digests, time primitives,
//! stable error descriptors, content blocks, blob references, messages, run
//! lineage, effect/interaction envelopes, journal drafts, and runtime events. It
//! must not perform network, filesystem, database, clock, environment, process,
//! or host-language I/O.
//!
//! # Module map
//!
//! - `primitives` — IDs, time, JSON, digest, errors, bounds, and shared refs
//! - `content` — content blocks and blob references
//! - `conversation` — messages and conversation tree
//! - `records` — journal envelopes and durable payloads (`run`, `lifecycle`,
//!   `policy`, `tools`)
//! - `effects` — effect and interaction envelopes
//! - `events` — runtime events
//! - `state` — kernel state and projections
//! - `reducer` — decide and apply
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
#![forbid(unsafe_code)]
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

mod content;
mod conversation;
mod effects;
mod events;
mod primitives;
mod records;
mod reducer;
mod state;

pub use content::{
    BlobRef, CONTENT_MAX_ITEMS, ContentBlock, ContentError, JsonBlock, LABEL_MAX_BYTES, MediaRef,
    OpaqueBlock, OpaquePayload, TEXT_MAX_BYTES, TextBlock, ToolCallBlock, ToolResultBlock,
};
pub use conversation::{
    ConversationEntry, ConversationError, EntryBody, LaneProjection, MODEL_CONTEXT_LENGTH_MAX,
    Message, MessageError, MessageRole, ModelRef, OperationSummary, ProviderIds, SessionProjection,
    ThinkingLevel,
};
pub use effects::{
    ComponentInvocation, EffectCancelled, EffectCompleted, EffectDeferred, EffectError,
    EffectFailed, EffectInput, EffectKind, EffectOutputContract, EffectOutputKind, EffectPurpose,
    EffectRelation, EffectRequested, InteractionCancelled, InteractionExpired, InteractionKind,
    InteractionRequest, InteractionResolution, InvocationRecovery, NestedModelKind,
    PipelinePosition, ReconciliationPolicy, RetrySafety,
};
pub use events::{
    EventError, ModelTextDelta, ProviderHeartbeat, QueueDepthWarning, RUN_EVENT_KIND_VERSION,
    RUN_EVENT_SCHEMA_VERSION, ReasoningDelta, RunEvent, RunEventBody, RunEventClass, RunEventKind,
    ToolProgress,
};
pub use primitives::{
    AGENT_SPEC_DIGEST_SCHEMA_VERSION, AgentId, AgentTag, AppendBatchId, AppendBatchTag, ArtifactId,
    ArtifactTag, BudgetReservationId, BudgetReservationTag, BudgetScopeId, BudgetScopeTag,
    BundleId, BundleTag, CancellationRequestId, CancellationRequestTag, CapabilityId,
    CapabilityTag, ComponentId, ComponentTag, DOMAIN_AGENT_SPEC, DOMAIN_RECORD_ENVELOPE,
    DOMAIN_RECORD_PAYLOAD, DURATION_JS_SAFE_MAX_MS, Digest, DigestError, Duration, EffectId,
    EffectOutputKey, EffectOutputTag, EffectTag, EntryId, EntryTag, ErrorCategory, ErrorCode,
    ErrorCodeError, ErrorDescriptor, ErrorDescriptorError, ErrorIdentifiers, EventId, EventTag, Id,
    IdParseError, IdTag, InteractionId, InteractionTag, KEY_MAX_BYTES, Key, KeyParseError,
    KeyParseErrorKind, KeyTag, LaneId, LaneTag, LimitKey, LimitTag, METADATA_MAX_BYTES,
    METADATA_MAX_DEPTH, METADATA_MAX_KEY_BYTES, METADATA_MAX_MEMBERS, MessageId, MessageTag,
    Metadata, ModelRequestId, ModelRequestTag, RAW_JSON_MAX_BYTES, RAW_JSON_MAX_DEPTH,
    RECORD_ENVELOPE_DIGEST_SCHEMA_VERSION, RECORD_PAYLOAD_DIGEST_SCHEMA_VERSION, RawJson,
    RawJsonError, RecordId, RecordTag, RunId, RunTag, SEMANTIC_ARRAY_MAX_ITEMS,
    SEMANTIC_MAP_MAX_ENTRIES, SessionId, SessionTag, StaticDigestDomain, StaticErrorCode,
    StaticKey, TIMESTAMP_MAX_MS, TIMESTAMP_MIN_MS, TimeError, Timestamp, ToolBatchId, ToolBatchTag,
    ToolCallId, ToolCallTag, ToolId, ToolTag, TurnId, TurnTag, UNIX_EPOCH,
};
pub use primitives::{
    AllocatedIds, ArtifactRef, AssigneeHint, AuthorizationEvidence, BoundedMap, ComponentRef,
    CostAmount, Diagnostic, DiagnosticSeverity, ExternalHandleRef, MiddlewareRef, PrincipalRef,
    RefsError, Sensitivity, Usage, Version, hex_nibble, label_is_valid,
};
pub use records::lifecycle::{
    ContextPrepared, ContextPreparedError, EntryAppended, EntryError, RetryClassification,
    RetryDirective, RetryScheduled, RunCancelled, RunCompleted, RunFailed, RunSuspended, Stage,
    StageCursor, StageDisposition, StageOutcomeRecorded, TimerFired,
};
pub use records::policy::{
    ActiveCapability, BudgetChargeReceipt, BudgetChargeRecorded, BudgetChargeRequest,
    BudgetRecordError, BudgetReleaseReceipt, BudgetReleaseRequest, BudgetRequest,
    BudgetReservationReceipt, BudgetReservationReleased, BudgetReservationRequested,
    BudgetReservationSettled, BudgetReserveRequest, CapabilitiesActivated,
    CapabilityActivationSource, CostLimit, FinalResultRecorded, INTERNAL_TOOL_NAMESPACE,
    JsonSchemaDraft, LOAD_CAPABILITY_TOOL, LimitDimension, LimitReached, LimitUsage, LimitValue,
    LimitsError, OutputConfiguration, OutputEndStrategy, OutputSpec, OutputValidated,
    OutputValidationFailed, RunLimits, SUBMIT_FINAL_OUTPUT_TOOL, SchemaRef, StructuredResultSource,
    UnknownUsagePolicy, ValidationIssue, ValidationOutcome, is_internal_tool_name,
};
pub use records::run::{
    BudgetPropagation, CancellationInitiator, CancellationPropagation, CancellationReconciled,
    CancellationRequest, CancellationRequested, ChildPlacement, ChildRunLocator, ChildRunPrepared,
    CompactionAuthorization, DeadlinePropagation, ExternalCommandError, ExternalCommandKind,
    ExternalCommandRejected, ExternalCommandTarget, ExternalEffectCompletionCommand,
    InteractionResolutionCommand, MAX_RUN_RELATION_DEPTH, OperationLocator, PrincipalPropagation,
    RecordExternalCommandRejected, RemoteRouteRef, RunAccepted, RunError, RunPropagationPolicy,
    RunRelation, RunRelationKind, RunSecurityContext,
};
pub use records::tools::{
    ActiveToolBatch, ActiveToolCall, ActiveToolCallStatus, AssignedToolCall, SyntheticToolClosure,
    ToolBatchClosed, ToolBatchContinuation, ToolBatchOpened, ToolBatchOutcome, ToolCallIdentity,
    ToolCallPlan, ToolCallSettled, ToolExecutionMode, ToolFailurePolicy, ToolSettlementFingerprint,
    ToolSettlementKind, ValidatedToolCall,
};
pub use records::{
    APPEND_BATCH_MAX_RECORDS, AppendRequest, LaneCreated, LaneMoved, RECORD_FORMAT_VERSION,
    RECORD_KIND_VERSION, RecordBody, RecordDraft, RecordEnvelope, RecordError, SessionCreated,
    SessionRecordError, SnapshotWritten,
};
pub use reducer::{
    AcceptRun, CancelRequested, CancellationReconciledInput, CommittedBatch, Decision,
    ExtensionEffectSettled, ExtensionSettlement, ExternalEffectCompletedInput,
    ExternalEffectCompletion, ExternalEffectOutcome, InteractionSettled, Kernel, KernelError,
    KernelInput, ModelSettled, ModelSettlement, PostCommitAction, ReducerStageOutcome,
    RequestCompactionModel, RequestExtensionEffect, RequestInteraction, StageSettled,
    TimerFiredInput, ToolBatchSettled, ToolSettlement,
};
pub use state::{
    BudgetReservationReplay, CancellationState, CompletionIdentity, CurrentTurn,
    ExtensionSettlementFingerprint, ExtensionSettlementKind, InteractionTerminal,
    InteractionTerminalOutcome, KernelState, ModelSettlementFingerprint, ModelSettlementKind,
    PendingExtensionEffect, PendingInteraction, PendingModelEffect, ResolutionIdentity, RetryState,
    RunPhase, TerminalCandidate, TerminalState, TransitionEnv,
};
