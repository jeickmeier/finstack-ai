//! Semantic IDs, time, raw JSON, digests, errors, bounds, and shared refs.
//!
//! Owns typed identifiers (`Id` and the `*Id` newtypes), `Timestamp` and
//! `Duration`, canonical `RawJson`/`Metadata`, domain-scoped `Digest` values,
//! stable `ErrorDescriptor` types, and shared reference values (`PrincipalRef`,
//! `Usage`, `AllocatedIds`, diagnostics). Collection ceilings are
//! [`SEMANTIC_ARRAY_MAX_ITEMS`] and [`SEMANTIC_MAP_MAX_ENTRIES`].

mod allocated_ids;
mod bounds;
mod diagnostic;
mod digest;
mod error;
mod handles;
mod identity;
mod ids;
pub(crate) mod label;
mod raw_json;
mod refs_error;
mod time;
mod transcode;
mod usage;

#[cfg(test)]
mod refs_tests;

pub use allocated_ids::AllocatedIds;
pub use bounds::BoundedMap;
pub(crate) use bounds::{BoundedString, BoundedVec};
pub use bounds::{SEMANTIC_ARRAY_MAX_ITEMS, SEMANTIC_MAP_MAX_ENTRIES};
pub use diagnostic::{Diagnostic, DiagnosticSeverity, Sensitivity};
pub use digest::{
    AGENT_SPEC_DIGEST_SCHEMA_VERSION, DOMAIN_AGENT_SPEC, DOMAIN_RECORD_ENVELOPE,
    DOMAIN_RECORD_PAYLOAD, Digest, DigestError, RECORD_ENVELOPE_DIGEST_SCHEMA_VERSION,
    RECORD_PAYLOAD_DIGEST_SCHEMA_VERSION,
};
pub(crate) use digest::{DigestWriter, HEX_DIGITS, str_from_ascii};
pub use error::{
    ErrorCategory, ErrorCode, ErrorCodeError, ErrorDescriptor, ErrorDescriptorError,
    ErrorIdentifiers,
};
pub use handles::{ArtifactRef, ExternalHandleRef};
pub use identity::{
    AssigneeHint, AuthorizationEvidence, ComponentRef, MiddlewareRef, PrincipalRef, Version,
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
pub use label::{hex_nibble, label_is_valid};
pub use raw_json::{
    METADATA_MAX_BYTES, METADATA_MAX_DEPTH, METADATA_MAX_KEY_BYTES, METADATA_MAX_MEMBERS, Metadata,
    RAW_JSON_MAX_BYTES, RAW_JSON_MAX_DEPTH, RawJson, RawJsonError,
};
pub use refs_error::RefsError;
pub(crate) use refs_error::{
    deserialize_micros, serialize_micros, validated_label, validated_text,
};
pub use time::{
    DURATION_JS_SAFE_MAX_MS, Duration, TIMESTAMP_MAX_MS, TIMESTAMP_MIN_MS, TimeError, Timestamp,
    UNIX_EPOCH,
};
pub(crate) use transcode::CanonicalJson;
pub use usage::{CostAmount, Usage};
