//! Protocol codec errors.

use thiserror::Error;

/// Canonical-CBOR and journal-codec failures.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ProtocolError {
    /// Input exceeded a v1 schema ceiling before allocation.
    #[error("canonical limit exceeded for {resource}: {limit}")]
    LimitExceeded {
        /// Stable resource name.
        resource: &'static str,
        /// Configured ceiling.
        limit: usize,
    },
    /// CBOR is not in the frozen v1 profile.
    #[error("canonical cbor rejected: {reason_code}")]
    InvalidCbor {
        /// Stable reason code.
        reason_code: &'static str,
    },
    /// Typed encode/decode failed after a valid value tree.
    #[error("canonical codec: {message}")]
    Codec {
        /// Diagnostic message without secrets.
        message: String,
    },
    /// Envelope checksum or payload digest did not match.
    #[error("journal integrity failure: {reason_code}")]
    Integrity {
        /// Stable reason code.
        reason_code: &'static str,
    },
}

impl ProtocolError {
    /// Stable lowercase error code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::LimitExceeded { .. } => "canonical_limit_exceeded",
            Self::InvalidCbor { reason_code } | Self::Integrity { reason_code } => reason_code,
            Self::Codec { .. } => "canonical_codec",
        }
    }

    pub(crate) fn invalid(reason_code: &'static str) -> Self {
        Self::InvalidCbor { reason_code }
    }

    pub(crate) fn limit(resource: &'static str, limit: usize) -> Self {
        Self::LimitExceeded { resource, limit }
    }

    /// Construct a typed codec failure.
    #[must_use]
    pub fn codec(message: impl Into<String>) -> Self {
        Self::Codec {
            message: message.into(),
        }
    }

    pub(crate) fn integrity(reason_code: &'static str) -> Self {
        Self::Integrity { reason_code }
    }
}

impl serde::ser::Error for ProtocolError {
    fn custom<T: std::fmt::Display>(msg: T) -> Self {
        Self::codec(msg.to_string())
    }
}

impl serde::de::Error for ProtocolError {
    fn custom<T: std::fmt::Display>(msg: T) -> Self {
        Self::codec(msg.to_string())
    }
}
