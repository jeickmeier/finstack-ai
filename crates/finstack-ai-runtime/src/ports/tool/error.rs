use finstack_ai_kernel::{ErrorCategory, ErrorDescriptor, InteractionRequest, Metadata};
use thiserror::Error;

use crate::error::{PortErrorData, PortErrorInvalid};

const TOOL_TEXT_MAX_BYTES: usize = 1_048_576;

/// Stable unknown-tool closure code.
pub const UNKNOWN_TOOL: &str = "unknown_tool";
/// Stable invalid-argument closure code.
pub const TOOL_ARGUMENTS_INVALID: &str = "tool_arguments_invalid";
/// Stable invalid-output adapter code.
pub const TOOL_OUTPUT_INVALID: &str = "tool_output_invalid";
/// Stable malformed-stream adapter code.
pub const TOOL_STREAM_INVALID: &str = "tool_stream_invalid";
/// Stable result-bound adapter code.
pub const TOOL_RESULT_LIMIT_EXCEEDED: &str = "tool_result_limit_exceeded";
/// Stable stream-bound adapter code.
pub const TOOL_STREAM_LIMIT_EXCEEDED: &str = "tool_stream_limit_exceeded";
/// Stable approval-required closure code.
pub const TOOL_APPROVAL_REQUIRED: &str = "tool_approval_required";
/// Stable mid-tool HITL park code. The tool effect stays committed.
pub const TOOL_INTERACTION_REQUIRED: &str = "tool_interaction_required";
/// Stable intercept: a committed tool asked the host to run a nested model.
pub const MCP_SAMPLING_REQUIRED: &str = "mcp_sampling_required";
/// Stable reject when nested sampling has no locked model or parent budget.
pub const MCP_SAMPLING_UNAVAILABLE: &str = "mcp_sampling_unavailable";
/// Stable reject when a Toolset does not implement nested sampling completion.
pub const MCP_SAMPLING_UNSUPPORTED: &str = "mcp_sampling_unsupported";
/// Stable policy-denial closure code.
pub const TOOL_POLICY_DENIED: &str = "tool_policy_denied";
/// Stable cancellation adapter code.
pub const TOOL_CANCELLED: &str = "tool_cancelled";
/// Stable deadline adapter code.
pub const TOOL_DEADLINE_EXCEEDED: &str = "tool_deadline_exceeded";
/// Stable contained native-panic adapter code.
pub const TOOL_PANICKED: &str = "tool_panicked";
/// Stable registration or schema-resolution code.
pub const TOOL_REGISTRATION_INVALID: &str = "tool_registration_invalid";
/// Stable uncertainty code when a tool effect cannot be retried or classified.
pub const TOOL_RECONCILIATION_UNSUPPORTED: &str = "tool_reconciliation_unsupported";

/// Stable Toolset adapter error.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{data}")]
pub struct ToolError {
    data: PortErrorData,
}

impl From<PortErrorInvalid> for ToolError {
    fn from(error: PortErrorInvalid) -> Self {
        match error {
            PortErrorInvalid::InvalidCode => Self::registration("invalid tool error code"),
            PortErrorInvalid::InvalidMessage => Self::registration("invalid tool error message"),
            PortErrorInvalid::InvalidClassification => {
                Self::registration("reserved tool adapter code has an invalid classification")
            }
        }
    }
}

impl ToolError {
    /// Construct a bounded safe adapter error.
    ///
    /// # Errors
    ///
    /// Returns [`PortErrorInvalid`] when the supplied error representation is invalid.
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
            TOOL_TEXT_MAX_BYTES,
        )?;
        if let Some(expected) = reserved_category(data.code.as_str())
            && (data.category != expected || data.retryable)
        {
            return Err(PortErrorInvalid::InvalidClassification);
        }
        Ok(Self { data })
    }

    pub(crate) fn stable(code: &'static str, message: &'static str) -> Self {
        Self {
            data: PortErrorData::frozen(
                code,
                reserved_category(code).unwrap_or(ErrorCategory::Tool),
                false,
                message,
            ),
        }
    }

    pub(super) fn registration(message: &'static str) -> Self {
        Self::stable(TOOL_REGISTRATION_INVALID, message)
    }

    /// Stable adapter code.
    #[must_use]
    pub fn code(&self) -> &str {
        self.data.code.as_str()
    }

    /// Stable framework category.
    #[must_use]
    pub const fn category(&self) -> ErrorCategory {
        self.data.category
    }

    /// Retryability classification.
    #[must_use]
    pub const fn retryable(&self) -> bool {
        self.data.retryable
    }

    /// Safe bounded message.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.data.message
    }

    /// Bounded namespaced safe metadata.
    #[must_use]
    pub const fn metadata(&self) -> &Metadata {
        &self.data.metadata
    }

    /// Recover a durable HITL request from [`TOOL_INTERACTION_REQUIRED`] metadata.
    #[must_use]
    pub fn interaction_request(&self) -> Option<InteractionRequest> {
        if self.code() != TOOL_INTERACTION_REQUIRED {
            return None;
        }
        serde_json::from_slice(self.metadata().as_bytes()).ok()
    }

    /// Recover sampling params from [`MCP_SAMPLING_REQUIRED`] metadata.
    #[must_use]
    pub fn sampling_params(&self) -> Option<finstack_ai_kernel::RawJson> {
        if self.code() != MCP_SAMPLING_REQUIRED {
            return None;
        }
        finstack_ai_kernel::RawJson::parse(self.metadata().as_bytes()).ok()
    }

    /// Convert to a source-free durable kernel descriptor.
    ///
    /// # Errors
    ///
    /// Returns a registration error only if an internal invariant is violated.
    pub fn to_descriptor(&self) -> Result<ErrorDescriptor, Self> {
        let mut descriptor = ErrorDescriptor::new(
            self.data.code.as_str(),
            self.data.message.as_ref(),
            self.data.category,
            self.data.retryable,
        )
        .map_err(|_| Self::registration("tool error descriptor is invalid"))?;
        descriptor.safe_details = self.data.metadata.clone();
        Ok(descriptor)
    }
}

fn reserved_category(code: &str) -> Option<ErrorCategory> {
    match code {
        TOOL_ARGUMENTS_INVALID | TOOL_OUTPUT_INVALID | TOOL_STREAM_INVALID => {
            Some(ErrorCategory::Validation)
        }
        TOOL_RESULT_LIMIT_EXCEEDED | TOOL_STREAM_LIMIT_EXCEEDED => Some(ErrorCategory::Limit),
        TOOL_CANCELLED => Some(ErrorCategory::Cancellation),
        TOOL_DEADLINE_EXCEEDED => Some(ErrorCategory::Deadline),
        TOOL_PANICKED => Some(ErrorCategory::Internal),
        TOOL_REGISTRATION_INVALID => Some(ErrorCategory::Registration),
        UNKNOWN_TOOL
        | TOOL_APPROVAL_REQUIRED
        | TOOL_INTERACTION_REQUIRED
        | TOOL_POLICY_DENIED
        | MCP_SAMPLING_REQUIRED
        | MCP_SAMPLING_UNAVAILABLE
        | MCP_SAMPLING_UNSUPPORTED => Some(ErrorCategory::Tool),
        _ => None,
    }
}
