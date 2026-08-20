//! Fail-closed configuration for the bounded HTTP fetch toolset: host
//! allowlist patterns and the validated, redaction-aware config/toolset
//! shells consumed by the (later) `Toolset` port implementation.

use std::collections::BTreeMap;
use std::fmt;
use std::time::Duration;

use reqwest::header::{HeaderName, HeaderValue};
use thiserror::Error;

use crate::{
    FETCH_DESTINATION_BLOCKED, FETCH_HOST_NOT_ALLOWLISTED, FETCH_INVALID_ARGUMENTS,
    FETCH_LIMIT_EXCEEDED, FETCH_REDIRECT_DENIED, FETCH_TIMEOUT, FETCH_TRANSPORT_FAILED,
};

/// Hard ceiling on `HttpFetchConfig::max_response_bytes` (8 MiB).
pub(crate) const MAX_RESPONSE_BYTES_CEILING: usize = 8 * 1_048_576;
/// Default `HttpFetchConfig::max_response_bytes` (2 MiB).
pub(crate) const MAX_RESPONSE_BYTES_DEFAULT: usize = 2 * 1_048_576;
/// Hard ceiling on `HttpFetchConfig::request_timeout` (120 s).
pub(crate) const REQUEST_TIMEOUT_CEILING: Duration = Duration::from_mins(2);
/// Default `HttpFetchConfig::request_timeout` (30 s).
pub(crate) const REQUEST_TIMEOUT_DEFAULT: Duration = Duration::from_secs(30);
/// Hard ceiling on `HttpFetchConfig::max_redirects` (5).
pub(crate) const MAX_REDIRECTS_CEILING: usize = 5;
/// Default `HttpFetchConfig::max_redirects` (3).
pub(crate) const MAX_REDIRECTS_DEFAULT: usize = 3;

/// Redacted placeholder shown for per-host header values in `Debug` output.
const REDACTED: &str = "<redacted>";

/// One allowlist entry: exact host or `*.suffix` subdomain wildcard.
#[derive(Clone, PartialEq, Eq)]
pub struct HostPattern(PatternKind);

#[derive(Clone, PartialEq, Eq)]
enum PatternKind {
    /// Matches exactly this lowercase host.
    Exact(String),
    /// Matches any proper (non-empty) subdomain of this lowercase suffix,
    /// never the bare suffix itself.
    SubdomainOf(String),
}

impl HostPattern {
    /// Parse one allowlist entry: an exact host (`docs.rs`) or an explicit
    /// subdomain wildcard (`*.wikipedia.org`).
    ///
    /// # Errors
    ///
    /// Returns [`HttpFetchError::Configuration`] when the entry is empty,
    /// contains characters that are never valid in a bare host (`*`, `/`,
    /// `:`, `?`, `#`, `@`, whitespace) after stripping a `*.` prefix, or has
    /// an empty dot-separated label.
    pub fn parse(entry: &str) -> Result<Self, HttpFetchError> {
        let entry = entry.to_ascii_lowercase();
        let (wildcard, host) = match entry.strip_prefix("*.") {
            Some(rest) => (true, rest),
            None => (false, entry.as_str()),
        };
        let valid = !host.is_empty()
            && !host.contains(['*', '/', ':', '?', '#', '@', ' '])
            && host.split('.').all(|label| !label.is_empty());
        if !valid {
            return Err(HttpFetchError::Configuration {
                reason: "invalid_allowlist_entry",
            });
        }
        Ok(Self(if wildcard {
            PatternKind::SubdomainOf(host.to_owned())
        } else {
            PatternKind::Exact(host.to_owned())
        }))
    }

    /// Whether this pattern is an exact host (not a `*.suffix` wildcard).
    #[must_use]
    pub(crate) fn is_exact(&self) -> bool {
        matches!(self.0, PatternKind::Exact(_))
    }

    /// Case-insensitively test whether `host` matches this pattern.
    #[must_use]
    pub fn matches(&self, host: &str) -> bool {
        let host = host.to_ascii_lowercase();
        match &self.0 {
            PatternKind::Exact(exact) => host == *exact,
            PatternKind::SubdomainOf(suffix) => host
                .strip_suffix(suffix.as_str())
                .and_then(|prefix| prefix.strip_suffix('.'))
                .is_some_and(|rest| !rest.is_empty()),
        }
    }
}

/// Bounded HTTP fetch toolset configuration. Deny-by-default: an empty
/// `allowlist` makes the toolset unusable until filled in.
#[derive(Clone)]
pub struct HttpFetchConfig {
    /// Deny-by-default host allowlist entries, parsed by [`HostPattern::parse`].
    pub allowlist: Vec<String>,
    /// Maximum response body size in bytes. Default 2 MiB; hard ceiling 8 MiB.
    pub max_response_bytes: usize,
    /// Per-request timeout. Default 30 s; hard ceiling 120 s.
    pub request_timeout: Duration,
    /// Maximum redirect hops followed. Default 3; hard ceiling 5.
    pub max_redirects: usize,
    /// Static headers attached only when the request host exactly matches
    /// the key. Redacted from `Debug`. This is the sole cookie/auth door.
    pub per_host_headers: BTreeMap<String, Vec<(String, String)>>,
    /// Allow plaintext-HTTP loopback fixtures for tests. Default false.
    ///
    /// This also bypasses the `allowlist` match for loopback destinations:
    /// when a vetted URL resolves to a loopback host *and* this flag is
    /// set, the request flow skips the allowlist check entirely (net-guard
    /// still vets scheme/component policy and destination safety). This is
    /// what lets fixture servers on `127.0.0.1`/`localhost` run without
    /// adding every ephemeral test port to the allowlist; it has no effect
    /// on non-loopback hosts, which are always allowlist-gated.
    pub allow_loopback_http: bool,
    /// Optional `User-Agent` override.
    pub user_agent: Option<String>,
}

impl Default for HttpFetchConfig {
    fn default() -> Self {
        Self {
            allowlist: Vec::new(),
            max_response_bytes: MAX_RESPONSE_BYTES_DEFAULT,
            request_timeout: REQUEST_TIMEOUT_DEFAULT,
            max_redirects: MAX_REDIRECTS_DEFAULT,
            per_host_headers: BTreeMap::new(),
            allow_loopback_http: false,
            user_agent: None,
        }
    }
}

impl fmt::Debug for HttpFetchConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let redacted_headers: BTreeMap<&str, Vec<(&str, &str)>> = self
            .per_host_headers
            .iter()
            .map(|(host, headers)| {
                let headers = headers
                    .iter()
                    .map(|(name, _value)| (name.as_str(), REDACTED))
                    .collect();
                (host.as_str(), headers)
            })
            .collect();
        f.debug_struct("HttpFetchConfig")
            .field("allowlist", &self.allowlist)
            .field("max_response_bytes", &self.max_response_bytes)
            .field("request_timeout", &self.request_timeout)
            .field("max_redirects", &self.max_redirects)
            .field("per_host_headers", &redacted_headers)
            .field("allow_loopback_http", &self.allow_loopback_http)
            .field("user_agent", &self.user_agent)
            .finish()
    }
}

/// Bounded HTTP fetch failure. Reasons are stable, non-secret strings.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum HttpFetchError {
    /// The configuration failed validation.
    #[error("{FETCH_INVALID_ARGUMENTS}: {reason}")]
    Configuration {
        /// Stable non-secret reason.
        reason: &'static str,
    },
}

// Referenced so the frozen error-code constants stay linked to the crate's
// stable vocabulary even though this task does not yet map every code to a
// runtime failure path (Task 6 wires request-flow error mapping).
#[allow(dead_code)]
const _: [&str; 6] = [
    FETCH_HOST_NOT_ALLOWLISTED,
    FETCH_DESTINATION_BLOCKED,
    FETCH_REDIRECT_DENIED,
    FETCH_TRANSPORT_FAILED,
    FETCH_LIMIT_EXCEEDED,
    FETCH_TIMEOUT,
];

/// Validate `config` in isolation (allowlist parses, limits within their
/// ceilings, per-host header keys are exact hosts with valid HTTP
/// name/value pairs) and return the normalized config (per-host-header keys
/// lowercased) plus the parsed allowlist patterns.
///
/// Split out of `HttpFetchToolset::try_new` (in `toolset.rs`) so the pure
/// config-shape checks stay next to the types they validate.
///
/// # Errors
///
/// Returns [`HttpFetchError::Configuration`] when the allowlist is empty,
/// any allowlist or per-host-header-key entry fails [`HostPattern::parse`],
/// a per-host-header key is a wildcard pattern rather than an exact host,
/// any numeric limit is zero or exceeds its hard ceiling, or any per-host
/// header name/value is not a valid HTTP header.
pub(crate) fn validate(
    config: HttpFetchConfig,
) -> Result<(HttpFetchConfig, Vec<HostPattern>), HttpFetchError> {
    if config.allowlist.is_empty() {
        return Err(HttpFetchError::Configuration {
            reason: "allowlist_empty",
        });
    }
    let patterns = config
        .allowlist
        .iter()
        .map(|entry| HostPattern::parse(entry))
        .collect::<Result<Vec<_>, _>>()?;

    if config.max_response_bytes == 0 || config.max_response_bytes > MAX_RESPONSE_BYTES_CEILING {
        return Err(HttpFetchError::Configuration {
            reason: "max_response_bytes_out_of_range",
        });
    }
    if config.request_timeout.is_zero() || config.request_timeout > REQUEST_TIMEOUT_CEILING {
        return Err(HttpFetchError::Configuration {
            reason: "request_timeout_out_of_range",
        });
    }
    if config.max_redirects > MAX_REDIRECTS_CEILING {
        return Err(HttpFetchError::Configuration {
            reason: "max_redirects_out_of_range",
        });
    }

    let mut per_host_headers = BTreeMap::new();
    for (host, headers) in &config.per_host_headers {
        let pattern = HostPattern::parse(host)?;
        if !pattern.is_exact() {
            return Err(HttpFetchError::Configuration {
                reason: "per_host_header_key_not_exact_host",
            });
        }
        for (name, value) in headers {
            HeaderName::from_bytes(name.as_bytes()).map_err(|_| HttpFetchError::Configuration {
                reason: "invalid_per_host_header_name",
            })?;
            HeaderValue::from_str(value).map_err(|_| HttpFetchError::Configuration {
                reason: "invalid_per_host_header_value",
            })?;
        }
        per_host_headers.insert(host.to_ascii_lowercase(), headers.clone());
    }

    let config = HttpFetchConfig {
        per_host_headers,
        ..config
    };

    Ok((config, patterns))
}

impl fmt::Debug for PatternKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Exact(host) => f.debug_tuple("Exact").field(host).finish(),
            Self::SubdomainOf(suffix) => f.debug_tuple("SubdomainOf").field(suffix).finish(),
        }
    }
}

impl fmt::Debug for HostPattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
