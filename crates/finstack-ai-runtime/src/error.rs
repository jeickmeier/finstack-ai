//! Shared port-error payload and construction failures.
//!
//! Durable and kernel boundaries must use source-free
//! [`finstack_ai_kernel::ErrorDescriptor`] values.

use core::fmt;
use std::sync::Arc;

use finstack_ai_kernel::{ErrorCategory, ErrorCode, Metadata};
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
            code: ErrorCode::from_static(code),
            category,
            retryable,
            message: Arc::from(message),
            metadata: Metadata::empty(),
        }
    }

    pub(crate) fn code(&self) -> &str {
        self.code.as_str()
    }

    pub(crate) const fn category(&self) -> ErrorCategory {
        self.category
    }

    pub(crate) const fn retryable(&self) -> bool {
        self.retryable
    }

    pub(crate) fn message(&self) -> &str {
        &self.message
    }

    pub(crate) const fn metadata(&self) -> &Metadata {
        &self.metadata
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
