use std::sync::Arc;

use finstack_ai_kernel::{ErrorCategory, ErrorCode, ErrorDescriptor, Metadata};
use thiserror::Error;

use crate::error::{PortErrorData, PortErrorInvalid};

use super::{
    COMPACTION_BUDGET_EXCEEDED, COMPACTION_RESULT_INVALID, MIDDLEWARE_OUTCOME_NOT_ALLOWED,
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

    /// Stable error code.
    #[must_use]
    pub fn code(&self) -> &ErrorCode {
        self.data.code()
    }

    /// Safe error descriptor suitable for durable failure records.
    #[must_use]
    pub fn descriptor(&self) -> ErrorDescriptor {
        ErrorDescriptor {
            code: self.data.code.clone(),
            message: Arc::clone(&self.data.message),
            category: self.data.category(),
            retryable: false,
            identifiers: finstack_ai_kernel::ErrorIdentifiers::default(),
            safe_details: self.data.metadata().clone(),
        }
    }
}

/// Stable code for a middleware outcome with no kernel landing path at the
/// stage it was produced at.
///
/// Covers outcomes that never have a `ReducerStageOutcome` peer at all
/// (`Suspend`, `Complete`, `RequestInteraction`, `RequestCompactionModel`),
/// and outcomes that have one only at a different stage than the one they
/// were produced at (`Retry` outside `BeforeFinalize`; `Replace` and
/// `CompactContext` outside the two stages that carry model context).
pub const MIDDLEWARE_STAGE_UNLANDABLE: &str = "middleware_stage_unlandable";

/// Stable code for a fold whose accumulated `AddInstructions`/`AddContext`
/// content would exceed a kernel-enforced array bound if landed.
///
/// This bound used to be checked only by the kernel, against the final
/// message array. It moves to the driver because middleware can now add to
/// that array; the driver checks what it can see (the aggregate additions),
/// which is a necessary — not sufficient — condition for the kernel accepting
/// the eventual `ReducerStageOutcome`.
pub const MIDDLEWARE_STAGE_BOUNDS_EXCEEDED: &str = "middleware_stage_bounds_exceeded";
