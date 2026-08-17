//! Journal drafts, envelopes, owned record bodies, and durable payloads.
//!
//! This module is the payload catalog: envelope/draft/body types plus nested
//! `run`, `lifecycle`, `policy`, and `tools` records.

mod append;
mod body;
mod draft;
mod envelope;
mod error;
pub(crate) mod lifecycle;
pub(crate) mod policy;
pub(crate) mod run;
mod session;
pub(crate) mod tools;
mod validate;

#[cfg(test)]
mod tests;

/// V1 atomic append batch record-count ceiling (TDD §6.5).
pub const APPEND_BATCH_MAX_RECORDS: usize = 256;
/// Current record envelope format version.
pub const RECORD_FORMAT_VERSION: u16 = 1;
/// Current kind version for PR-008-owned bodies.
pub const RECORD_KIND_VERSION: u16 = 1;

pub use append::AppendRequest;
pub use body::RecordBody;
pub use draft::RecordDraft;
pub use envelope::RecordEnvelope;
pub use error::RecordError;
pub use session::{LaneCreated, LaneMoved, SessionCreated, SessionRecordError, SnapshotWritten};
