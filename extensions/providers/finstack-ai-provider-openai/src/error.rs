use finstack_ai_runtime::{ErrorCategory, Metadata, ModelError};

pub(crate) const CONFIG_INVALID: &str = "openai_config_invalid";
pub(crate) const REQUEST_INVALID: &str = "openai_request_invalid";
pub(crate) const HTTP_ERROR: &str = "openai_http_error";
pub(crate) const TRANSPORT_ERROR: &str = "openai_transport_error";
pub(crate) const TIMEOUT: &str = "openai_timeout";
pub(crate) const CANCELLED: &str = "openai_cancelled";
pub(crate) const STREAM_INVALID: &str = "openai_stream_invalid";
pub(crate) const STREAM_LIMIT_EXCEEDED: &str = "openai_stream_limit_exceeded";
pub(crate) const RESPONSE_INVALID: &str = "openai_response_invalid";

pub(crate) fn error(
    code: &'static str,
    category: ErrorCategory,
    retryable: bool,
    message: &'static str,
) -> ModelError {
    ModelError::try_new(code, category, retryable, message, Metadata::empty())
        .expect("frozen OpenAI provider error is valid")
}

pub(crate) fn config_error(message: &'static str) -> ModelError {
    error(CONFIG_INVALID, ErrorCategory::Configuration, false, message)
}

pub(crate) fn request_error(message: &'static str) -> ModelError {
    error(REQUEST_INVALID, ErrorCategory::Validation, false, message)
}

pub(crate) fn response_error(message: &'static str) -> ModelError {
    error(RESPONSE_INVALID, ErrorCategory::Model, false, message)
}

pub(crate) fn incomplete_error() -> ModelError {
    error(
        RESPONSE_INVALID,
        ErrorCategory::Limit,
        false,
        "OpenAI response was incomplete",
    )
}

pub(crate) fn stream_error(message: &'static str) -> ModelError {
    error(STREAM_INVALID, ErrorCategory::Model, false, message)
}

pub(crate) fn stream_limit_error() -> ModelError {
    error(
        STREAM_LIMIT_EXCEEDED,
        ErrorCategory::Limit,
        false,
        "OpenAI response exceeded a configured stream limit",
    )
}
