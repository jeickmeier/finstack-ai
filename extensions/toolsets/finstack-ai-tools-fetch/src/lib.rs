//! Bounded, allowlisted HTTP fetch implementation of the public `Toolset` port.

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

mod config;

pub use config::{HostPattern, HttpFetchConfig, HttpFetchError, HttpFetchToolset};

/// Stable invalid-arguments error code.
pub const FETCH_INVALID_ARGUMENTS: &str = "fetch_invalid_arguments";
/// Stable host-not-allowlisted error code.
pub const FETCH_HOST_NOT_ALLOWLISTED: &str = "fetch_host_not_allowlisted";
/// Stable destination-blocked error code.
pub const FETCH_DESTINATION_BLOCKED: &str = "fetch_destination_blocked";
/// Stable redirect-denied error code.
pub const FETCH_REDIRECT_DENIED: &str = "fetch_redirect_denied";
/// Stable transport-failed error code.
pub const FETCH_TRANSPORT_FAILED: &str = "fetch_transport_failed";
/// Stable limit-exceeded error code.
pub const FETCH_LIMIT_EXCEEDED: &str = "fetch_limit_exceeded";
/// Stable timeout error code.
pub const FETCH_TIMEOUT: &str = "fetch_timeout";

#[cfg(test)]
mod tests;
