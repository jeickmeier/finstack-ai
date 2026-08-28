//! Canonical-CBOR journal codec and shared remote/process framing.
//!
//! Production encode validates a value tree, recursively RFC 8949-sorts map
//! keys, then writes definite lengths and shortest lossless integer/float
//! forms. The project-owned writer is canonical (ADR-015 / ADR-038).
//! `ciborium` is test-only interop and is not the profile writer. Runtime
//! and default SDK stay protocol-free: they do not depend on this crate for
//! in-process work.

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

mod cbor;
mod error;
mod journal;
mod prebeta;
mod process;
mod remote;
mod snapshot;
mod wire;

pub use cbor::{
    APPEND_BATCH_MAX_BYTES, CANONICAL_ARRAY_MAX_ITEMS, CANONICAL_ENVELOPE_MAX_BYTES,
    CANONICAL_MAP_MAX_ENTRIES, CANONICAL_NESTING_DEPTH, CANONICAL_STRING_MAX_BYTES, CanonicalValue,
    decode, decode_value, encode, encode_value, from_diagnostic_json, to_diagnostic_json,
    to_diagnostic_jsonl,
};
pub use error::ProtocolError;
pub use journal::{
    ChainAnchor, JournalKnownAnswer, commit_record, commit_records, envelope_checksum,
    journal_known_answer, payload_digest, verify_chain, verify_chain_from, verify_envelope,
};
pub use prebeta::{
    CHILD_RUN_PREPARED, EXTERNAL_EFFECT_COMPLETION, INTERACTION_RESOLUTION, PrebetaError,
    normalize_prebeta_shape,
};
pub use process::ProcessPreAuth;
pub use remote::{
    RemoteAgentRef, RemoteAuthMethod, RemoteCommand, RemoteCommandId, RemoteCommandPayload,
    RemoteCommandResult, RemoteDurableStep, RemoteEventView, RemoteLocator, RemotePostAuth,
    RemotePreAuth, RemoteSnapshot, RemoteStartRequest,
};
pub use snapshot::{
    DecodedSnapshot, SNAPSHOT_ENVELOPE_FORMAT_VERSION, decode_opaque_snapshot, decode_snapshot,
    encode_snapshot,
};
pub use wire::{
    FRAME_LENGTH_BYTES, POST_AUTH_FRAME_MAX_BYTES, PRE_AUTH_FRAME_MAX_BYTES, PROTOCOL_VERSION_V1,
    PayloadFamily, ProtocolEnvelope, VersionOffer, decode_envelope, decode_frame, decode_frame_len,
    encode_envelope, encode_frame, require_features, select_version,
};
