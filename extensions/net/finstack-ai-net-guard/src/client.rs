//! Address-pinned client construction and bounded body reads.

use std::net::SocketAddr;
use std::time::Duration;

use futures_util::StreamExt;

use crate::NetGuardError;
use crate::vet::VettedUrl;

/// Build a reqwest client pinned to the vetted address. Redirects are
/// disabled; callers follow them manually so every hop is re-vetted.
/// Proxies (system/env, e.g. `HTTP_PROXY`/`HTTPS_PROXY`/`ALL_PROXY`) are
/// also disabled, since this crate has no proxy support as a non-goal.
///
/// # Errors
///
/// [`NetGuardError::ClientBuildFailed`] if the client cannot be constructed.
pub fn pinned_client(
    vetted: &VettedUrl,
    addr: SocketAddr,
    timeout: Duration,
) -> Result<reqwest::Client, NetGuardError> {
    reqwest::Client::builder()
        .http1_only()
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::none())
        // Disable env/system proxies: reqwest enables them by default, and
        // a proxy would receive the unpinned hostname, re-resolve it itself,
        // and bypass the `.resolve()` address pin below — reopening the
        // DNS-rebinding TOCTOU this client exists to close.
        .no_proxy()
        .resolve(&vetted.host, addr)
        .build()
        .map_err(|_| NetGuardError::ClientBuildFailed)
}

/// Stream the body up to `cap` bytes; exceeding the cap is an error,
/// never a truncation.
///
/// # Errors
///
/// [`NetGuardError::TransportFailed`] if the stream fails mid-read.
/// [`NetGuardError::LimitExceeded`] if the body exceeds the byte cap.
pub async fn read_body_bounded(
    response: reqwest::Response,
    cap: usize,
) -> Result<Vec<u8>, NetGuardError> {
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| NetGuardError::TransportFailed)?;
        if body.len().saturating_add(chunk.len()) > cap {
            return Err(NetGuardError::LimitExceeded);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}
