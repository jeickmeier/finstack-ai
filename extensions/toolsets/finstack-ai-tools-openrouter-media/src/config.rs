//! Construction config, error codes, and endpoint validation.

use finstack_ai_net_guard::{NetGuardError, UrlPolicy, validate_fixed_endpoint};
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
/// Stable code for an artifact-input call made without a configured store.
pub const OPENROUTER_MEDIA_STORE_REQUIRED: &str = "openrouter_media_store_required";

/// Explicit `OpenRouter` media route. Never populated from the environment.
#[derive(Clone)]
pub struct OpenRouterMediaConfig {
    /// Explicit API key. Empty values fail closed.
    ///
    /// Construction validates the value as
    /// [`SecretString`](finstack_ai_runtime::ports::model::SecretString) and stores it as
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

/// Validate the operator-configured endpoint through net-guard's fixed
/// endpoint rules: `https` anywhere, plaintext `http` only on loopback, no
/// userinfo, query, or fragment. A non-443 `https` port is accepted because
/// `OpenRouter`-compatible gateways commonly listen elsewhere.
pub(crate) fn validate_endpoint(value: &str) -> Result<(), OpenRouterMediaError> {
    let policy = UrlPolicy {
        allow_loopback_http: true,
        allow_nonstandard_https_port: true,
    };
    validate_fixed_endpoint(value, &policy)
        .map(drop)
        .map_err(|error| OpenRouterMediaError::EndpointInvalid {
            reason: endpoint_reason(&error),
        })
}

/// Map a net-guard rejection onto the crate's stable, non-secret reasons.
fn endpoint_reason(error: &NetGuardError) -> &'static str {
    match error {
        NetGuardError::InvalidUrl { reason } => match *reason {
            "plaintext_http_forbidden" => "plaintext HTTP is allowed only for loopback endpoints",
            "userinfo_forbidden" | "fragment_forbidden" | "query_forbidden" => {
                "endpoint contains forbidden components"
            }
            "host_missing" | "port_missing" => "endpoint host is missing",
            _ => "endpoint must be an http or https URL",
        },
        _ => "endpoint is invalid",
    }
}
