use finstack_ai_kernel::{ErrorCategory, Metadata};
use finstack_ai_runtime::ports::model::ModelError;

pub(crate) const GEMINI_CONFIG_INVALID: &str = "gemini_config_invalid";
pub(crate) const GEMINI_REQUEST_INVALID: &str = "gemini_request_invalid";
pub(crate) const GEMINI_HTTP_ERROR: &str = "gemini_http_error";
pub(crate) const GEMINI_TRANSPORT_ERROR: &str = "gemini_transport_error";
pub(crate) const GEMINI_TIMEOUT: &str = "gemini_timeout";
pub(crate) const GEMINI_CANCELLED: &str = "gemini_cancelled";
pub(crate) const GEMINI_STREAM_INVALID: &str = "gemini_stream_invalid";
pub(crate) const GEMINI_STREAM_LIMIT_EXCEEDED: &str = "gemini_stream_limit_exceeded";
pub(crate) const GEMINI_RESPONSE_INVALID: &str = "gemini_response_invalid";

pub(crate) fn error(
    code: &'static str,
    category: ErrorCategory,
    retryable: bool,
    message: &'static str,
) -> ModelError {
    ModelError::try_new(code, category, retryable, message, Metadata::empty())
        .unwrap_or_else(ModelError::from)
}
