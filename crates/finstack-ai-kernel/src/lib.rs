//! Deterministic, synchronous, I/O-free semantic kernel for `finstack-ai`.
//!
//! This crate owns semantic IDs, raw JSON/metadata, digests, time primitives,
//! stable error descriptors, content blocks, blob references, and messages. It
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
mod error;
mod ids;
mod message;
mod raw_json;
mod time;

pub use content::{
    BlobRef, CONTENT_MAX_ITEMS, ContentBlock, ContentError, JsonBlock, LABEL_MAX_BYTES, MediaRef,
    OpaqueBlock, OpaquePayload, TEXT_MAX_BYTES, TextBlock, ToolCallBlock, ToolResultBlock,
};
pub use digest::{
    BLOB_CONTENT_DIGEST_SCHEMA_VERSION, DOMAIN_BLOB_CONTENT, DOMAIN_RAW_JSON, Digest, DigestError,
    RAW_JSON_DIGEST_SCHEMA_VERSION,
};
pub use error::{ErrorCategory, ErrorCode, ErrorCodeError, ErrorDescriptor, ErrorIdentifiers};
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
pub use message::{
    MODEL_CONTEXT_LENGTH_MAX, Message, MessageError, MessageRole, ModelRef, ProviderIds,
    ThinkingLevel,
};
pub use raw_json::{
    METADATA_MAX_BYTES, METADATA_MAX_DEPTH, METADATA_MAX_KEY_BYTES, METADATA_MAX_MEMBERS, Metadata,
    RAW_JSON_MAX_BYTES, RAW_JSON_MAX_DEPTH, RawJson, RawJsonError,
};
pub use time::{
    DURATION_JS_SAFE_MAX_MS, Duration, TIMESTAMP_MAX_MS, TIMESTAMP_MIN_MS, TimeError, Timestamp,
};
