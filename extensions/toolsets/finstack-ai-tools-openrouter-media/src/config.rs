//! Construction config, error codes, and endpoint validation.

use std::net::IpAddr;

use thiserror::Error;

pub(crate) const DEFAULT_ENDPOINT: &str = "https://openrouter.ai";
pub(crate) const MAX_RESULT_BYTES_CEILING: usize = 8 * 1_048_576;

/// Stable missing-credential code.
pub const OPENROUTER_MEDIA_CREDENTIAL_REQUIRED: &str = "openrouter_media_credential_required";
/// Stable endpoint-configuration code.
pub const OPENROUTER_MEDIA_ENDPOINT_INVALID: &str = "openrouter_media_endpoint_invalid";
/// Stable argument-validation code.
pub const OPENROUTER_MEDIA_INVALID_ARGUMENTS: &str = "openrouter_media_invalid_arguments";
/// Stable remote-transport code.
pub const OPENROUTER_MEDIA_TRANSPORT_FAILED: &str = "openrouter_media_transport_failed";
/// Stable output-limit code.
pub const OPENROUTER_MEDIA_LIMIT_EXCEEDED: &str = "openrouter_media_limit_exceeded";
/// Stable cancellation/deadline code.
pub const OPENROUTER_MEDIA_TIMEOUT: &str = "openrouter_media_timeout";

/// Explicit `OpenRouter` media route. Never populated from the environment.
#[derive(Clone)]
pub struct OpenRouterMediaConfig {
    /// Explicit API key. Empty values fail closed.
    ///
    /// Construction validates the value as
    /// [`SecretString`](finstack_ai_runtime::SecretString) and stores it as
    /// an `Authorization` [`HeaderValue`](reqwest::header::HeaderValue) so
    /// the raw key is not cloned on every call.
    pub api_key: String,
    /// HTTPS endpoint, or loopback HTTP for scripted fixtures. Empty selects
    /// `https://openrouter.ai`.
    pub endpoint: String,
    /// Optional non-secret `HTTP-Referer` attribution header.
    pub referer: Option<String>,
    /// Optional non-secret `X-Title` attribution header.
    pub title: Option<String>,
    /// Result-size cap in bytes. Images and speech come back base64, so
    /// hosts wanting inline media raise this. Zero or above 8 MiB fails
    /// construction; the SDK default is `262_144`.
    pub max_result_bytes: usize,
}

impl std::fmt::Debug for OpenRouterMediaConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenRouterMediaConfig")
            .field("api_key", &"[redacted]")
            .field("endpoint", &self.endpoint)
            .field("referer", &self.referer)
            .field("title", &self.title)
            .field("max_result_bytes", &self.max_result_bytes)
            .finish()
    }
}

/// Construction or route failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum OpenRouterMediaError {
    /// API key was omitted.
    #[error(
        "{OPENROUTER_MEDIA_CREDENTIAL_REQUIRED}: openrouter media construction requires an explicit API key"
    )]
    CredentialRequired,
    /// Endpoint scheme, host, components, or result cap are invalid.
    #[error("{OPENROUTER_MEDIA_ENDPOINT_INVALID}: {reason}")]
    EndpointInvalid {
        /// Stable non-secret reason.
        reason: &'static str,
    },
}

pub(crate) fn validate_endpoint(value: &str) -> Result<(), OpenRouterMediaError> {
    let Some((scheme, rest)) = value.split_once("://") else {
        return Err(OpenRouterMediaError::EndpointInvalid {
            reason: "endpoint must be an http or https URL",
        });
    };
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return Err(OpenRouterMediaError::EndpointInvalid {
            reason: "endpoint must be an http or https URL",
        });
    }
    if rest.contains('@') || rest.contains('?') || rest.contains('#') {
        return Err(OpenRouterMediaError::EndpointInvalid {
            reason: "endpoint contains forbidden components",
        });
    }
    let host = endpoint_host(rest).ok_or(OpenRouterMediaError::EndpointInvalid {
        reason: "endpoint host is missing",
    })?;
    if scheme.eq_ignore_ascii_case("http") && !is_loopback_host(host) {
        return Err(OpenRouterMediaError::EndpointInvalid {
            reason: "plaintext HTTP is allowed only for loopback endpoints",
        });
    }
    Ok(())
}

pub(crate) fn endpoint_host(rest: &str) -> Option<&str> {
    if let Some(rest) = rest.strip_prefix('[') {
        return rest.split(']').next().filter(|host| !host.is_empty());
    }
    rest.split(['/', ':'])
        .next()
        .filter(|host| !host.is_empty())
}

pub(crate) fn is_loopback_host(host: &str) -> bool {
    let host = host
        .strip_prefix('[')
        .map_or(host, |rest| rest.strip_suffix(']').unwrap_or(rest));
    host.eq_ignore_ascii_case("localhost")
        || host.parse::<IpAddr>().is_ok_and(|addr| addr.is_loopback())
}
