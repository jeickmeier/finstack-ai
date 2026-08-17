//! Journal drafts, envelopes, and owned record bodies (TDD §12.1–§12.2).

mod append;
mod body;
mod draft;
mod envelope;
mod error;
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
