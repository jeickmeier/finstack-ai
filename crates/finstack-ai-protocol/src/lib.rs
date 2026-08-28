//! Canonical journal codec and bounded remote/process framing for `finstack-ai`.
//!
//! This crate owns wire representation only. The runtime and default SDK do
//! not depend on it for in-process values or semantic decisions.
//!
//! # Canonical journal data
//!
//! [`encode`] validates a value tree, recursively applies RFC 8949 map-key
//! ordering, and writes definite lengths plus shortest lossless integer and
//! float forms. [`decode`] rejects non-canonical or over-limit input before
//! allocating declared collections. [`payload_digest`], [`envelope_checksum`],
//! and [`verify_chain`] bind journal records to the same canonical bytes used
//! by persistence and cross-language known-answer fixtures.
//!
//! # Remote framing
//!
//! [`ProtocolEnvelope`] and [`RemotePostAuth`] define the versioned session
//! framing contract. Frame, collection, string, and nesting limits are public
//! constants and are enforced during decoding. Version and feature negotiation
//! fail closed through [`select_version`] and [`require_features`].
//!
//! # Pre-beta shapes
//!
//! [`normalize_prebeta_shape`] is the shared Rust validator used by Python and
//! WASM for child-lineage, interaction-resolution, and authenticated external
//! completion DTOs. It validates data only and does not route a live command.
//!
//! # Diagnostics
//!
//! [`to_diagnostic_json`] and [`to_diagnostic_jsonl`] are explicit diagnostic
//! projections. They are not alternate canonical encodings.

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
