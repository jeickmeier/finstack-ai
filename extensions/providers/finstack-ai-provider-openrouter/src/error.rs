use finstack_ai_kernel::{ErrorCategory, Metadata};
use finstack_ai_runtime::ModelError;

pub(crate) const CONFIG_INVALID: &str = "openrouter_config_invalid";
pub(crate) const REQUEST_INVALID: &str = "openrouter_request_invalid";

pub(crate) fn error(
    code: &'static str,
    category: ErrorCategory,
    retryable: bool,
    message: &'static str,
) -> ModelError {
    ModelError::try_new(code, category, retryable, message, Metadata::empty())
        .expect("frozen OpenRouter provider error is valid")
}

pub(crate) fn config_error(message: &'static str) -> ModelError {
    error(CONFIG_INVALID, ErrorCategory::Configuration, false, message)
}

pub(crate) fn request_error(message: &'static str) -> ModelError {
    error(REQUEST_INVALID, ErrorCategory::Validation, false, message)
}
