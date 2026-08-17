use finstack_ai_runtime::{ErrorCategory, Metadata, ModelError};

pub(crate) const CONFIG_INVALID: &str = "ollama_config_invalid";
pub(crate) const REQUEST_INVALID: &str = "ollama_request_invalid";
pub(crate) const HTTP_ERROR: &str = "ollama_http_error";
pub(crate) const TRANSPORT_ERROR: &str = "ollama_transport_error";
pub(crate) const TIMEOUT: &str = "ollama_timeout";
pub(crate) const CANCELLED: &str = "ollama_cancelled";
pub(crate) const STREAM_INVALID: &str = "ollama_stream_invalid";
pub(crate) const STREAM_LIMIT_EXCEEDED: &str = "ollama_stream_limit_exceeded";
pub(crate) const RESPONSE_INVALID: &str = "ollama_response_invalid";

pub(crate) fn error(
    code: &'static str,
    category: ErrorCategory,
    retryable: bool,
    message: &'static str,
) -> ModelError {
    ModelError::try_new(code, category, retryable, message, Metadata::empty())
        .expect("frozen Ollama provider error is valid")
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

pub(crate) fn stream_error(message: &'static str) -> ModelError {
    error(STREAM_INVALID, ErrorCategory::Model, false, message)
}

pub(crate) fn stream_limit_error() -> ModelError {
    error(
        STREAM_LIMIT_EXCEEDED,
        ErrorCategory::Limit,
        false,
        "Ollama response exceeded a configured stream limit",
    )
}
