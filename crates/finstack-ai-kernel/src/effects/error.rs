//! Effect and interaction errors.

use thiserror::Error;

use crate::content::ContentError;
use crate::primitives::ErrorDescriptorError;
use crate::primitives::RefsError;

/// Effect/interaction errors.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum EffectError {
    /// Effect kind did not match input variant.
    #[error("effect kind/input mismatch")]
    KindMismatch,
    /// Principal/authorization pairing invalid.
    #[error("interaction cancellation requires both principal and authorization or neither")]
    InvalidCancellationPair,
    /// Semantic array exceeded its v1 item ceiling.
    #[error("{field} has {len} items; max {max}")]
    TooManyItems {
        /// Field name.
        field: &'static str,
        /// Observed item count.
        len: usize,
        /// Maximum item count.
        max: usize,
    },
    /// Settlement identity or output contract differed from the originating request.
    #[error("effect settlement identity/output contract mismatch")]
    SettlementMismatch,
    /// Failure descriptor violates durable semantic limits.
    #[error(transparent)]
    InvalidErrorDescriptor(ErrorDescriptorError),
    /// Label error.
    #[error(transparent)]
    Refs(#[from] RefsError),
    /// Content error.
    #[error(transparent)]
    Content(#[from] ContentError),
    /// Serialization failed.
    #[error("serialize failed: {detail}")]
    Serialize {
        /// Detail.
        detail: String,
    },
}

impl EffectError {
    /// Stable error code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::KindMismatch => "effect_kind_mismatch",
            Self::InvalidCancellationPair => "invalid_cancellation_pair",
            Self::TooManyItems { .. } => "too_many_items",
            Self::SettlementMismatch => "effect_settlement_mismatch",
            Self::InvalidErrorDescriptor(_) => "invalid_error_descriptor",
            Self::Refs(inner) => inner.code(),
            Self::Content(_) => "invalid_content",
            Self::Serialize { .. } => "serialize_failed",
        }
    }
}
