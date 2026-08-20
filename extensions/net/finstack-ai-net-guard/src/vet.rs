//! URL parsing and scheme/component policy.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use url::Url;

use crate::NetGuardError;

/// Scheme/host rules for one outbound request.
#[derive(Debug, Clone, Copy)]
pub struct UrlPolicy {
    /// Plaintext HTTP permitted for loopback destinations (test fixtures).
    pub allow_loopback_http: bool,
}

/// Parsed, policy-vetted URL.
#[derive(Debug, Clone)]
pub struct VettedUrl {
    /// The parsed URL (serialization is what gets sent).
    pub url: Url,
    /// ASCII/punycode host as produced by the parser.
    pub host: String,
    /// Effective port (default 443 for https, scheme default otherwise).
    pub port: u16,
    /// True when the destination is loopback under a permissive policy.
    pub is_loopback: bool,
}

fn invalid(reason: &'static str) -> NetGuardError {
    NetGuardError::InvalidUrl { reason }
}

/// True only for the exact loopback names/addresses: `localhost`,
/// `127.0.0.1`, and `::1` (including the IPv4-mapped `::ffff:127.0.0.1`
/// form, which canonicalizes to `127.0.0.1`). This is deliberately
/// narrower than [`std::net::IpAddr::is_loopback`], which accepts the
/// entire `127.0.0.0/8` block: only the single conventional loopback
/// address per family is treated as loopback here, so e.g. `127.77.1.2`
/// is NOT loopback even though the OS would route it locally.
#[must_use]
pub fn is_loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<IpAddr>()
            .is_ok_and(|addr| {
                let canonical = addr.to_canonical();
                canonical == IpAddr::V4(Ipv4Addr::LOCALHOST) || canonical == IpAddr::V6(Ipv6Addr::LOCALHOST)
            })
}

/// Parse and vet: https only (http iff loopback allowed and host is
/// loopback), forbid userinfo and fragment, https port 443 only.
///
/// # Errors
///
/// Returns `NetGuardError::InvalidUrl` when the URL is malformed or violates
/// scheme/component policy.
pub fn parse_and_vet_url(value: &str, policy: &UrlPolicy) -> Result<VettedUrl, NetGuardError> {
    let url = Url::parse(value).map_err(|_| invalid("url_malformed"))?;
    let scheme = url.scheme();
    let https = scheme == "https";
    let http = scheme == "http";
    if !https && !http {
        return Err(invalid("scheme_not_allowed"));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(invalid("userinfo_forbidden"));
    }
    if url.fragment().is_some() {
        return Err(invalid("fragment_forbidden"));
    }
    let host = url.host_str().ok_or_else(|| invalid("host_missing"))?.to_owned();
    let port = url
        .port_or_known_default()
        .ok_or_else(|| invalid("port_missing"))?;
    let loopback = is_loopback_host(&host);
    if http && !(policy.allow_loopback_http && loopback) {
        return Err(invalid("plaintext_http_forbidden"));
    }
    if https && port != 443 {
        return Err(invalid("https_port_forbidden"));
    }
    Ok(VettedUrl {
        url,
        host,
        port,
        is_loopback: http && loopback,
    })
}
