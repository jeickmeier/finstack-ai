//! Address-pinned client construction and bounded body reads.

use std::future::Future;
use std::net::SocketAddr;
use std::time::Duration;

use futures_util::StreamExt;

use crate::NetGuardError;
use crate::vet::VettedUrl;

/// Caller-provided cancellation and deadline futures for one body read.
pub struct BodyReadInterrupt<C, D> {
    cancellation: C,
    deadline: D,
}

impl<C, D> BodyReadInterrupt<C, D> {
    /// Bind the futures that interrupt the response body stream.
    #[must_use]
    pub const fn new(cancellation: C, deadline: D) -> Self {
        Self {
            cancellation,
            deadline,
        }
    }
}

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
    read_body_bounded_interruptible(
        response,
        cap,
        BodyReadInterrupt::new(std::future::pending::<()>(), std::future::pending::<()>()),
    )
    .await
}

/// Stream a bounded body while racing caller cancellation and deadline.
///
/// # Errors
///
/// Returns [`NetGuardError::Cancelled`] or
/// [`NetGuardError::DeadlineExceeded`] when the corresponding interrupt wins,
/// in addition to the transport and limit failures of [`read_body_bounded`].
pub async fn read_body_bounded_interruptible<C, D>(
    response: reqwest::Response,
    cap: usize,
    interrupt: BodyReadInterrupt<C, D>,
) -> Result<Vec<u8>, NetGuardError>
where
    C: Future<Output = ()>,
    D: Future<Output = ()>,
{
    let BodyReadInterrupt {
        cancellation,
        deadline,
    } = interrupt;
    tokio::pin!(cancellation);
    tokio::pin!(deadline);
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    loop {
        let next = tokio::select! {
            biased;
            () = &mut cancellation => return Err(NetGuardError::Cancelled),
            () = &mut deadline => return Err(NetGuardError::DeadlineExceeded),
            chunk = stream.next() => chunk,
        };
        let Some(chunk) = next else {
            break;
        };
        let chunk = chunk.map_err(|_| NetGuardError::TransportFailed)?;
        if body.len().saturating_add(chunk.len()) > cap {
            return Err(NetGuardError::LimitExceeded);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}
