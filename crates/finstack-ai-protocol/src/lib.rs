//! Canonical-CBOR journal codec and shared remote/process framing.
//!
//! Production encode validates a value tree, recursively RFC 8949-sorts map
//! keys, then writes definite lengths and shortest lossless integer/float
//! forms. `ciborium` is pinned for value interop; its writer is not treated as
//! canonical by itself. Runtime and default SDK stay protocol-free: they do
//! not depend on this crate for in-process work.

#![warn(missing_docs)]

mod cbor;
mod error;
mod journal;
mod process;
mod remote;
mod snapshot;
mod wire;

pub use cbor::{
    APPEND_BATCH_MAX_BYTES, CANONICAL_ARRAY_MAX_ITEMS, CANONICAL_ENVELOPE_MAX_BYTES,
    CANONICAL_MAP_MAX_ENTRIES, CANONICAL_NESTING_DEPTH, CANONICAL_STRING_MAX_BYTES, CanonicalValue,
    decode, decode_value, encode, encode_value, from_diagnostic_json, to_ciborium,
    to_diagnostic_json, to_diagnostic_jsonl,
};
pub use error::ProtocolError;
pub use journal::{
    JournalKnownAnswer, batch_canonical_len, commit_record, commit_records, envelope_checksum,
    journal_known_answer, payload_digest, verify_chain, verify_chain_from, verify_envelope,
};
pub use process::ProcessPreAuth;
pub use remote::{
    DOMAIN_REMOTE_COMMAND, REMOTE_COMMAND_DIGEST_SCHEMA_VERSION, RemoteAuthMethod, RemoteCommand,
    RemoteCommandOp, RemoteCommandResult, RemoteEventView, RemoteLocator, RemotePostAuth,
    RemotePreAuth, RemoteSnapshot, command_digest, decode_remote_post_auth, decode_remote_pre_auth,
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
