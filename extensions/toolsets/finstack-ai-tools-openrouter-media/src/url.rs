//! Caller-supplied download URL validation and address-pinned fetches.
//!
//! Policy: a model-supplied `audio_url` must be `https` (any port), carry no
//! userinfo or fragment, and must not resolve to a loopback, private,
//! link-local, or otherwise forbidden destination; plaintext `http` to
//! loopback is admitted only in this crate's own test build, never in
//! production, regardless of where the toolset endpoint points.

use finstack_ai_kernel::ErrorCategory;
use finstack_ai_net_guard::{
    BodyReadInterrupt, NetGuardError, SystemResolver, UrlPolicy, VettedUrl, parse_and_vet_url,
    pinned_client, read_body_bounded_interruptible, reject_literal_destination, resolve_and_pin,
};
use finstack_ai_runtime::ports::tool::{ToolCallContext, ToolError};

use crate::http::{
    REQUEST_TIMEOUT, check_interrupted, endpoint_rejected, invalid_arguments, timeout_error,
    tool_error,
};

/// Sent when fetching caller-supplied audio, which is an arbitrary host
/// rather than `OpenRouter`.
const DOWNLOAD_USER_AGENT: &str = concat!(
    "finstack-ai-tools-openrouter-media/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/jeickmeier/finstack-ai)"
);

pub(crate) const MAX_AUDIO_DOWNLOAD_BYTES: usize = 25 * 1_048_576;

/// The vetting policy for a model-supplied download URL.
///
/// `allow_loopback_http` is a compile-time property of the build, not a
/// runtime property of the configured endpoint: scripted fixtures in this
/// crate's tests serve audio from a loopback listener, but a production
/// build never fetches plaintext audio even when the toolset itself is
/// pointed at a loopback endpoint.
pub(crate) fn download_url_policy(allow_loopback_fixtures: bool) -> UrlPolicy {
    UrlPolicy {
        allow_loopback_http: allow_loopback_fixtures,
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

/// Vet a caller-supplied download URL before any network activity.
pub(crate) fn validate_download_url(value: &str) -> Result<VettedUrl, ToolError> {
    validate_download_url_with(value, download_url_policy(cfg!(test)))
}

/// [`validate_download_url`] under an explicit policy, so tests can assert
/// the production policy from inside the test build.
pub(crate) fn validate_download_url_with(
    value: &str,
    policy: UrlPolicy,
) -> Result<VettedUrl, ToolError> {
    let vetted = parse_and_vet_url(value, &policy).map_err(|_| invalid_download_url())?;
    reject_literal_destination(&vetted, &policy).map_err(|_| invalid_download_url())?;
    Ok(vetted)
}

pub(crate) async fn download_bytes(
    url: &str,
    ctx: &ToolCallContext,
    cap: usize,
) -> Result<Vec<u8>, ToolError> {
    let vetted = validate_download_url(url)?;
    check_interrupted(ctx)?;
    let resolve = resolve_and_pin(&vetted, &SystemResolver);
    let addr = tokio::select! {
        () = ctx.run.interrupted() => return Err(timeout_error()),
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
        () = ctx.run.interrupted() => return Err(timeout_error()),
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
            ctx.run.wait_deadline(),
        ),
    )
    .await
    .map_err(map_net_guard_error)
}

#[cfg(test)]
mod tests {
    use super::{download_url_policy, validate_download_url, validate_download_url_with};

    #[test]
    fn download_url_plaintext_loopback_is_rejected_in_production_policy() {
        // The test build opts loopback fixtures in; production never does.
        assert_eq!(
            download_url_policy(cfg!(test)).allow_loopback_http,
            cfg!(test)
        );
        let production = download_url_policy(false);
        assert!(!production.allow_loopback_http);
        assert!(production.allow_nonstandard_https_port);
        validate_download_url_with("http://127.0.0.1/a.mp3", production)
            .expect_err("production policy must reject plaintext loopback audio");
        validate_download_url_with("http://localhost/a.mp3", production)
            .expect_err("production policy must reject plaintext localhost audio");
        // An off-loopback plaintext URL is rejected under both policies.
        validate_download_url("http://8.8.8.8/a.mp3").expect_err("plaintext off loopback");
    }
}
