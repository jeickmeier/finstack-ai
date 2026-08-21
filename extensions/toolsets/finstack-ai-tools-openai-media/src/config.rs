use std::net::IpAddr;

use thiserror::Error;

use crate::{
    MAX_RESULT_BYTES_CEILING, OPENAI_MEDIA_CREDENTIAL_REQUIRED, OPENAI_MEDIA_ENDPOINT_INVALID,
};

pub(crate) const DEFAULT_ENDPOINT: &str = "https://api.openai.com";

/// Explicit `OpenAI` media route. Never populated from the environment.
#[derive(Clone)]
pub struct OpenAiMediaConfig {
    /// Explicit API key. Empty values fail closed.
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

pub(crate) fn validate_endpoint(value: &str) -> Result<(), OpenAiMediaError> {
    let Some((scheme, rest)) = value.split_once("://") else {
        return Err(OpenAiMediaError::EndpointInvalid {
            reason: "endpoint must be an http or https URL",
        });
    };
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return Err(OpenAiMediaError::EndpointInvalid {
            reason: "endpoint must be an http or https URL",
        });
    }
    if rest.contains('@') || rest.contains('?') || rest.contains('#') {
        return Err(OpenAiMediaError::EndpointInvalid {
            reason: "endpoint contains forbidden components",
        });
    }
    let host = endpoint_host(rest).ok_or(OpenAiMediaError::EndpointInvalid {
        reason: "endpoint host is missing",
    })?;
    if scheme.eq_ignore_ascii_case("http") && !is_loopback_host(host) {
        return Err(OpenAiMediaError::EndpointInvalid {
            reason: "plaintext HTTP is allowed only for loopback endpoints",
        });
    }
    Ok(())
}

fn endpoint_host(rest: &str) -> Option<&str> {
    if let Some(rest) = rest.strip_prefix('[') {
        return rest.split(']').next().filter(|host| !host.is_empty());
    }
    rest.split(['/', ':'])
        .next()
        .filter(|host| !host.is_empty())
}

fn is_loopback_host(host: &str) -> bool {
    let host = host
        .strip_prefix('[')
        .map_or(host, |rest| rest.strip_suffix(']').unwrap_or(rest));
    host.eq_ignore_ascii_case("localhost")
        || host.parse::<IpAddr>().is_ok_and(|addr| addr.is_loopback())
}

pub(crate) fn validate_result_cap(cap: usize) -> Result<(), OpenAiMediaError> {
    if cap == 0 || cap > MAX_RESULT_BYTES_CEILING {
        return Err(OpenAiMediaError::EndpointInvalid {
            reason: "result cap out of range",
        });
    }
    Ok(())
}
