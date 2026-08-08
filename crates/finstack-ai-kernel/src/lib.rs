//! Deterministic, synchronous, I/O-free semantic kernel for `finstack-ai`.
//!
//! This crate owns semantic IDs, raw JSON/metadata, digests, time primitives, and
//! stable error descriptors. It must not perform network, filesystem, database,
//! clock, environment, process, or host-language I/O.
//!
//! # Examples
//!
//! ```
//! use finstack_ai_kernel::{RawJson, RunId, Timestamp};
//!
//! let run = RunId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("id");
//! assert_eq!(run.to_canonical_string(), "01234567-89ab-7cde-89ab-0123456789ab");
//!
//! let json = RawJson::parse(r#"{"b":1,"a":2}"#).expect("json");
//! assert_eq!(json.as_str(), r#"{"a":2,"b":1}"#);
//!
//! let ts = Timestamp::from_unix_ms(0).expect("epoch");
//! assert_eq!(ts.to_rfc3339(), "1970-01-01T00:00:00.000Z");
//! ```

#![warn(missing_docs)]

mod digest;
mod error;
mod ids;
mod raw_json;
mod time;

pub use digest::{DOMAIN_RAW_JSON, Digest, DigestError, RAW_JSON_DIGEST_SCHEMA_VERSION};
pub use error::{ErrorCategory, ErrorCode, ErrorDescriptor, ErrorIdentifiers};
pub use ids::{
    AgentId, AgentTag, AppendBatchId, AppendBatchTag, ArtifactId, ArtifactTag, BudgetReservationId,
    BudgetReservationTag, BudgetScopeId, BudgetScopeTag, BundleId, BundleTag,
    CancellationRequestId, CancellationRequestTag, CapabilityId, CapabilityTag, ComponentId,
    ComponentTag, EffectId, EffectTag, EntryId, EntryTag, EventId, EventTag, Id, IdParseError,
    IdTag, InteractionId, InteractionTag, KEY_MAX_BYTES, Key, KeyParseError, KeyParseErrorKind,
    KeyTag, LaneId, LaneTag, MessageId, MessageTag, ModelRequestId, ModelRequestTag, RecordId,
    RecordTag, RunId, RunTag, SessionId, SessionTag, ToolBatchId, ToolBatchTag, ToolCallId,
    ToolCallTag, ToolId, ToolTag, TurnId, TurnTag,
};
pub use raw_json::{
    METADATA_MAX_BYTES, METADATA_MAX_DEPTH, METADATA_MAX_KEY_BYTES, METADATA_MAX_MEMBERS, Metadata,
    RAW_JSON_MAX_BYTES, RAW_JSON_MAX_DEPTH, RawJson, RawJsonError,
};
pub use time::{
    DURATION_JS_SAFE_MAX_MS, Duration, TIMESTAMP_MAX_MS, TIMESTAMP_MIN_MS, TimeError, Timestamp,
};
