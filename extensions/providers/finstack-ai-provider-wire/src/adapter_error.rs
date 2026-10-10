//! Shared adapter-error construction for first-party provider leaves.
//!
//! Each leaf owns its stable `pub(crate)` code strings (so the error-code
//! scanner keeps reading them from the leaf) and bundles them into one
//! [`ProviderErrorCodes`] value. The helpers here turn those codes plus a
//! classification into a [`ModelError`] without ever retaining an untrusted
//! response body.

use finstack_ai_kernel::{ErrorCategory, Metadata};
use finstack_ai_runtime::ports::model::{MediaResolveKind, ModelError, ResolveDraftMediaError};

use crate::{StreamNormError, StreamNormKind};

/// The stable adapter codes one provider leaf registers.
///
/// Every field is a leaf-owned `pub(crate) const`, so a leaf declares its
/// bundle in const context:
///
/// ```
/// use finstack_ai_provider_wire::ProviderErrorCodes;
///
/// const CODES: ProviderErrorCodes = ProviderErrorCodes {
///     config_invalid: "example_config_invalid",
///     request_invalid: "example_request_invalid",
///     http_error: "example_http_error",
///     transport_error: "example_transport_error",
///     timeout: "example_timeout",
///     cancelled: "example_cancelled",
///     stream_invalid: "example_stream_invalid",
///     stream_limit_exceeded: "example_stream_limit_exceeded",
///     response_invalid: "example_response_invalid",
/// };
/// assert_eq!(CODES.http_error, "example_http_error");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderErrorCodes {
    /// Provider or model configuration is invalid.
    pub config_invalid: &'static str,
    /// The outgoing request could not be translated to the wire protocol.
    pub request_invalid: &'static str,
    /// The endpoint answered with a non-success HTTP status.
    pub http_error: &'static str,
    /// The connection failed before or while reading the response.
    pub transport_error: &'static str,
    /// The whole-request timeout elapsed.
    pub timeout: &'static str,
    /// The host cancelled the request.
    pub cancelled: &'static str,
    /// A wire event could not be framed or decoded.
    pub stream_invalid: &'static str,
    /// An event or the accumulated stream exceeded a configured ceiling.
    pub stream_limit_exceeded: &'static str,
    /// The decoded payload violated the protocol contract or was truncated.
    pub response_invalid: &'static str,
}

/// Build a bounded adapter error with empty metadata.
///
/// This mirrors the leaves' local `error()` helper: when the code or message
/// is rejected by the runtime bound, the resulting validation error is
/// returned instead so callers never panic on a malformed constant.
#[must_use]
pub fn adapter_error(
    code: &'static str,
    category: ErrorCategory,
    retryable: bool,
    message: impl AsRef<str>,
) -> ModelError {
    ModelError::try_new(code, category, retryable, message, Metadata::empty())
        .unwrap_or_else(ModelError::from)
}

/// Build the leaf's `config_invalid` error under [`ErrorCategory::Configuration`].
#[must_use]
pub fn config_error(codes: &ProviderErrorCodes, message: impl AsRef<str>) -> ModelError {
    adapter_error(
        codes.config_invalid,
        ErrorCategory::Configuration,
        false,
        message,
    )
}

/// Build the leaf's `request_invalid` error under [`ErrorCategory::Validation`].
#[must_use]
pub fn request_error(codes: &ProviderErrorCodes, message: impl AsRef<str>) -> ModelError {
    adapter_error(
        codes.request_invalid,
        ErrorCategory::Validation,
        false,
        message,
    )
}

/// Build the final `cancelled` error (`"{vendor} request was cancelled"`).
#[must_use]
pub fn cancelled_error(codes: &ProviderErrorCodes, vendor: &'static str) -> ModelError {
    adapter_error(
        codes.cancelled,
        ErrorCategory::Cancellation,
        false,
        format!("{vendor} request was cancelled"),
    )
}

/// Build the retryable `timeout` error under [`ErrorCategory::Deadline`]
/// (`"{vendor} request timed out"`).
#[must_use]
pub fn timeout_error(codes: &ProviderErrorCodes, vendor: &'static str) -> ModelError {
    adapter_error(
        codes.timeout,
        ErrorCategory::Deadline,
        true,
        format!("{vendor} request timed out"),
    )
}

/// Build the final limit error for an outgoing payload over the stream cap
/// (`"{vendor} request exceeded the configured byte limit"`).
#[must_use]
pub fn request_limit_error(codes: &ProviderErrorCodes, vendor: &'static str) -> ModelError {
    adapter_error(
        codes.stream_limit_exceeded,
        ErrorCategory::Limit,
        false,
        format!("{vendor} request exceeded the configured byte limit"),
    )
}

/// Build the final `stream_limit_exceeded` error under [`ErrorCategory::Limit`]
/// (`"{vendor} response exceeded a configured stream limit"`).
#[must_use]
pub fn stream_limit_error(codes: &ProviderErrorCodes, vendor: &'static str) -> ModelError {
    adapter_error(
        codes.stream_limit_exceeded,
        ErrorCategory::Limit,
        false,
        format!("{vendor} response exceeded a configured stream limit"),
    )
}

/// Map a draft media-resolution failure to the leaf's stable adapter error.
///
/// | failure | code | category | retryable |
/// |---|---|---|---|
/// | no resolver configured | `request_invalid` | `Validation` | `false` |
/// | blob not found | `request_invalid` | `Validation` | `false` |
/// | resolver unavailable | `transport_error` | `Model` | `true` |
/// | any size limit | `stream_limit_exceeded` | `Limit` | `false` |
#[must_use]
pub fn draft_media_error(
    codes: &ProviderErrorCodes,
    vendor: &'static str,
    failure: ResolveDraftMediaError,
) -> ModelError {
    match failure {
        ResolveDraftMediaError::MissingResolver => {
            request_error(codes, "media content requires a configured media resolver")
        }
        ResolveDraftMediaError::Resolve(inner) => match inner.kind {
            MediaResolveKind::NotFound => request_error(codes, inner.message),
            MediaResolveKind::Unavailable => adapter_error(
                codes.transport_error,
                ErrorCategory::Model,
                true,
                format!("{vendor} media resolution is unavailable"),
            ),
            MediaResolveKind::Limit => stream_limit_error(codes, vendor),
        },
        ResolveDraftMediaError::Limit => stream_limit_error(codes, vendor),
    }
}

/// Build a durable HTTP-status failure that keeps only the status as context.
///
/// The status is attached as the `http_status` metadata member and rendered
/// as `"{vendor} {endpoint} returned HTTP {status}"`. The response body is
/// never consulted, so no untrusted or secret-bearing text can leak into the
/// durable error. Retryability follows the shared transient-status rule:
/// `408`, `409`, `429`, and every `5xx` status.
///
/// ```
/// use finstack_ai_kernel::ErrorCategory;
/// use finstack_ai_provider_wire::{ProviderErrorCodes, http_error};
///
/// const CODES: ProviderErrorCodes = ProviderErrorCodes {
///     config_invalid: "example_config_invalid",
///     request_invalid: "example_request_invalid",
///     http_error: "example_http_error",
///     transport_error: "example_transport_error",
///     timeout: "example_timeout",
///     cancelled: "example_cancelled",
///     stream_invalid: "example_stream_invalid",
///     stream_limit_exceeded: "example_stream_limit_exceeded",
///     response_invalid: "example_response_invalid",
/// };
/// let error = http_error(&CODES, "Example", "responses endpoint", 503);
/// assert_eq!(error.code().as_str(), "example_http_error");
/// assert_eq!(error.category(), ErrorCategory::Model);
/// assert!(error.retryable());
/// assert_eq!(error.message(), "Example responses endpoint returned HTTP 503");
/// ```
#[must_use]
pub fn http_error(
    codes: &ProviderErrorCodes,
    vendor: &'static str,
    endpoint: &'static str,
    status: u16,
) -> ModelError {
    let retryable = matches!(status, 408 | 409 | 429 | 500..=599);
    let metadata = Metadata::parse(format!(r#"{{"http_status":{status}}}"#))
        .unwrap_or_else(|_| Metadata::empty());
    let message = format!("{vendor} {endpoint} returned HTTP {status}");
    ModelError::try_new(
        codes.http_error,
        ErrorCategory::Model,
        retryable,
        message,
        metadata,
    )
    .unwrap_or_else(ModelError::from)
}

/// Build the non-retryable limit failure for a generation that stopped early.
///
/// The code is the leaf's `response_invalid` code under
/// [`ErrorCategory::Limit`], with the message
/// `"{vendor} response was truncated before completion"`.
#[must_use]
pub fn incomplete_error(codes: &ProviderErrorCodes, vendor: &'static str) -> ModelError {
    adapter_error(
        codes.response_invalid,
        ErrorCategory::Limit,
        false,
        format!("{vendor} response was truncated before completion"),
    )
}

/// Map a wire-normalization failure to the leaf's stable adapter error.
///
/// | [`StreamNormKind`] | code | category | retryable |
/// |---|---|---|---|
/// | `Limit` | `stream_limit_exceeded` | `Limit` | `false` |
/// | `Stream` | `stream_invalid` | `Model` | [`StreamNormError::retryable`] |
/// | `Response` | `response_invalid` | `Model` | `false` |
/// | `Incomplete` | `response_invalid` | `Limit` | `false` |
///
/// The `Limit` and `Incomplete` arms use fixed vendor-prefixed messages; the
/// other arms carry the normalizer's own safe diagnostic.
#[must_use]
pub fn stream_norm_error(
    codes: &ProviderErrorCodes,
    vendor: &'static str,
    error: &StreamNormError,
) -> ModelError {
    match error.kind {
        StreamNormKind::Limit => stream_limit_error(codes, vendor),
        StreamNormKind::Stream => adapter_error(
            codes.stream_invalid,
            ErrorCategory::Model,
            error.retryable,
            error.message,
        ),
        StreamNormKind::Response => adapter_error(
            codes.response_invalid,
            ErrorCategory::Model,
            false,
            error.message,
        ),
        StreamNormKind::Incomplete => incomplete_error(codes, vendor),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CODES: ProviderErrorCodes = ProviderErrorCodes {
        config_invalid: "test_config_invalid",
        request_invalid: "test_request_invalid",
        http_error: "test_http_error",
        transport_error: "test_transport_error",
        timeout: "test_timeout",
        cancelled: "test_cancelled",
        stream_invalid: "test_stream_invalid",
        stream_limit_exceeded: "test_stream_limit_exceeded",
        response_invalid: "test_response_invalid",
    };

    #[test]
    fn adapter_error_preserves_code_category_and_retryability() {
        let error = adapter_error(
            CODES.timeout,
            ErrorCategory::Deadline,
            true,
            String::from("Test request timed out"),
        );
        assert_eq!(error.code().as_str(), "test_timeout");
        assert_eq!(error.category(), ErrorCategory::Deadline);
        assert!(error.retryable());
        assert_eq!(error.message(), "Test request timed out");
        assert_eq!(error.metadata().as_raw_json().as_str(), "{}");
    }

    #[test]
    fn http_error_retryability_by_status() {
        for status in [408_u16, 409, 429, 500, 502, 503, 599] {
            let error = http_error(&CODES, "Test", "responses endpoint", status);
            assert!(error.retryable(), "status {status} must be retryable");
        }
        for status in [400_u16, 401, 402, 403, 404, 410, 422, 600] {
            let error = http_error(&CODES, "Test", "responses endpoint", status);
            assert!(!error.retryable(), "status {status} must not be retryable");
        }
        let error = http_error(&CODES, "Test", "models endpoint", 502);
        assert_eq!(error.code().as_str(), "test_http_error");
        assert_eq!(error.category(), ErrorCategory::Model);
        assert_eq!(
            error.metadata().as_raw_json().as_str(),
            r#"{"http_status":502}"#
        );
    }

    #[test]
    fn http_error_message_has_no_body() {
        let body = r#"{"error":"secret-canary-7b2"}"#;
        let error = http_error(&CODES, "Test", "responses endpoint", 402);
        assert_eq!(error.message(), "Test responses endpoint returned HTTP 402");
        assert!(!error.message().contains("secret-canary"));
        assert!(!error.metadata().as_raw_json().as_str().contains(body));
        assert_eq!(
            error.metadata().as_raw_json().as_str(),
            r#"{"http_status":402}"#
        );
    }

    #[test]
    fn request_phase_errors_keep_their_categories() {
        let cancelled = cancelled_error(&CODES, "Test");
        assert_eq!(cancelled.code().as_str(), "test_cancelled");
        assert_eq!(cancelled.category(), ErrorCategory::Cancellation);
        assert!(!cancelled.retryable());
        assert_eq!(cancelled.message(), "Test request was cancelled");

        let timeout = timeout_error(&CODES, "Test");
        assert_eq!(timeout.code().as_str(), "test_timeout");
        assert_eq!(timeout.category(), ErrorCategory::Deadline);
        assert!(timeout.retryable());

        let limit = request_limit_error(&CODES, "Test");
        assert_eq!(limit.code().as_str(), "test_stream_limit_exceeded");
        assert_eq!(limit.category(), ErrorCategory::Limit);
        assert_eq!(
            limit.message(),
            "Test request exceeded the configured byte limit"
        );

        let config = config_error(&CODES, "bad");
        assert_eq!(config.code().as_str(), "test_config_invalid");
        assert_eq!(config.category(), ErrorCategory::Configuration);
        let request = request_error(&CODES, "bad");
        assert_eq!(request.code().as_str(), "test_request_invalid");
        assert_eq!(request.category(), ErrorCategory::Validation);
    }

    #[test]
    fn draft_media_failures_map_by_kind() {
        let missing = draft_media_error(&CODES, "Test", ResolveDraftMediaError::MissingResolver);
        assert_eq!(missing.code().as_str(), "test_request_invalid");
        let limit = draft_media_error(&CODES, "Test", ResolveDraftMediaError::Limit);
        assert_eq!(limit.code().as_str(), "test_stream_limit_exceeded");
        assert_eq!(limit.category(), ErrorCategory::Limit);
        assert_eq!(
            limit.message(),
            "Test response exceeded a configured stream limit"
        );
        let unavailable = draft_media_error(
            &CODES,
            "Test",
            ResolveDraftMediaError::Resolve(finstack_ai_runtime::ports::model::MediaResolveError {
                kind: MediaResolveKind::Unavailable,
                message: "down",
            }),
        );
        assert_eq!(unavailable.code().as_str(), "test_transport_error");
        assert!(unavailable.retryable());
        assert_eq!(
            unavailable.message(),
            "Test media resolution is unavailable"
        );
    }

    #[test]
    fn incomplete_error_is_a_final_limit_failure() {
        let error = incomplete_error(&CODES, "Test");
        assert_eq!(error.code().as_str(), "test_response_invalid");
        assert_eq!(error.category(), ErrorCategory::Limit);
        assert!(!error.retryable());
        assert_eq!(
            error.message(),
            "Test response was truncated before completion"
        );
    }

    #[test]
    fn stream_norm_error_maps_kinds() {
        let limit = stream_norm_error(&CODES, "Test", &StreamNormError::limit());
        assert_eq!(limit.code().as_str(), "test_stream_limit_exceeded");
        assert_eq!(limit.category(), ErrorCategory::Limit);
        assert!(!limit.retryable());
        assert_eq!(
            limit.message(),
            "Test response exceeded a configured stream limit"
        );

        let stream = stream_norm_error(&CODES, "Test", &StreamNormError::stream("bad frame"));
        assert_eq!(stream.code().as_str(), "test_stream_invalid");
        assert_eq!(stream.category(), ErrorCategory::Model);
        assert!(!stream.retryable());
        assert_eq!(stream.message(), "bad frame");

        let transient = stream_norm_error(
            &CODES,
            "Test",
            &StreamNormError::upstream("overloaded", true),
        );
        assert_eq!(transient.code().as_str(), "test_stream_invalid");
        assert_eq!(transient.category(), ErrorCategory::Model);
        assert!(transient.retryable());
        assert_eq!(transient.message(), "overloaded");

        let final_upstream =
            stream_norm_error(&CODES, "Test", &StreamNormError::upstream("invalid", false));
        assert!(!final_upstream.retryable());

        let response = stream_norm_error(&CODES, "Test", &StreamNormError::response("bad payload"));
        assert_eq!(response.code().as_str(), "test_response_invalid");
        assert_eq!(response.category(), ErrorCategory::Model);
        assert!(!response.retryable());
        assert_eq!(response.message(), "bad payload");

        let incomplete =
            stream_norm_error(&CODES, "Test", &StreamNormError::incomplete("max tokens"));
        assert_eq!(incomplete.code().as_str(), "test_response_invalid");
        assert_eq!(incomplete.category(), ErrorCategory::Limit);
        assert!(!incomplete.retryable());
        assert_eq!(
            incomplete.message(),
            "Test response was truncated before completion"
        );
    }
}
