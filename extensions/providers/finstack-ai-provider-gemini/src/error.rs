//! Stable Gemini adapter error codes and the thin constructors over the
//! shared `finstack-ai-provider-wire` adapter-error helpers.

use finstack_ai_kernel::ErrorCategory;
use finstack_ai_provider_wire::{ProviderErrorCodes, adapter_error};
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

/// The Gemini leaf's code bundle for the shared adapter-error helpers.
pub(crate) const CODES: ProviderErrorCodes = ProviderErrorCodes {
    config_invalid: GEMINI_CONFIG_INVALID,
    request_invalid: GEMINI_REQUEST_INVALID,
    http_error: GEMINI_HTTP_ERROR,
    transport_error: GEMINI_TRANSPORT_ERROR,
    timeout: GEMINI_TIMEOUT,
    cancelled: GEMINI_CANCELLED,
    stream_invalid: GEMINI_STREAM_INVALID,
    stream_limit_exceeded: GEMINI_STREAM_LIMIT_EXCEEDED,
    response_invalid: GEMINI_RESPONSE_INVALID,
};

pub(crate) fn error(
    code: &'static str,
    category: ErrorCategory,
    retryable: bool,
    message: &'static str,
) -> ModelError {
    adapter_error(code, category, retryable, message)
}

pub(crate) fn config_error(message: &'static str) -> ModelError {
    error(
        CODES.config_invalid,
        ErrorCategory::Configuration,
        false,
        message,
    )
}

pub(crate) fn request_error(message: &'static str) -> ModelError {
    error(
        CODES.request_invalid,
        ErrorCategory::Validation,
        false,
        message,
    )
}

pub(crate) fn stream_error(message: &'static str) -> ModelError {
    error(CODES.stream_invalid, ErrorCategory::Model, false, message)
}

pub(crate) fn stream_limit_error() -> ModelError {
    error(
        CODES.stream_limit_exceeded,
        ErrorCategory::Limit,
        false,
        "Gemini response exceeded a configured stream limit",
    )
}
