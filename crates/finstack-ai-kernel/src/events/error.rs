//! Event construction errors.

use thiserror::Error;

use crate::refs::RefsError;

use super::RunEventClass;

/// Event construction errors.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum EventError {
    /// Durable/transient class mismatch.
    #[error("run event class mismatch: expected {expected:?}, got {actual:?}")]
    ClassMismatch {
        /// Expected class.
        expected: RunEventClass,
        /// Actual class.
        actual: RunEventClass,
    },
    /// Invalid label.
    #[error("invalid {field} label")]
    InvalidLabel {
        /// Field.
        field: &'static str,
    },
    /// Percent out of range.
    #[error("tool progress percent must be <= 100")]
    InvalidPercent,
    /// Unsupported event schema version.
    #[error("unsupported event schema_version {schema_version}")]
    UnsupportedSchemaVersion {
        /// Version.
        schema_version: u16,
    },
    /// Unsupported kind version.
    #[error("unsupported event kind_version {kind_version}")]
    UnsupportedKindVersion {
        /// Version.
        kind_version: u16,
    },
    /// Unsupported ordinal.
    #[error("unsupported derived-event ordinal {ordinal}")]
    UnsupportedOrdinal {
        /// Ordinal.
        ordinal: usize,
    },
    /// Event envelope correlation did not match its body/source record.
    #[error("run event correlation mismatch: {reason}")]
    CorrelationMismatch {
        /// Reason.
        reason: &'static str,
    },
    /// Refs error.
    #[error(transparent)]
    Refs(#[from] RefsError),
}

impl EventError {
    /// Stable error code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::ClassMismatch { .. } => "event_class_mismatch",
            Self::InvalidLabel { .. } => "invalid_label",
            Self::InvalidPercent => "invalid_percent",
            Self::UnsupportedSchemaVersion { .. } => "unsupported_schema_version",
            Self::UnsupportedKindVersion { .. } => "unsupported_kind_version",
            Self::UnsupportedOrdinal { .. } => "unsupported_ordinal",
            Self::CorrelationMismatch { .. } => "event_correlation_mismatch",
            Self::Refs(inner) => inner.code(),
        }
    }
}
