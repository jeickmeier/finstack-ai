//! Destination vetting and DNS resolve-and-pin.

use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;

use crate::NetGuardError;
use crate::vet::VettedUrl;

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
            Ok(tokio::net::lookup_host((host.as_str(), port)).await?.collect())
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

fn blocked(addr: IpAddr, allow_loopback: bool) -> bool {
    let addr = addr.to_canonical();
    if allow_loopback && addr.is_loopback() {
        return false;
    }
    is_forbidden_destination(addr)
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
            return Err(NetGuardError::DestinationBlocked { reason: "literal_address_forbidden" });
        }
        return Ok(SocketAddr::new(ip, vetted.port));
    }
    let addrs = resolver
        .resolve(&vetted.host, vetted.port)
        .await
        .map_err(|_| NetGuardError::ResolutionFailed)?;
    let mut chosen = None;
    for addr in addrs {
        if blocked(addr.ip(), allow_loopback) {
            return Err(NetGuardError::DestinationBlocked { reason: "resolved_address_forbidden" });
        }
        if chosen.is_none() {
            chosen = Some(addr);
        }
    }
    chosen.ok_or(NetGuardError::ResolutionFailed)
}
