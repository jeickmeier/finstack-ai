//! Record construction errors.

use thiserror::Error;

use crate::primitives::ErrorDescriptorError;
use crate::records::SessionRecordError;
use crate::run::RunError;

/// Record construction errors.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RecordError {
    /// Budget request or receipt is malformed.
    #[error("invalid budget record")]
    InvalidBudgetRecord,
    /// Unsupported envelope format version.
    #[error("unsupported record format_version {format_version}")]
    UnsupportedFormatVersion {
        /// Version.
        format_version: u16,
    },
    /// Unsupported body kind version.
    #[error("unsupported record kind_version {kind_version}")]
    UnsupportedKindVersion {
        /// Version.
        kind_version: u16,
    },
    /// Derived event id count mismatch.
    #[error("derived_event_ids len {actual} != expected {expected}")]
    DerivedEventCount {
        /// Expected.
        expected: usize,
        /// Actual.
        actual: usize,
    },
    /// Append batch exceeded record count.
    #[error("append batch has {len} records; max {max}")]
    BatchTooLarge {
        /// Length.
        len: usize,
        /// Max.
        max: usize,
    },
    /// A record did not belong to the append request's session.
    #[error("append request record session does not match request session")]
    RecordSessionMismatch,
    /// Interaction request/effect records were missing, duplicated, or mismatched.
    #[error("interaction request must pair with exactly one matching interaction effect")]
    InvalidInteractionPair,
    /// A decoded non-root run was not validated against its parent.
    #[error("non-root RunAccepted lineage must be validated before record construction")]
    UnvalidatedRunLineage,
    /// Record run id did not match the run-scoped body.
    #[error("record run_id does not match RunAccepted body")]
    RecordRunMismatch,
    /// Durable failure descriptor violates semantic limits.
    #[error(transparent)]
    InvalidErrorDescriptor(ErrorDescriptorError),
    /// Child run lineage or attenuation validation failed.
    #[error(transparent)]
    Run(#[from] RunError),
    /// Session, lane, or snapshot body is invalid.
    #[error(transparent)]
    Session(#[from] SessionRecordError),
}

impl RecordError {
    /// Stable error code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidBudgetRecord => "invalid_budget_record",
            Self::UnsupportedFormatVersion { .. } => "unsupported_format_version",
            Self::UnsupportedKindVersion { .. } => "unsupported_kind_version",
            Self::DerivedEventCount { .. } => "derived_event_count",
            Self::BatchTooLarge { .. } => "batch_too_large",
            Self::RecordSessionMismatch => "record_session_mismatch",
            Self::InvalidInteractionPair => "invalid_interaction_pair",
            Self::UnvalidatedRunLineage => "unvalidated_run_lineage",
            Self::RecordRunMismatch => "record_run_mismatch",
            Self::InvalidErrorDescriptor(_) => "invalid_error_descriptor",
            Self::Run(inner) => inner.code(),
            Self::Session(inner) => inner.code(),
        }
    }
}
