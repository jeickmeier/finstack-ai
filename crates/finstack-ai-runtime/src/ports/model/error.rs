use finstack_ai_kernel::{ErrorCategory, ErrorCode, ErrorDescriptor, Metadata};
use thiserror::Error;

use crate::error::{PortErrorData, PortErrorInvalid};

use super::{
    MODEL_CONTEXT_LIMIT_EXCEEDED, MODEL_ESTIMATOR_MISMATCH, MODEL_PROFILE_INVALID,
    MODEL_PROFILE_OVERRIDE_NOT_ALLOWED, MODEL_PROFILE_RELAXATION, MODEL_RECONCILIATION_UNSUPPORTED,
    MODEL_REQUEST_INVALID, MODEL_RESPONSE_MISMATCH, MODEL_STREAM_DUPLICATE_COMPLETION,
    MODEL_STREAM_ERROR_AFTER_COMPLETION, MODEL_STREAM_ITEM_AFTER_COMPLETION,
    MODEL_STREAM_LIMIT_EXCEEDED, MODEL_STREAM_MISSING_COMPLETION,
    MODEL_TOOL_CALL_ARGUMENTS_INVALID, MODEL_TOOL_CALL_DELTA_INVALID, MODEL_TOOL_CALL_INCOMPLETE,
    MODEL_USAGE_INVALID,
};

pub(super) const STREAM_TEXT_MAX_BYTES: usize = 1_048_576;
pub(super) const STREAM_REASONING_MAX_BYTES: usize = 1_048_576;

/// Stable model adapter error.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{data}")]
pub struct ModelError {
    data: PortErrorData,
}

impl From<PortErrorInvalid> for ModelError {
    fn from(error: PortErrorInvalid) -> Self {
        match error {
            PortErrorInvalid::InvalidCode => {
                Self::validation(MODEL_REQUEST_INVALID, "invalid model error code")
            }
            PortErrorInvalid::InvalidMessage => {
                Self::validation(MODEL_REQUEST_INVALID, "invalid model error message")
            }
            PortErrorInvalid::InvalidClassification => Self::validation(
                MODEL_REQUEST_INVALID,
                "reserved model adapter code has an invalid classification",
            ),
        }
    }
}

impl ModelError {
    /// Construct a bounded adapter error.
    ///
    /// # Errors
    ///
    /// Returns [`PortErrorInvalid`] when the supplied code, message, or reserved
    /// classification is invalid.
    pub fn try_new(
        code: impl AsRef<str>,
        category: ErrorCategory,
        retryable: bool,
        message: impl AsRef<str>,
        metadata: Metadata,
    ) -> Result<Self, PortErrorInvalid> {
        let data = PortErrorData::try_from_parts(
            code,
            category,
            retryable,
            message,
            metadata,
            STREAM_TEXT_MAX_BYTES,
        )?;
        if let Some(expected) = reserved_adapter_category(data.code().as_str())
            && (data.category() != expected || data.retryable())
        {
            return Err(PortErrorInvalid::InvalidClassification);
        }
        Ok(Self { data })
    }

    pub(crate) fn frozen(
        code: &'static str,
        category: ErrorCategory,
        retryable: bool,
        message: &'static str,
    ) -> Self {
        Self {
            data: PortErrorData::frozen(code, category, retryable, message),
        }
    }

    pub(crate) fn validation(code: &'static str, message: &'static str) -> Self {
        Self::frozen(code, ErrorCategory::Validation, false, message)
    }

    pub(crate) fn limit(code: &'static str, message: &'static str) -> Self {
        Self {
            data: PortErrorData::frozen(code, ErrorCategory::Limit, false, message),
        }
    }

    /// Stable adapter code.
    #[must_use]
    pub fn code(&self) -> &ErrorCode {
        self.data.code()
    }

    /// Stable framework category.
    #[must_use]
    pub const fn category(&self) -> ErrorCategory {
        self.data.category()
    }

    /// Retryability classification.
    #[must_use]
    pub const fn retryable(&self) -> bool {
        self.data.retryable()
    }

    /// Safe bounded message.
    #[must_use]
    pub fn message(&self) -> &str {
        self.data.message()
    }

    /// Bounded namespaced safe metadata.
    #[must_use]
    pub const fn metadata(&self) -> &Metadata {
        self.data.metadata()
    }

    /// Convert to the kernel's source-free durable descriptor.
    ///
    /// # Errors
    ///
    /// Returns a stable request error only if an internal invariant is violated.
    pub fn to_descriptor(&self) -> Result<ErrorDescriptor, ModelError> {
        let mut descriptor = ErrorDescriptor::new(
            self.data.code(),
            self.data.message(),
            self.data.category(),
            self.data.retryable(),
        )
        .map_err(|_| {
            Self::validation(MODEL_REQUEST_INVALID, "model error descriptor is invalid")
        })?;
        descriptor.safe_details = self.data.metadata().clone();
        Ok(descriptor)
    }
}

fn reserved_adapter_category(code: &str) -> Option<ErrorCategory> {
    match code {
        MODEL_CONTEXT_LIMIT_EXCEEDED | MODEL_STREAM_LIMIT_EXCEEDED => Some(ErrorCategory::Limit),
        MODEL_REQUEST_INVALID
        | MODEL_PROFILE_INVALID
        | MODEL_PROFILE_RELAXATION
        | MODEL_PROFILE_OVERRIDE_NOT_ALLOWED
        | MODEL_ESTIMATOR_MISMATCH
        | MODEL_STREAM_MISSING_COMPLETION
        | MODEL_STREAM_DUPLICATE_COMPLETION
        | MODEL_STREAM_ITEM_AFTER_COMPLETION
        | MODEL_STREAM_ERROR_AFTER_COMPLETION
        | MODEL_TOOL_CALL_DELTA_INVALID
        | MODEL_TOOL_CALL_INCOMPLETE
        | MODEL_TOOL_CALL_ARGUMENTS_INVALID
        | MODEL_USAGE_INVALID
        | MODEL_RESPONSE_MISMATCH
        | MODEL_RECONCILIATION_UNSUPPORTED => Some(ErrorCategory::Validation),
        _ => None,
    }
}
