//! Caller-supplied download URL validation and address-pinned fetches.
//!
//! Backed by `finstack-ai-net-guard`'s vetted-egress primitives: URL
//! parsing/vetting (`parse_and_vet_url`), a synchronous literal-destination
//! check (`reject_literal_destination`), DNS resolve-and-pin
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
//! Everything else is behavior-preserving, including a non-standard
//! `https` port: [`download_url_policy`] sets
//! `allow_nonstandard_https_port: true`, restoring the crate's prior
//! any-port behavior (net-guard's own default is 443-only, the right
//! choice for a model-supplied URL, but a presigned download URL against a
//! self-hosted object store on a custom port is a normal shape this crate
//! has always accepted).
//!
//! One deliberate, benign relaxation: the crate's former check rejected
//! `@` anywhere in the URL after the scheme, which also rejected a
//! legitimate query string like `?X-Amz-Credential=key@example` — a `@`
//! after the authority cannot influence host parsing, so that was an
//! over-broad substring scan, not a security boundary. `parse_and_vet_url`
//! instead rejects userinfo and fragment structurally
//! (`Url::username`/`password`/`fragment`), the correct authority-scoped
//! equivalent: an actual `user:pw@host` userinfo component is still
//! rejected, but a `@` legitimately embedded in a query string (as in
//! presigned S3/GCS URLs) is now accepted.

use finstack_ai_kernel::ErrorCategory;
use finstack_ai_net_guard::{
    BodyReadInterrupt, NetGuardError, SystemResolver, UrlPolicy, parse_and_vet_url, pinned_client,
    read_body_bounded_interruptible, reject_literal_destination, resolve_and_pin,
};
use finstack_ai_runtime::ports::tool::{ToolCallContext, ToolError};

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
        allow_nonstandard_https_port: true,
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
        NetGuardError::Cancelled | NetGuardError::DeadlineExceeded => timeout_error(),
    }
}

pub(crate) fn validate_download_url(
    value: &str,
    endpoint_is_loopback: bool,
) -> Result<(), ToolError> {
    let policy = download_url_policy(endpoint_is_loopback);
    let vetted = parse_and_vet_url(value, &policy).map_err(|_| invalid_download_url())?;
    reject_literal_destination(&vetted, &policy).map_err(|_| invalid_download_url())
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
    reject_literal_destination(&vetted, &policy).map_err(map_net_guard_error)?;
    let resolve = resolve_and_pin(&vetted, &SystemResolver);
    let addr = tokio::select! {
        () = ctx.run.cancellation.cancelled() => return Err(timeout_error()),
        () = wait_deadline(ctx.run.deadline) => return Err(timeout_error()),
        result = resolve => result.map_err(map_net_guard_error)?,
    };
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
        return Err(endpoint_rejected("audio host", status, response, ctx).await);
    }
    let cancellation = ctx.run.cancellation.clone();
    read_body_bounded_interruptible(
        response,
        cap,
        BodyReadInterrupt::new(
            async move { cancellation.cancelled().await },
            wait_deadline(ctx.run.deadline),
        ),
    )
    .await
    .map_err(map_net_guard_error)
}
