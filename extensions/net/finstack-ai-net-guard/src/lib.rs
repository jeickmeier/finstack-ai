//! Vetted outbound HTTP primitives: URL vetting, private-address deny,
//! DNS resolve-and-pin, and bounded body reads.

#![warn(missing_docs)]
#![forbid(unsafe_code)]
#![warn(clippy::float_cmp)]
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
#![deny(clippy::panic)]
#![deny(clippy::unreachable)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::indexing_slicing,
        clippy::float_cmp,
    )
)]
// Allow expect() in doc tests (they are test code)
#![doc(test(attr(allow(clippy::expect_used))))]

use thiserror::Error;

/// Stable invalid-URL code prefix.
pub const NET_GUARD_INVALID_URL: &str = "net_guard_invalid_url";
/// Stable blocked-destination code prefix.
pub const NET_GUARD_DESTINATION_BLOCKED: &str = "net_guard_destination_blocked";

/// Vetted-egress failure. Reasons are stable, non-secret strings.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum NetGuardError {
    /// The URL is malformed or violates scheme/component policy.
    #[error("{NET_GUARD_INVALID_URL}: {reason}")]
    InvalidUrl {
        /// Stable non-secret reason.
        reason: &'static str,
    },
    /// A literal or resolved address is loopback, private, link-local,
    /// or unique-local.
    #[error("{NET_GUARD_DESTINATION_BLOCKED}: {reason}")]
    DestinationBlocked {
        /// Stable non-secret reason.
        reason: &'static str,
    },
    /// DNS resolution failed or returned no addresses.
    #[error("net_guard_resolution_failed")]
    ResolutionFailed,
    /// The pinned HTTP client could not be constructed.
    #[error("net_guard_client_build_failed")]
    ClientBuildFailed,
    /// The transport failed mid-read.
    #[error("net_guard_transport_failed")]
    TransportFailed,
    /// The body exceeded the caller's byte cap.
    #[error("net_guard_limit_exceeded")]
    LimitExceeded,
}

mod vet;
pub use vet::{UrlPolicy, VettedUrl, parse_and_vet_url, is_loopback_host};

#[cfg(test)]
mod tests;
