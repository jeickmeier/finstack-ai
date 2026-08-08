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

#![warn(missing_docs)]

mod content;
mod digest;
mod effects;
mod error;
mod events;
mod ids;
mod limits;
mod message;
mod raw_json;
mod records;
mod refs;
mod run;
mod time;

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
pub use error::{ErrorCategory, ErrorCode, ErrorCodeError, ErrorDescriptor, ErrorIdentifiers};
pub use events::{
    EventError, ModelTextDelta, ProviderHeartbeat, QueueDepthWarning, RUN_EVENT_KIND_VERSION,
    RUN_EVENT_SCHEMA_VERSION, ReasoningDelta, RunEvent, RunEventBody, RunEventClass, RunEventKind,
    ToolProgress, derived_event_kind,
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
pub use limits::{CostLimit, LimitDimension, LimitsError, RunLimits, UnknownUsagePolicy};
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
pub use refs::{
    AllocatedIds, ArtifactRef, AssigneeHint, AuthorizationEvidence, ComponentRef, CostAmount,
    Diagnostic, DiagnosticSeverity, ExternalHandleRef, MiddlewareRef, PrincipalRef, RefsError,
    Sensitivity, Usage, Version,
};
pub use run::{
    BudgetPropagation, CancellationPropagation, DeadlinePropagation, MAX_RUN_RELATION_DEPTH,
    PrincipalPropagation, RunAccepted, RunError, RunPropagationPolicy, RunRelation,
    RunRelationKind, RunSecurityContext,
};
pub use time::{
    DURATION_JS_SAFE_MAX_MS, Duration, TIMESTAMP_MAX_MS, TIMESTAMP_MIN_MS, TimeError, Timestamp,
};
