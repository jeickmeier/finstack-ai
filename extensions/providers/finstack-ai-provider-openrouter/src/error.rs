use finstack_ai_kernel::{ErrorCategory, Metadata};
use finstack_ai_runtime::ports::model::ModelError;

pub(crate) const CONFIG_INVALID: &str = "openrouter_config_invalid";
pub(crate) const REQUEST_INVALID: &str = "openrouter_request_invalid";
pub(crate) const HTTP_ERROR: &str = "openrouter_http_error";
pub(crate) const TRANSPORT_ERROR: &str = "openrouter_transport_error";
pub(crate) const TIMEOUT: &str = "openrouter_timeout";
pub(crate) const CANCELLED: &str = "openrouter_cancelled";
pub(crate) const STREAM_INVALID: &str = "openrouter_stream_invalid";
pub(crate) const STREAM_LIMIT_EXCEEDED: &str = "openrouter_stream_limit_exceeded";
pub(crate) const RESPONSE_INVALID: &str = "openrouter_response_invalid";

pub(crate) fn error(
    code: &'static str,
    category: ErrorCategory,
    retryable: bool,
    message: &'static str,
) -> ModelError {
    ModelError::try_new(code, category, retryable, message, Metadata::empty())
        .unwrap_or_else(ModelError::from)
}

/// Build a durable HTTP-status failure without retaining an untrusted response body.
pub(crate) fn http_error(endpoint: &'static str, status: u16) -> ModelError {
    let retryable = matches!(status, 408 | 409 | 429 | 500..=599);
    let metadata = Metadata::parse(format!(r#"{{"http_status":{status}}}"#))
        .unwrap_or_else(|_| Metadata::empty());
    let message = format!("OpenRouter {endpoint} returned HTTP {status}");
    ModelError::try_new(
        HTTP_ERROR,
        ErrorCategory::Model,
        retryable,
        message,
        metadata,
    )
    .unwrap_or_else(ModelError::from)
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
        "OpenRouter response was incomplete",
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
        "OpenRouter response exceeded a configured stream limit",
    )
}

#[cfg(test)]
mod tests {
    use super::http_error;

    #[test]
    fn http_error_keeps_only_safe_status_context() {
        let error = http_error("responses endpoint", 402);
        assert_eq!(
            error.message(),
            "OpenRouter responses endpoint returned HTTP 402"
        );
        assert!(!error.message().contains("secret-canary"));
    }
}
