use finstack_ai_kernel::{ErrorCategory, Metadata};
use finstack_ai_runtime::ModelError;
use futures_util::StreamExt;

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
        .expect("frozen OpenRouter provider error is valid")
}

/// Largest error body read before the reason is extracted, so a malformed or
/// hostile endpoint cannot stream an unbounded body into an error message.
pub(crate) const ERROR_BODY_CAP: usize = 4096;

/// Read at most [`ERROR_BODY_CAP`] bytes from a rejected HTTP response.
///
/// A hostile or malformed endpoint can otherwise stream an unbounded body
/// through `Response::bytes` into an error path.
pub(crate) async fn read_error_body(response: reqwest::Response) -> Vec<u8> {
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let Ok(chunk) = chunk else {
            break;
        };
        let remaining = ERROR_BODY_CAP.saturating_sub(body.len());
        if remaining == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
    }
    body
}

/// Longest reason kept from an error body.
const ERROR_DETAIL_CHARS: usize = 400;

/// Build an HTTP-status failure that names the status and, when the endpoint
/// explained itself, the reason it gave.
///
/// A bare status collapses every 4xx into one indistinguishable error, and the
/// distinctions matter operationally: an unaffordable request comes back as
/// `402` with a body naming the exact token ceiling the balance covers, which
/// a caller can act on, while the status alone reads as an outage.
pub(crate) fn http_error(endpoint: &'static str, status: u16, body: &[u8]) -> ModelError {
    let retryable = matches!(status, 408 | 409 | 429 | 500..=599);
    let metadata = Metadata::parse(format!(r#"{{"http_status":{status}}}"#))
        .unwrap_or_else(|_| Metadata::empty());
    let message = match rejection_detail(body) {
        Some(detail) => format!("OpenRouter {endpoint} returned HTTP {status}: {detail}"),
        None => format!("OpenRouter {endpoint} returned HTTP {status}"),
    };
    ModelError::try_new(
        HTTP_ERROR,
        ErrorCategory::Model,
        retryable,
        message,
        metadata,
    )
    .expect("frozen OpenRouter HTTP error is valid")
}

/// Reduce an error response body to one short line.
fn rejection_detail(body: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(&body[..body.len().min(ERROR_BODY_CAP)]);
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    // OpenRouter reports failures as {"error":{"message":...}}; anything else
    // is surfaced verbatim so an unexpected shape still reaches the caller.
    let detail = serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .and_then(|value| {
            value
                .get("error")
                .and_then(|error| error.get("message"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_else(|| text.to_owned());
    let detail = detail.split_whitespace().collect::<Vec<_>>().join(" ");
    if detail.is_empty() {
        return None;
    }
    if detail.chars().count() <= ERROR_DETAIL_CHARS {
        return Some(detail);
    }
    let kept: String = detail.chars().take(ERROR_DETAIL_CHARS).collect();
    Some(format!("{kept}..."))
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
    use super::{ERROR_DETAIL_CHARS, http_error};

    #[test]
    fn an_unaffordable_request_keeps_the_endpoints_own_reason() {
        // A depleted balance is the common 402, and only the body says how
        // many tokens the balance still covers.
        let body = br#"{"error":{"message":"This request requires more credits, or fewer max_tokens. You requested up to 128000 tokens, but can only afford 106851.","code":402}}"#;
        let error = http_error("responses endpoint", 402, body);
        assert!(
            error.message().contains("can only afford 106851"),
            "the payable ceiling must survive into the message: {}",
            error.message()
        );
    }

    #[test]
    fn an_empty_body_still_names_the_status() {
        let error = http_error("responses endpoint", 402, b"");
        assert_eq!(
            error.message(),
            "OpenRouter responses endpoint returned HTTP 402"
        );
    }

    #[test]
    fn an_unexpected_body_shape_is_surfaced_verbatim_and_bounded() {
        let body = "unstructured gateway failure ".repeat(500);
        let error = http_error("models endpoint", 500, body.as_bytes());
        assert!(error.message().contains("unstructured gateway failure"));
        assert!(
            error.message().chars().count() < ERROR_DETAIL_CHARS * 2,
            "an unbounded body must not become an unbounded message"
        );
    }
}
