//! Destination vetting and DNS resolve-and-pin.

use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::pin::Pin;

use crate::NetGuardError;
use crate::vet::{UrlPolicy, VettedUrl, is_loopback_host};

/// Injectable DNS seam. Production uses [`SystemResolver`]; tests script
/// answer sets to exercise rebinding shapes.
pub trait HostResolver: Send + Sync {
    /// Resolve `host:port` to socket addresses.
    fn resolve(
        &self,
        host: &str,
        port: u16,
    ) -> Pin<Box<dyn Future<Output = std::io::Result<Vec<SocketAddr>>> + Send + '_>>;
}

/// System resolver over `tokio::net::lookup_host`.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemResolver;

impl HostResolver for SystemResolver {
    fn resolve(
        &self,
        host: &str,
        port: u16,
    ) -> Pin<Box<dyn Future<Output = std::io::Result<Vec<SocketAddr>>> + Send + '_>> {
        let host = host.to_owned();
        Box::pin(async move {
            Ok(tokio::net::lookup_host((host.as_str(), port))
                .await?
                .collect())
        })
    }
}

/// True when the canonicalized address is loopback, private, link-local,
/// unspecified, broadcast, multicast, or unique-local.
/// IPv4-mapped IPv6 is canonicalized first.
#[must_use]
pub fn is_forbidden_destination(addr: IpAddr) -> bool {
    match addr.to_canonical() {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_multicast()
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unique_local()
                || v6.is_unicast_link_local()
                || v6.is_unspecified()
                || v6.is_multicast()
        }
    }
}

/// `allow_loopback` exempts only the exact loopback addresses `127.0.0.1`
/// and `::1` (after canonicalization, which collapses `::ffff:127.0.0.1` to
/// `127.0.0.1`) — narrower than `IpAddr::is_loopback`, which accepts the
/// whole `127.0.0.0/8` block; see `vet::is_loopback_host`.
fn blocked(addr: IpAddr, allow_loopback: bool) -> bool {
    let addr = addr.to_canonical();
    let exact_loopback =
        addr == IpAddr::V4(Ipv4Addr::LOCALHOST) || addr == IpAddr::V6(Ipv6Addr::LOCALHOST);
    if allow_loopback && exact_loopback {
        return false;
    }
    is_forbidden_destination(addr)
}

/// Reject a literal loopback/private/forbidden destination address (or the
/// bare string `localhost`) synchronously, before any network I/O.
///
/// [`resolve_and_pin`] performs the equivalent check for literal and
/// resolved hosts alike, using the same internal destination-blocking
/// logic this function calls, but only after an async DNS-resolution
/// step, and its loopback
/// allowance is scoped to the `http` scheme via `VettedUrl::is_loopback`.
/// An `https` URL with a literal loopback/private host would otherwise
/// sail through [`crate::parse_and_vet_url`] unrejected until that async
/// step. Callers that need an earlier, synchronous, scheme-independent
/// rejection — e.g. a pre-flight `validate_*` check performed before any
/// HTTP traffic is attempted — call this directly, computing the loopback
/// allowance from `policy` and the URL's own host rather than from scheme.
///
/// # Errors
///
/// [`NetGuardError::DestinationBlocked`] when the literal address (or the
/// bare string `localhost`) is not permitted under `policy`.
pub fn reject_literal_destination(
    vetted: &VettedUrl,
    policy: &UrlPolicy,
) -> Result<(), NetGuardError> {
    let allow_loopback = policy.allow_loopback_http && is_loopback_host(&vetted.host);
    let bare_host = vetted.host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(addr) = bare_host.parse::<IpAddr>() {
        if blocked(addr, allow_loopback) {
            return Err(NetGuardError::DestinationBlocked {
                reason: "literal_address_forbidden",
            });
        }
    } else if !allow_loopback && vetted.host.eq_ignore_ascii_case("localhost") {
        return Err(NetGuardError::DestinationBlocked {
            reason: "literal_address_forbidden",
        });
    }
    Ok(())
}

/// Resolve and vet every address; return the address to pin.
///
/// Every resolved address must pass — one bad address rejects the set,
/// so a rebinding resolver cannot smuggle a private answer in later.
///
/// # Errors
///
/// [`NetGuardError::DestinationBlocked`] when any address is forbidden;
/// [`NetGuardError::ResolutionFailed`] on resolver failure or empty sets.
pub async fn resolve_and_pin(
    vetted: &VettedUrl,
    resolver: &dyn HostResolver,
) -> Result<SocketAddr, NetGuardError> {
    let allow_loopback = vetted.is_loopback;
    let bare = vetted.host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = bare.parse::<IpAddr>() {
        if blocked(ip, allow_loopback) {
            return Err(NetGuardError::DestinationBlocked {
                reason: "literal_address_forbidden",
            });
        }
        return Ok(SocketAddr::new(ip, vetted.port));
    }
    let addrs = resolver
        .resolve(&vetted.host, vetted.port)
        .await
        .map_err(|_| NetGuardError::ResolutionFailed)?;
    if addrs.iter().any(|addr| blocked(addr.ip(), allow_loopback)) {
        return Err(NetGuardError::DestinationBlocked {
            reason: "resolved_address_forbidden",
        });
    }
    addrs
        .into_iter()
        .next()
        .ok_or(NetGuardError::ResolutionFailed)
}
