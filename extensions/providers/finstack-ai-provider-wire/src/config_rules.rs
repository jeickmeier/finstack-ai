//! Shared configuration rules for first-party provider leaves.
//!
//! Every leaf validates the same transport and model-profile invariants; the
//! functions here are the single source of those rules. They return plain
//! verdicts (`bool` or a small error enum) so each leaf can map them onto its
//! own stable configuration-error message.

use core::net::IpAddr;
use core::time::Duration;

use url::Url;

/// Longest whole-request timeout any leaf accepts.
const MAX_REQUEST_TIMEOUT: Duration = Duration::from_hours(1);

/// Why a provider base URL was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaseUrlError {
    /// The value is not a parseable absolute URL.
    Invalid,
    /// The URL uses a non-HTTP scheme or carries userinfo, a query, or a fragment.
    ForbiddenComponents,
    /// Plaintext `http` is only allowed when the host is a literal loopback IP.
    PlaintextNotLoopback,
    /// The URL carries a path other than `""` or `"/"`.
    ///
    /// Leaves replace the base path with their own endpoint path, so any
    /// configured prefix would be silently dropped; reject it instead.
    NonRootPath,
}

/// Why a model context profile was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextProfileError {
    /// A ceiling is zero, an output budget exceeds the window, or the reserved
    /// and overhead margins overflow or exceed the window.
    Invalid,
}

/// Validate a provider base URL.
///
/// Accepts an absolute `http` or `https` URL with no userinfo, query, or
/// fragment, and a path of `""` or `"/"`. Plaintext `http` is accepted only
/// for a literal loopback IP host (`127.0.0.1`, `[::1]`); `localhost` is
/// rejected because it resolves through the system resolver.
///
/// # Errors
///
/// Returns the first [`BaseUrlError`] rule that fails, checked in the order
/// `Invalid`, `ForbiddenComponents`, `PlaintextNotLoopback`, `NonRootPath`.
///
/// ```
/// use finstack_ai_provider_wire::{BaseUrlError, validate_base_url};
///
/// assert_eq!(validate_base_url("https://api.example.test"), Ok(()));
/// assert_eq!(validate_base_url("http://127.0.0.1:11434/"), Ok(()));
/// assert_eq!(
///     validate_base_url("https://api.example.test/v1"),
///     Err(BaseUrlError::NonRootPath)
/// );
/// assert_eq!(
///     validate_base_url("http://localhost:11434"),
///     Err(BaseUrlError::PlaintextNotLoopback)
/// );
/// ```
pub fn validate_base_url(value: &str) -> Result<(), BaseUrlError> {
    let url = Url::parse(value).map_err(|_| BaseUrlError::Invalid)?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(BaseUrlError::ForbiddenComponents);
    }
    if url.scheme() == "http"
        && !url
            .host_str()
            .and_then(|host| host.trim_matches(['[', ']']).parse::<IpAddr>().ok())
            .is_some_and(|address| address.is_loopback())
    {
        return Err(BaseUrlError::PlaintextNotLoopback);
    }
    if !matches!(url.path(), "" | "/") {
        return Err(BaseUrlError::NonRootPath);
    }
    Ok(())
}

/// Validate a model context profile.
///
/// Every ceiling must be non-zero, `max_output_tokens` must fit inside
/// `context_window_tokens`, and `reserved_output_tokens + provider_overhead_tokens`
/// must neither overflow nor exceed `context_window_tokens`.
///
/// # Errors
///
/// Returns [`ContextProfileError::Invalid`] when any rule fails.
///
/// ```
/// use finstack_ai_provider_wire::{ContextProfileError, validate_context_profile};
///
/// assert_eq!(
///     validate_context_profile(1_000_000, 128_000, 4_096, 4_096, 256),
///     Ok(())
/// );
/// assert_eq!(
///     validate_context_profile(1_000_000, 128_000, 200_000, 4_096, 256),
///     Err(ContextProfileError::Invalid)
/// );
/// ```
pub fn validate_context_profile(
    hard_input_bytes: u64,
    context_window_tokens: u64,
    max_output_tokens: u64,
    reserved_output_tokens: u64,
    provider_overhead_tokens: u64,
) -> Result<(), ContextProfileError> {
    if hard_input_bytes == 0
        || context_window_tokens == 0
        || max_output_tokens == 0
        || reserved_output_tokens == 0
        || max_output_tokens > context_window_tokens
        || reserved_output_tokens
            .checked_add(provider_overhead_tokens)
            .is_none_or(|total| total > context_window_tokens)
    {
        return Err(ContextProfileError::Invalid);
    }
    Ok(())
}

/// Whether a whole-request timeout is acceptable: non-zero and at most one hour.
#[must_use]
pub fn validate_request_timeout(timeout: Duration) -> bool {
    !timeout.is_zero() && timeout <= MAX_REQUEST_TIMEOUT
}

/// Whether raw event and total response byte limits are acceptable.
///
/// `max_event_bytes` must be non-zero and `max_stream_bytes` must hold at
/// least one event.
#[must_use]
pub const fn validate_stream_limits(max_event_bytes: usize, max_stream_bytes: usize) -> bool {
    max_event_bytes != 0 && max_stream_bytes >= max_event_bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_url_accepts_root_https_and_loopback_http() {
        for url in [
            "https://api.example.test",
            "https://api.example.test/",
            "https://api.example.test:8443",
            "http://127.0.0.1:8080",
            "http://127.0.0.1:8080/",
            "http://[::1]:8080",
        ] {
            assert_eq!(validate_base_url(url), Ok(()), "{url}");
        }
    }

    #[test]
    fn base_url_rejects_non_root_path() {
        for url in [
            "https://api.example.test/v1",
            "https://api.example.test/v1/",
            "https://api.example.test//",
            "http://127.0.0.1:11434/api",
        ] {
            assert_eq!(
                validate_base_url(url),
                Err(BaseUrlError::NonRootPath),
                "{url}"
            );
        }
    }

    #[test]
    fn base_url_rejects_unparseable_values() {
        for url in ["", "not a url", "api.example.test", "/relative"] {
            assert_eq!(validate_base_url(url), Err(BaseUrlError::Invalid), "{url}");
        }
    }

    #[test]
    fn base_url_rejects_credential_and_redirect_shaping_components() {
        for url in [
            "ftp://example.test",
            "https://user:pass@example.test",
            "https://user@example.test",
            "https://example.test?token=nope",
            "https://example.test/#fragment",
        ] {
            assert_eq!(
                validate_base_url(url),
                Err(BaseUrlError::ForbiddenComponents),
                "{url}"
            );
        }
    }

    #[test]
    fn plaintext_http_requires_a_literal_loopback_ip() {
        for url in [
            "http://example.test",
            "http://192.0.2.1",
            "http://localhost",
            "http://[::2]",
        ] {
            assert_eq!(
                validate_base_url(url),
                Err(BaseUrlError::PlaintextNotLoopback),
                "{url}"
            );
        }
    }

    #[test]
    fn context_profile_rule_matches_leaf_examples() {
        // The leaves' rustdoc examples.
        assert_eq!(
            validate_context_profile(1_000_000, 128_000, 4_096, 4_096, 256),
            Ok(())
        );
        // Margins may exactly fill the window.
        assert_eq!(validate_context_profile(1, 1_000, 1_000, 500, 500), Ok(()));
        // Zero overhead is allowed; every other zero is not.
        assert_eq!(validate_context_profile(1, 1_000, 10, 10, 0), Ok(()));
        for (hard, window, out, reserved, overhead) in [
            (0, 1_000, 10, 10, 0),
            (1, 0, 10, 10, 0),
            (1, 1_000, 0, 10, 0),
            (1, 1_000, 10, 0, 0),
            // Output budget above the window.
            (1, 1_000, 1_001, 10, 0),
            // Margins above the window.
            (1, 1_000, 10, 600, 500),
            // Margin overflow.
            (1, u64::MAX, 10, u64::MAX, 1),
        ] {
            assert_eq!(
                validate_context_profile(hard, window, out, reserved, overhead),
                Err(ContextProfileError::Invalid),
                "{hard} {window} {out} {reserved} {overhead}"
            );
        }
    }

    #[test]
    fn request_timeout_is_non_zero_and_at_most_one_hour() {
        assert!(validate_request_timeout(Duration::from_millis(1)));
        assert!(validate_request_timeout(Duration::from_mins(2)));
        assert!(validate_request_timeout(Duration::from_hours(1)));
        assert!(!validate_request_timeout(Duration::ZERO));
        assert!(!validate_request_timeout(Duration::from_secs(3_601)));
    }

    #[test]
    fn stream_limits_require_room_for_one_event() {
        assert!(validate_stream_limits(1, 1));
        assert!(validate_stream_limits(1_048_576, 16 * 1_048_576));
        assert!(!validate_stream_limits(0, 1));
        assert!(!validate_stream_limits(0, 0));
        assert!(!validate_stream_limits(2, 1));
    }
}
