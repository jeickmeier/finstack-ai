//! Runtime error wrapper that may carry a local Rust source chain.
//!
//! Durable and kernel boundaries must use [`ErrorDescriptor`] only.

use core::fmt;
use std::error::Error as StdError;
use std::sync::Arc;

use finstack_ai_kernel::{ErrorCategory, ErrorCode, ErrorDescriptor, Metadata};
use thiserror::Error;

/// Shared source-free payload for primary-port adapter errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PortErrorData {
    pub(crate) code: ErrorCode,
    pub(crate) category: ErrorCategory,
    pub(crate) retryable: bool,
    pub(crate) message: Arc<str>,
    pub(crate) metadata: Metadata,
}

impl PortErrorData {
    pub(crate) fn try_from_parts(
        code: impl AsRef<str>,
        category: ErrorCategory,
        retryable: bool,
        message: impl AsRef<str>,
        metadata: Metadata,
        max_message_bytes: usize,
    ) -> Result<Self, PortErrorInvalid> {
        let code = ErrorCode::new(code).map_err(|_| PortErrorInvalid::InvalidCode)?;
        let message = message.as_ref();
        if message.is_empty()
            || message.len() > max_message_bytes
            || message.as_bytes().contains(&0)
        {
            return Err(PortErrorInvalid::InvalidMessage);
        }
        Ok(Self {
            code,
            category,
            retryable,
            message: Arc::from(message),
            metadata,
        })
    }

    pub(crate) fn frozen(
        code: &'static str,
        category: ErrorCategory,
        retryable: bool,
        message: &'static str,
    ) -> Self {
        Self {
            code: ErrorCode::new(code).expect("frozen port error code is valid"),
            category,
            retryable,
            message: Arc::from(message),
            metadata: Metadata::empty(),
        }
    }
}

impl fmt::Display for PortErrorData {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

/// Construction failure for a primary-port adapter error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum PortErrorInvalid {
    /// The supplied adapter code is empty, oversized, or otherwise invalid.
    #[error("port error code is invalid")]
    InvalidCode,
    /// The supplied adapter message is empty, oversized, or contains NUL.
    #[error("port error message is invalid")]
    InvalidMessage,
    /// A reserved adapter code was constructed with the wrong category or retryability.
    #[error("reserved adapter code has an invalid classification")]
    InvalidClassification,
}

/// Runtime error with an optional local diagnostic source chain.
#[derive(Clone)]
pub struct FrameworkError {
    /// Source-free serializable descriptor.
    pub descriptor: ErrorDescriptor,
    /// Local diagnostic source; never persisted or hashed.
    pub source: Option<Arc<dyn StdError + Send + Sync>>,
}

impl FrameworkError {
    /// Construct a framework error without a local source chain.
    #[must_use]
    pub fn from_descriptor(descriptor: ErrorDescriptor) -> Self {
        Self {
            descriptor,
            source: None,
        }
    }

    /// Attach a local diagnostic source.
    #[must_use]
    pub fn with_source(mut self, source: impl StdError + Send + Sync + 'static) -> Self {
        self.source = Some(Arc::new(source));
        self
    }

    /// Normalize to the source-free descriptor required at kernel / durable boundaries.
    #[must_use]
    pub fn into_descriptor(self) -> ErrorDescriptor {
        self.descriptor
    }

    /// Borrow the source-free descriptor.
    #[must_use]
    pub fn descriptor(&self) -> &ErrorDescriptor {
        &self.descriptor
    }
}

impl fmt::Debug for FrameworkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never dump unredacted source chains from Debug (may contain secrets).
        f.debug_struct("FrameworkError")
            .field("descriptor", &self.descriptor)
            .field(
                "source",
                &self.source.as_ref().map(|_| "<redacted local source>"),
            )
            .finish()
    }
}

impl fmt::Display for FrameworkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.descriptor)
    }
}

impl StdError for FrameworkError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        self.source
            .as_ref()
            .map(|error| error.as_ref() as &(dyn StdError + 'static))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use finstack_ai_kernel::{ErrorCategory, ErrorDescriptor};

    #[derive(Debug, thiserror::Error)]
    #[error("secret token=abc123")]
    struct SecretSource;

    #[test]
    fn debug_redacts_local_source_chain() {
        let error = FrameworkError::from_descriptor(
            ErrorDescriptor::new(
                "validation_failed",
                "input rejected",
                ErrorCategory::Validation,
                false,
            )
            .expect("descriptor"),
        )
        .with_source(SecretSource);
        let debug = format!("{error:?}");
        assert!(debug.contains("<redacted local source>"));
        assert!(!debug.contains("abc123"));
        assert!(!debug.contains("secret token"));
        assert!(error.source().is_some());
    }
}
