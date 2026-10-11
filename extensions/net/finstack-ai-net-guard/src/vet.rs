//! URL parsing and scheme/component policy.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use url::Url;

use crate::NetGuardError;

/// Scheme/host rules for one outbound request.
#[derive(Debug, Clone, Copy)]
pub struct UrlPolicy {
    /// Plaintext HTTP permitted for loopback destinations (test fixtures).
    pub allow_loopback_http: bool,
    /// Permit an `https` URL whose port is not 443.
    ///
    /// `false` (443-only) is the right default for a model-supplied URL: a
    /// non-standard port on an otherwise-plausible host is exactly the kind
    /// of thing an allowlist/SSRF check should treat with suspicion, and a
    /// model has no legitimate reason to ask for one. A caller that
    /// legitimately handles presigned URLs against a self-hosted
    /// object-store or CDN on a custom port (`MinIO`, a self-hosted
    /// S3-compatible endpoint, an internal media host behind a
    /// non-standard listener) sets this `true` explicitly to opt in.
    pub allow_nonstandard_https_port: bool,
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
                canonical == IpAddr::V4(Ipv4Addr::LOCALHOST)
                    || canonical == IpAddr::V6(Ipv6Addr::LOCALHOST)
            })
}

/// Parse and vet: https only (http iff loopback allowed and host is
/// loopback), forbid userinfo and fragment, https port 443 unless
/// `policy.allow_nonstandard_https_port` opts in to a different port.
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
    let host = url
        .host_str()
        .ok_or_else(|| invalid("host_missing"))?
        .to_owned();
    let port = url
        .port_or_known_default()
        .ok_or_else(|| invalid("port_missing"))?;
    let loopback = is_loopback_host(&host);
    if http && !(policy.allow_loopback_http && loopback) {
        return Err(invalid("plaintext_http_forbidden"));
    }
    if https && port != 443 && !policy.allow_nonstandard_https_port {
        return Err(invalid("https_port_forbidden"));
    }
    Ok(VettedUrl {
        url,
        host,
        port,
        is_loopback: http && loopback,
    })
}

/// Validate an operator-configured fixed endpoint (a provider base URL, a
/// webhook sink, a sandbox API host) once at construction time.
///
/// This applies every rule of [`parse_and_vet_url`] (https only, plaintext
/// `http` iff `policy.allow_loopback_http` and the host is loopback, no
/// userinfo, no fragment, https port 443 unless
/// `policy.allow_nonstandard_https_port`) and additionally rejects any query
/// component, including a bare trailing `?`. A fixed endpoint is a base that
/// the caller appends paths to; a query string on it would be silently
/// inherited by every request or dropped by path joining, so it is an
/// operator mistake either way.
///
/// No private-address deny list is applied. Destination vetting
/// ([`reject_literal_destination`](crate::reject_literal_destination),
/// [`resolve_and_pin`](crate::resolve_and_pin)) exists for model-supplied
/// URLs; an operator pointing a provider at `https://10.0.0.5/` is
/// legitimate. `localhost` stays accepted because [`is_loopback_host`]
/// treats it as loopback, which is the only way plaintext `http` passes.
///
/// # Errors
///
/// Returns `NetGuardError::InvalidUrl { reason: "query_forbidden" }` when
/// the URL carries a query, and every `InvalidUrl` reason that
/// [`parse_and_vet_url`] produces otherwise.
///
/// # Examples
///
/// ```
/// use finstack_ai_net_guard::{NetGuardError, UrlPolicy, validate_fixed_endpoint};
///
/// let policy = UrlPolicy {
///     allow_loopback_http: false,
///     allow_nonstandard_https_port: false,
/// };
/// let endpoint = validate_fixed_endpoint("https://api.example.com/v1", &policy)
///     .expect("fixed https endpoint");
/// assert_eq!(endpoint.host, "api.example.com");
///
/// let rejected = validate_fixed_endpoint("https://api.example.com/v1?key=x", &policy);
/// assert_eq!(
///     rejected.err(),
///     Some(NetGuardError::InvalidUrl {
///         reason: "query_forbidden"
///     })
/// );
/// ```
pub fn validate_fixed_endpoint(
    value: &str,
    policy: &UrlPolicy,
) -> Result<VettedUrl, NetGuardError> {
    let vetted = parse_and_vet_url(value, policy)?;
    if vetted.url.query().is_some() {
        return Err(invalid("query_forbidden"));
    }
    Ok(vetted)
}
