//! Stable Anthropic adapter error codes and thin constructors.
//!
//! The code strings stay here as leaf-owned constants; construction is
//! delegated to the shared provider-wire helpers so category, retryability,
//! and body-free messages follow the same rules as every other first-party
//! provider.

use finstack_ai_kernel::ErrorCategory;
use finstack_ai_provider_wire::{ProviderErrorCodes, adapter_error};
use finstack_ai_runtime::ports::model::ModelError;

pub(crate) const CONFIG_INVALID: &str = "anthropic_config_invalid";
pub(crate) const REQUEST_INVALID: &str = "anthropic_request_invalid";
pub(crate) const HTTP_ERROR: &str = "anthropic_http_error";
pub(crate) const TRANSPORT_ERROR: &str = "anthropic_transport_error";
pub(crate) const TIMEOUT: &str = "anthropic_timeout";
pub(crate) const CANCELLED: &str = "anthropic_cancelled";
pub(crate) const STREAM_INVALID: &str = "anthropic_stream_invalid";
pub(crate) const STREAM_LIMIT_EXCEEDED: &str = "anthropic_stream_limit_exceeded";
pub(crate) const RESPONSE_INVALID: &str = "anthropic_response_invalid";

/// The leaf's code bundle for the shared provider-wire error helpers.
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

pub(crate) fn config_error(message: &'static str) -> ModelError {
    adapter_error(
        CODES.config_invalid,
        ErrorCategory::Configuration,
        false,
        message,
    )
}

pub(crate) fn request_error(message: &'static str) -> ModelError {
    adapter_error(
        CODES.request_invalid,
        ErrorCategory::Validation,
        false,
        message,
    )
}

pub(crate) fn stream_error(message: &'static str) -> ModelError {
    adapter_error(CODES.stream_invalid, ErrorCategory::Model, false, message)
}

pub(crate) fn stream_limit_error() -> ModelError {
    adapter_error(
        CODES.stream_limit_exceeded,
        ErrorCategory::Limit,
        false,
        "Anthropic response exceeded a configured stream limit",
    )
}
