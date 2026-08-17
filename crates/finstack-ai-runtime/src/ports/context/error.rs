use std::sync::Arc;

use finstack_ai_kernel::{ErrorCategory, ErrorDescriptor, Metadata};
use serde::Serialize;
use thiserror::Error;

use crate::error::{PortErrorData, PortErrorInvalid};

/// Stable invalid-context-configuration code.
pub const CONTEXT_CONFIGURATION_INVALID: &str = "context_configuration_invalid";
/// Stable missing-commit-boundary code.
pub const CONTEXT_COMMIT_REQUIRED: &str = "context_commit_required";
/// Stable invalid-contribution code.
pub const CONTEXT_CONTRIBUTION_INVALID: &str = "context_contribution_invalid";
/// Stable context-budget exhaustion code.
pub const CONTEXT_BUDGET_EXCEEDED: &str = "context_budget_exceeded";
/// Stable unresolved non-repeatable context-invocation code.
pub const CONTEXT_RECOVERY_UNCERTAIN: &str = "context_recovery_uncertain";

/// Stable context-port error.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{data}")]
pub struct ContextError {
    data: PortErrorData,
}

impl From<PortErrorInvalid> for ContextError {
    fn from(error: PortErrorInvalid) -> Self {
        match error {
            PortErrorInvalid::InvalidCode => Self::stable(
                CONTEXT_CONTRIBUTION_INVALID,
                "context error code is invalid",
            ),
            PortErrorInvalid::InvalidMessage => Self::stable(
                CONTEXT_CONTRIBUTION_INVALID,
                "context error message is invalid",
            ),
            PortErrorInvalid::InvalidClassification => Self::stable(
                CONTEXT_CONTRIBUTION_INVALID,
                "context error classification is invalid",
            ),
        }
    }
}

impl ContextError {
    /// Construct a bounded provider-specific context error.
    ///
    /// # Errors
    ///
    /// Returns [`PortErrorInvalid`] when the code or message is invalid.
    pub fn try_new(
        code: impl AsRef<str>,
        category: ErrorCategory,
        message: impl AsRef<str>,
        metadata: Metadata,
    ) -> Result<Self, PortErrorInvalid> {
        PortErrorData::try_from_parts(code, category, false, message, metadata, 1_048_576)
            .map(|data| Self { data })
    }

    pub(super) fn stable(code: &'static str, message: &'static str) -> Self {
        Self {
            data: PortErrorData::frozen(
                code,
                if code == CONTEXT_BUDGET_EXCEEDED {
                    ErrorCategory::Limit
                } else {
                    ErrorCategory::Validation
                },
                false,
                message,
            ),
        }
    }

    pub(super) fn commit_required() -> Self {
        Self::stable(
            CONTEXT_COMMIT_REQUIRED,
            "context invocation is not backed by the exact committed effect",
        )
    }

    /// Stable error code.
    #[must_use]
    pub fn code(&self) -> &str {
        self.data.code.as_str()
    }

    /// Safe error descriptor suitable for durable failure records.
    #[must_use]
    pub fn descriptor(&self) -> ErrorDescriptor {
        ErrorDescriptor {
            code: self.data.code.clone(),
            message: Arc::clone(&self.data.message),
            category: self.data.category,
            retryable: false,
            identifiers: finstack_ai_kernel::ErrorIdentifiers::default(),
            safe_details: self.data.metadata.clone(),
        }
    }
}

pub(super) fn canonical_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, ContextError> {
    serde_json_canonicalizer::to_vec(value).map_err(|_| {
        ContextError::stable(
            CONTEXT_CONTRIBUTION_INVALID,
            "context value could not be canonicalized",
        )
    })
}

pub(super) fn validate_label(value: &str, field: &'static str) -> Result<(), ContextError> {
    if value.is_empty() || value.len() > 256 || value.as_bytes().contains(&0) {
        return Err(ContextError::stable(CONTEXT_CONTRIBUTION_INVALID, field));
    }
    Ok(())
}

pub(super) fn validate_text(value: &str, field: &'static str) -> Result<(), ContextError> {
    if value.is_empty() || value.len() > 1_048_576 || value.as_bytes().contains(&0) {
        return Err(ContextError::stable(CONTEXT_CONTRIBUTION_INVALID, field));
    }
    Ok(())
}
