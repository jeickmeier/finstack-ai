use finstack_ai_net_guard::{NetGuardError, UrlPolicy, validate_fixed_endpoint};
use thiserror::Error;

use crate::{OPENAI_MEDIA_CREDENTIAL_REQUIRED, OPENAI_MEDIA_ENDPOINT_INVALID};

pub(crate) const DEFAULT_ENDPOINT: &str = "https://api.openai.com";

/// Explicit `OpenAI` media route. Never populated from the environment.
#[derive(Clone)]
pub struct OpenAiMediaConfig {
    /// Explicit API key. Empty values fail closed.
    ///
    /// Construction validates the value as
    /// [`SecretString`](finstack_ai_runtime::ports::model::SecretString) and stores it as
    /// an `Authorization` [`HeaderValue`](reqwest::header::HeaderValue) so
    /// the raw key is not cloned on every call.
    pub api_key: String,
    /// HTTPS endpoint, or loopback HTTP for scripted fixtures. Empty selects
    /// `https://api.openai.com`.
    pub endpoint: String,
    /// Result-size cap in bytes. Images and speech come back base64, so
    /// hosts wanting inline media raise this. Zero or above 8 MiB fails
    /// construction; the SDK default is `262_144`.
    pub max_result_bytes: usize,
}

impl std::fmt::Debug for OpenAiMediaConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiMediaConfig")
            .field("api_key", &"[redacted]")
            .field("endpoint", &self.endpoint)
            .field("max_result_bytes", &self.max_result_bytes)
            .finish()
    }
}

/// Construction or route failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum OpenAiMediaError {
    /// API key was omitted.
    #[error(
        "{OPENAI_MEDIA_CREDENTIAL_REQUIRED}: openai media construction requires an explicit API key"
    )]
    CredentialRequired,
    /// Endpoint scheme, host, components, or result cap are invalid.
    #[error("{OPENAI_MEDIA_ENDPOINT_INVALID}: {reason}")]
    EndpointInvalid {
        /// Stable non-secret reason.
        reason: &'static str,
    },
}

/// Validate the operator-configured endpoint through net-guard's fixed
/// endpoint rules: `https` anywhere, plaintext `http` only on loopback, no
/// userinfo, query, or fragment. A non-443 `https` port is accepted because
/// `OpenAI`-compatible gateways commonly listen elsewhere.
pub(crate) fn validate_endpoint(value: &str) -> Result<(), OpenAiMediaError> {
    let policy = UrlPolicy {
        allow_loopback_http: true,
        allow_nonstandard_https_port: true,
    };
    validate_fixed_endpoint(value, &policy)
        .map(drop)
        .map_err(|error| OpenAiMediaError::EndpointInvalid {
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
