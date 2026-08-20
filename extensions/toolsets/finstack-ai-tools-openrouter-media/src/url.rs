//! Caller-supplied download URL validation and address-pinned fetches.

use std::net::{IpAddr, SocketAddr};

use finstack_ai_kernel::ErrorCategory;
use finstack_ai_runtime::{ToolCallContext, ToolError};

use crate::config::is_loopback_host;
use crate::http::{
    REQUEST_TIMEOUT, deadline_elapsed, endpoint_rejected, fetch_bytes_bounded, invalid_arguments,
    timeout_error, tool_error, wait_deadline,
};

/// Sent when fetching caller-supplied audio, which is an arbitrary host
/// rather than `OpenRouter`.
const DOWNLOAD_USER_AGENT: &str = concat!(
    "finstack-ai-tools-openrouter-media/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/jeickmeier/finstack-ai)"
);

pub(crate) const MAX_AUDIO_DOWNLOAD_BYTES: usize = 25 * 1_048_576;

pub(crate) fn validate_download_url(
    value: &str,
    endpoint_is_loopback: bool,
) -> Result<(), ToolError> {
    let parsed = parse_download_url(value, endpoint_is_loopback)?;
    reject_literal_download_host(parsed.host, parsed.allow_loopback)
}

struct ParsedDownloadUrl<'a> {
    host: &'a str,
    port: u16,
    allow_loopback: bool,
}

fn parse_download_url(
    value: &str,
    endpoint_is_loopback: bool,
) -> Result<ParsedDownloadUrl<'_>, ToolError> {
    let Some((scheme, rest)) = value.split_once("://") else {
        return Err(invalid_arguments(
            "openrouter media audio_url must be an http or https URL",
        ));
    };
    // Query strings are allowed: signed download URLs (e.g. presigned S3 or
    // GCS links) are a normal shape for caller-supplied audio_url values.
    if rest.contains('@') || rest.contains('#') {
        return Err(invalid_arguments(
            "openrouter media audio_url contains forbidden components",
        ));
    }
    let https = scheme.eq_ignore_ascii_case("https");
    let http = scheme.eq_ignore_ascii_case("http");
    if !https && !http {
        return Err(invalid_arguments(
            "openrouter media audio_url must be an http or https URL",
        ));
    }
    let default_port = if https { 443 } else { 80 };
    let (host, port) = split_download_host_port(rest, default_port)
        .ok_or_else(|| invalid_arguments("openrouter media audio_url host is missing"))?;
    let allow_loopback = endpoint_is_loopback && is_loopback_host(host);
    if http && !allow_loopback {
        return Err(invalid_arguments(
            "openrouter media audio_url must be https (plaintext HTTP is allowed only for loopback fixtures when the toolset endpoint is also loopback)",
        ));
    }
    Ok(ParsedDownloadUrl {
        host,
        port,
        allow_loopback,
    })
}

fn forbidden_download_host() -> ToolError {
    invalid_arguments("openrouter media audio_url host is not allowed")
}

pub(crate) fn is_forbidden_destination(addr: IpAddr) -> bool {
    match addr.to_canonical() {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
        IpAddr::V6(v6) => v6.is_loopback() || v6.is_unique_local() || v6.is_unicast_link_local(),
    }
}

fn destination_blocked(addr: IpAddr, allow_loopback: bool) -> bool {
    let addr = addr.to_canonical();
    if allow_loopback && addr.is_loopback() {
        return false;
    }
    is_forbidden_destination(addr)
}

fn reject_literal_download_host(host: &str, allow_loopback: bool) -> Result<(), ToolError> {
    if !allow_loopback && host.eq_ignore_ascii_case("localhost") {
        return Err(forbidden_download_host());
    }
    if let Ok(addr) = host.parse::<IpAddr>()
        && destination_blocked(addr, allow_loopback)
    {
        return Err(forbidden_download_host());
    }
    Ok(())
}

fn split_download_host_port(rest: &str, default_port: u16) -> Option<(&str, u16)> {
    if let Some(after) = rest.strip_prefix('[') {
        let (host, tail) = after.split_once(']')?;
        if host.is_empty() {
            return None;
        }
        let port = match tail.strip_prefix(':') {
            Some(port_and_path) => {
                let port_str = port_and_path.split(['/', '?']).next().unwrap_or("");
                if port_str.is_empty() {
                    default_port
                } else {
                    port_str.parse().ok()?
                }
            }
            None => default_port,
        };
        return Some((host, port));
    }
    let authority = rest
        .split(['/', '?'])
        .next()
        .filter(|part| !part.is_empty())?;
    if authority.parse::<IpAddr>().is_ok() {
        return Some((authority, default_port));
    }
    match authority.rsplit_once(':') {
        Some((host, port_str)) if !host.is_empty() => Some((host, port_str.parse().ok()?)),
        _ => Some((authority, default_port)),
    }
}

pub(crate) fn select_vetted_download_addr(
    addrs: impl IntoIterator<Item = SocketAddr>,
    allow_loopback: bool,
) -> Result<SocketAddr, ToolError> {
    let mut chosen = None;
    for addr in addrs {
        if destination_blocked(addr.ip(), allow_loopback) {
            return Err(forbidden_download_host());
        }
        if chosen.is_none() {
            chosen = Some(addr);
        }
    }
    chosen.ok_or_else(forbidden_download_host)
}

pub(crate) async fn resolve_download_target(
    url: &str,
    endpoint_is_loopback: bool,
) -> Result<(&str, SocketAddr), ToolError> {
    let parsed = parse_download_url(url, endpoint_is_loopback)?;
    reject_literal_download_host(parsed.host, parsed.allow_loopback)?;
    if let Ok(ip) = parsed.host.parse::<IpAddr>() {
        return Ok((parsed.host, SocketAddr::new(ip, parsed.port)));
    }
    let resolved = tokio::net::lookup_host((parsed.host, parsed.port))
        .await
        .map_err(|_| invalid_arguments("openrouter media audio_url host could not be resolved"))?;
    let vetted = select_vetted_download_addr(resolved, parsed.allow_loopback)?;
    Ok((parsed.host, vetted))
}

fn pinned_download_client(host: &str, vetted: SocketAddr) -> Result<reqwest::Client, ToolError> {
    reqwest::Client::builder()
        .http1_only()
        .timeout(REQUEST_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .resolve(host, vetted)
        .build()
        .map_err(|_| {
            tool_error(
                crate::config::OPENROUTER_MEDIA_TRANSPORT_FAILED,
                ErrorCategory::Tool,
                "openrouter media audio download failed",
            )
        })
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
    let (host, vetted) = resolve_download_target(url, endpoint_is_loopback).await?;
    let client = pinned_download_client(host, vetted)?;
    // Identify the client: hosts serving public media commonly answer an
    // anonymous request with 403 rather than the file.
    let send = client
        .get(url)
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
    fetch_bytes_bounded(response, cap).await
}
