//! Semantic IDs, time, raw JSON, digests, errors, and bounded collections.
//!
//! Owns typed identifiers (`Id` and the `*Id` newtypes), `Timestamp` and
//! `Duration`, canonical `RawJson`/`Metadata`, domain-scoped `Digest` values,
//! and stable `ErrorDescriptor` types. Collection ceilings are
//! [`SEMANTIC_ARRAY_MAX_ITEMS`] and [`SEMANTIC_MAP_MAX_ENTRIES`].

mod bounds;
mod digest;
mod error;
mod ids;
mod raw_json;
mod time;
mod transcode;

pub(crate) use bounds::{BoundedMap, BoundedString, BoundedVec};
pub use bounds::{SEMANTIC_ARRAY_MAX_ITEMS, SEMANTIC_MAP_MAX_ENTRIES};
pub use digest::{
    AGENT_SPEC_DIGEST_SCHEMA_VERSION, BLOB_CONTENT_DIGEST_SCHEMA_VERSION, DOMAIN_AGENT_SPEC,
    DOMAIN_BLOB_CONTENT, DOMAIN_EFFECT_INPUT, DOMAIN_EFFECT_OUTPUT, DOMAIN_MIDDLEWARE_CHAIN,
    DOMAIN_RAW_JSON, DOMAIN_RECORD_ENVELOPE, DOMAIN_RECORD_PAYLOAD, DOMAIN_SNAPSHOT_STATE, Digest,
    DigestError, EFFECT_INPUT_DIGEST_SCHEMA_VERSION, EFFECT_OUTPUT_DIGEST_SCHEMA_VERSION,
    MIDDLEWARE_CHAIN_DIGEST_SCHEMA_VERSION, RAW_JSON_DIGEST_SCHEMA_VERSION,
    RECORD_ENVELOPE_DIGEST_SCHEMA_VERSION, RECORD_PAYLOAD_DIGEST_SCHEMA_VERSION,
    SNAPSHOT_STATE_DIGEST_SCHEMA_VERSION,
};
pub(crate) use digest::{DigestWriter, HEX_DIGITS};
pub use error::{
    ErrorCategory, ErrorCode, ErrorCodeError, ErrorDescriptor, ErrorDescriptorError,
    ErrorIdentifiers,
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
pub use raw_json::{
    METADATA_MAX_BYTES, METADATA_MAX_DEPTH, METADATA_MAX_KEY_BYTES, METADATA_MAX_MEMBERS, Metadata,
    RAW_JSON_MAX_BYTES, RAW_JSON_MAX_DEPTH, RawJson, RawJsonError,
};
pub use time::{
    DURATION_JS_SAFE_MAX_MS, Duration, TIMESTAMP_MAX_MS, TIMESTAMP_MIN_MS, TimeError, Timestamp,
};
pub(crate) use transcode::CanonicalJson;
