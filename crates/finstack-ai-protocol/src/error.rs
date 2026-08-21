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
    /// Length-prefixed framing is malformed.
    #[error("protocol frame rejected: {reason_code}")]
    InvalidFrame {
        /// Stable reason code.
        reason_code: &'static str,
    },
    /// Envelope family is not the expected protocol family.
    #[error("protocol envelope rejected: {reason_code}")]
    InvalidEnvelope {
        /// Stable reason code.
        reason_code: &'static str,
    },
    /// Envelope or handshake version is unsupported.
    #[error("protocol version rejected: {reason_code}")]
    UnsupportedVersion {
        /// Stable reason code.
        reason_code: &'static str,
    },
    /// A typed protocol message violates its semantic invariants.
    #[error("protocol message rejected: {reason_code}")]
    InvalidMessage {
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
            Self::InvalidCbor { reason_code }
            | Self::InvalidFrame { reason_code }
            | Self::InvalidEnvelope { reason_code }
            | Self::UnsupportedVersion { reason_code }
            | Self::InvalidMessage { reason_code }
            | Self::Integrity { reason_code } => reason_code,
            Self::Codec { .. } => "canonical_codec",
        }
    }

    pub(crate) fn invalid(reason_code: &'static str) -> Self {
        Self::InvalidCbor { reason_code }
    }

    pub(crate) fn invalid_frame(reason_code: &'static str) -> Self {
        Self::InvalidFrame { reason_code }
    }

    pub(crate) fn invalid_envelope(reason_code: &'static str) -> Self {
        Self::InvalidEnvelope { reason_code }
    }

    pub(crate) fn unsupported_version(reason_code: &'static str) -> Self {
        Self::UnsupportedVersion { reason_code }
    }

    pub(crate) fn invalid_message(reason_code: &'static str) -> Self {
        Self::InvalidMessage { reason_code }
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
