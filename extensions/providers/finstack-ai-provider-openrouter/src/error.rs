//! Stable adapter error codes and thin constructors for the `OpenRouter` leaf.
//!
//! The code strings are owned here so the error-code scanner keeps reading
//! them from this crate; construction is delegated to the shared
//! [`finstack_ai_provider_wire`] helpers.

use finstack_ai_kernel::ErrorCategory;
use finstack_ai_provider_wire::{ProviderErrorCodes, adapter_error};
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

/// Vendor label rendered into shared error messages.
pub(crate) const VENDOR: &str = "OpenRouter";

/// The leaf's code bundle for the shared wire error constructors.
pub(crate) const CODES: ProviderErrorCodes = ProviderErrorCodes {
    config_invalid: CONFIG_INVALID,
    request_invalid: REQUEST_INVALID,
    http_error: HTTP_ERROR,
    transport_error: TRANSPORT_ERROR,
    timeout: TIMEOUT,
    cancelled: CANCELLED,
    stream_invalid: STREAM_INVALID,
    stream_limit_exceeded: STREAM_LIMIT_EXCEEDED,
    response_invalid: RESPONSE_INVALID,
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
    error(CONFIG_INVALID, ErrorCategory::Configuration, false, message)
}

pub(crate) fn request_error(message: &'static str) -> ModelError {
    error(REQUEST_INVALID, ErrorCategory::Validation, false, message)
}

pub(crate) fn response_error(message: &'static str) -> ModelError {
    error(RESPONSE_INVALID, ErrorCategory::Model, false, message)
}
