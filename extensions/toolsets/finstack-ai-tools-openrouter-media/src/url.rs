//! Caller-supplied download URL validation and address-pinned fetches.
//!
//! Backed by `finstack-ai-net-guard`'s vetted-egress primitives: URL
//! parsing/vetting (`parse_and_vet_url`), DNS resolve-and-pin
//! (`resolve_and_pin`), a redirect-disabled address-pinned client
//! (`pinned_client`), and a bounded body read (`read_body_bounded`). This
//! replaces the crate's former private copy of the same pipeline.
//!
//! Two behaviors are adopted from net-guard beyond legacy parity, both
//! strictly tightening and controller-ruled safe:
//!
//! - Literal or resolved unspecified (`0.0.0.0`), broadcast, and multicast
//!   addresses are now denied. No legitimate media host is `0.0.0.0` or a
//!   multicast address.
//! - Outbound env/system proxies are disabled (`.no_proxy()`, applied by
//!   `pinned_client`). A proxy would receive the unpinned hostname and
//!   re-resolve it itself, bypassing the address pin below (security
//!   finding F-1).
//!
//! One structural delta: net-guard's `parse_and_vet_url` restricts `https`
//! to port 443 and enforces userinfo/fragment rejection structurally
//! (rather than the crate's former substring scan for `@`/`#`). Both are
//! stricter than the legacy checks; no existing test exercises a non-443
//! `https` download URL or a query string containing `@`/`#`, so this does
//! not change observable behavior for any covered caller-supplied URL.

use std::net::IpAddr;

use finstack_ai_kernel::ErrorCategory;
use finstack_ai_net_guard::{
    NetGuardError, SystemResolver, UrlPolicy, VettedUrl, is_forbidden_destination,
    is_loopback_host, parse_and_vet_url, pinned_client, read_body_bounded, resolve_and_pin,
};
use finstack_ai_runtime::{ToolCallContext, ToolError};

use crate::http::{
    REQUEST_TIMEOUT, deadline_elapsed, endpoint_rejected, invalid_arguments, timeout_error,
    tool_error, wait_deadline,
};

/// Sent when fetching caller-supplied audio, which is an arbitrary host
/// rather than `OpenRouter`.
const DOWNLOAD_USER_AGENT: &str = concat!(
    "finstack-ai-tools-openrouter-media/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/jeickmeier/finstack-ai)"
);

pub(crate) const MAX_AUDIO_DOWNLOAD_BYTES: usize = 25 * 1_048_576;

fn download_url_policy(endpoint_is_loopback: bool) -> UrlPolicy {
    UrlPolicy {
        allow_loopback_http: endpoint_is_loopback,
    }
}

/// Map a net-guard vetting/resolution failure onto the crate's stable
/// `openrouter_media_invalid_arguments` code. All three variants
/// (malformed/forbidden URL, blocked destination, DNS failure) were
/// argument-validation failures under the crate's former private checks.
fn invalid_download_url() -> ToolError {
    invalid_arguments("openrouter media audio_url is not allowed")
}

/// Map every other net-guard failure (client construction, transport,
/// oversize body) onto the crate's existing transport/limit codes.
#[allow(clippy::needless_pass_by_value)] // used as a `map_err` function pointer
fn map_net_guard_error(error: NetGuardError) -> ToolError {
    match error {
        NetGuardError::InvalidUrl { .. }
        | NetGuardError::DestinationBlocked { .. }
        | NetGuardError::ResolutionFailed => invalid_download_url(),
        NetGuardError::ClientBuildFailed | NetGuardError::TransportFailed => tool_error(
            crate::config::OPENROUTER_MEDIA_TRANSPORT_FAILED,
            ErrorCategory::Tool,
            "openrouter media audio download failed",
        ),
        NetGuardError::LimitExceeded => tool_error(
            crate::config::OPENROUTER_MEDIA_LIMIT_EXCEEDED,
            ErrorCategory::Limit,
            "openrouter media response exceeds the configured byte limit",
        ),
    }
}

/// True when a literal address (or the bare string `localhost`) violates
/// destination policy. `resolve_and_pin` performs the equivalent check for
/// resolved and literal hostnames alike, but only after an async DNS
/// resolution step and only using `VettedUrl::is_loopback` (which net-guard
/// scopes to the `http` scheme). The crate's former private checks applied
/// this synchronously, and to *any* scheme, so `validate_download_url`
/// keeps a synchronous literal check here for parity: the loopback
/// allowance is computed from the endpoint policy and the URL's own host,
/// not from the scheme.
fn reject_literal_destination(
    vetted: &VettedUrl,
    endpoint_is_loopback: bool,
) -> Result<(), ToolError> {
    let allow_loopback = endpoint_is_loopback && is_loopback_host(&vetted.host);
    let bare_host = vetted
        .host
        .trim_start_matches('[')
        .trim_end_matches(']');
    if let Ok(addr) = bare_host.parse::<IpAddr>() {
        let canonical = addr.to_canonical();
        let blocked = !(allow_loopback && canonical.is_loopback()) && is_forbidden_destination(addr);
        if blocked {
            return Err(invalid_download_url());
        }
    } else if !allow_loopback && vetted.host.eq_ignore_ascii_case("localhost") {
        return Err(invalid_download_url());
    }
    Ok(())
}

pub(crate) fn validate_download_url(
    value: &str,
    endpoint_is_loopback: bool,
) -> Result<(), ToolError> {
    let vetted = parse_and_vet_url(value, &download_url_policy(endpoint_is_loopback))
        .map_err(|_| invalid_download_url())?;
    reject_literal_destination(&vetted, endpoint_is_loopback)
}

pub(crate) async fn download_bytes(
    url: &str,
    endpoint_is_loopback: bool,
    ctx: &ToolCallContext,
    cap: usize,
) -> Result<Vec<u8>, ToolError> {
    if ctx.run.cancellation.is_cancelled() || deadline_elapsed(ctx.run.deadline) {
        return Err(timeout_error());
    }
    let policy = download_url_policy(endpoint_is_loopback);
    let vetted = parse_and_vet_url(url, &policy).map_err(map_net_guard_error)?;
    reject_literal_destination(&vetted, endpoint_is_loopback)?;
    let addr = resolve_and_pin(&vetted, &SystemResolver)
        .await
        .map_err(map_net_guard_error)?;
    let client = pinned_client(&vetted, addr, REQUEST_TIMEOUT).map_err(map_net_guard_error)?;
    // Identify the client: hosts serving public media commonly answer an
    // anonymous request with 403 rather than the file.
    let send = client
        .get(vetted.url.as_str())
        .header(reqwest::header::USER_AGENT, DOWNLOAD_USER_AGENT)
        .send();
    let response = tokio::select! {
        () = ctx.run.cancellation.cancelled() => return Err(timeout_error()),
        () = wait_deadline(ctx.run.deadline) => return Err(timeout_error()),
        result = send => result.map_err(|_| {
            tool_error(
                crate::config::OPENROUTER_MEDIA_TRANSPORT_FAILED,
                ErrorCategory::Tool,
                "openrouter media audio download failed",
            )
        })?,
    };
    let status = response.status();
    if !status.is_success() {
        return Err(endpoint_rejected("audio host", status, response).await);
    }
    read_body_bounded(response, cap)
        .await
        .map_err(map_net_guard_error)
}
