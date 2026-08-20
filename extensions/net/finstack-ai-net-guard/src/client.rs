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
/// ## Redirect loops
///
/// Because redirects are disabled here, the caller owns the hop loop, and
/// with it the security invariant this client depends on: **every** hop —
/// not just hop 0 — must re-run the full parse-and-vet → allowlist →
/// resolve-and-pin sequence on the redirect target before building a new
/// pinned client for it. Skipping re-vetting on later hops reopens exactly
/// the DNS-rebinding/SSRF gap this module exists to close.
///
/// A related trap: any loopback or fixture-only bypass (e.g. an
/// `allow_loopback_http`-style carve-out on the allowlist check) must be
/// decided from the *original*, hop-0 URL and never recomputed per hop.
/// Deciding it per hop would let an allowlisted public host 30x-redirect
/// into a loopback destination and have the bypass apply there too. See
/// `finstack-ai-tools-fetch`'s `host_allowed`/`origin_is_loopback`
/// (`pipeline.rs`) for the reference implementation: `origin_is_loopback`
/// is computed once from hop 0 and threaded unchanged through every
/// subsequent hop's allowlist check.
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
