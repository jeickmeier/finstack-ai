use std::sync::Arc;

use finstack_ai_kernel::{ErrorCategory, ErrorDescriptor, Metadata};
use thiserror::Error;

use crate::error::{PortErrorData, PortErrorInvalid};

use super::{
    COMPACTION_BUDGET_EXCEEDED, COMPACTION_MODEL_NOT_AUTHORIZED, COMPACTION_RESULT_INVALID,
    MIDDLEWARE_COMMIT_REQUIRED, MIDDLEWARE_OUTCOME_NOT_ALLOWED,
};

/// Stable middleware error.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{data}")]
pub struct MiddlewareError {
    data: PortErrorData,
}

impl From<PortErrorInvalid> for MiddlewareError {
    fn from(error: PortErrorInvalid) -> Self {
        match error {
            PortErrorInvalid::InvalidCode => Self::stable(
                MIDDLEWARE_OUTCOME_NOT_ALLOWED,
                "middleware error code is invalid",
            ),
            PortErrorInvalid::InvalidMessage => Self::stable(
                MIDDLEWARE_OUTCOME_NOT_ALLOWED,
                "middleware error message is invalid",
            ),
            PortErrorInvalid::InvalidClassification => Self::stable(
                MIDDLEWARE_OUTCOME_NOT_ALLOWED,
                "middleware error classification is invalid",
            ),
        }
    }
}

impl MiddlewareError {
    /// Construct a bounded middleware-specific error.
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

    pub(crate) fn stable(code: &'static str, message: &'static str) -> Self {
        Self {
            data: PortErrorData::frozen(
                code,
                if code == COMPACTION_BUDGET_EXCEEDED {
                    ErrorCategory::Limit
                } else {
                    ErrorCategory::Middleware
                },
                false,
                message,
            ),
        }
    }

    pub(crate) fn commit_required() -> Self {
        Self::stable(
            MIDDLEWARE_COMMIT_REQUIRED,
            "middleware invocation is not backed by the exact committed effect",
        )
    }

    pub(crate) fn outcome_not_allowed() -> Self {
        Self::stable(
            MIDDLEWARE_OUTCOME_NOT_ALLOWED,
            "middleware outcome is not allowed at this stage or descriptor tier",
        )
    }

    pub(crate) fn compaction_invalid() -> Self {
        Self::stable(
            COMPACTION_RESULT_INVALID,
            "compaction projection or evidence violates the frozen integrity contract",
        )
    }

    pub(crate) fn compaction_model_not_authorized() -> Self {
        Self::stable(
            COMPACTION_MODEL_NOT_AUTHORIZED,
            "compaction child model is not backed by an authorized related effect",
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
