//! The bounded fetch request flow: cancellation/deadline gate → allowlist
//! match on the parsed host → net-guard vet/pin → GET with headers → bounded
//! read, mapped to the crate's stable error codes.
//!
//! Order and error mapping follow spec §4.4 (request flow) and §4.2 (error
//! codes); see the task brief for the numbered steps this module implements.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use finstack_ai_kernel::{ErrorCategory, Metadata, Timestamp};
use finstack_ai_net_guard::{
    HostResolver, NetGuardError, SystemResolver, UrlPolicy, VettedUrl, parse_and_vet_url,
    pinned_client, read_body_bounded, resolve_and_pin,
};
use finstack_ai_runtime::{ToolCallContext, ToolError};
use futures_util::StreamExt;
use reqwest::header::{HeaderName, HeaderValue};

use crate::config::HostPattern;
use crate::toolset::{FetchArguments, FetchMode};
use crate::{
    FETCH_DESTINATION_BLOCKED, FETCH_HOST_NOT_ALLOWLISTED, FETCH_INVALID_ARGUMENTS,
    FETCH_LIMIT_EXCEEDED, FETCH_REDIRECT_DENIED, FETCH_TIMEOUT, FETCH_TRANSPORT_FAILED,
    HttpFetchConfig,
};

/// Bytes of an error response body read before giving up on a reason.
const ERROR_BODY_CAP: usize = 4096;
/// Characters of endpoint reason kept in a tool-error message.
const ERROR_DETAIL_CHARS: usize = 300;

/// Default `User-Agent`, used when [`HttpFetchConfig::user_agent`] is unset.
fn default_user_agent() -> String {
    concat!(
        "finstack-ai-tools-fetch/",
        env!("CARGO_PKG_VERSION"),
        " (+https://github.com/jeickmeier/finstack-ai)"
    )
    .to_owned()
}

/// Bundled, validated pipeline state: parsed allowlist, validated config,
/// and the injectable DNS resolver seam (defaults to [`SystemResolver`]).
pub(crate) struct FetchState {
    pub(crate) config: HttpFetchConfig,
    pub(crate) patterns: Vec<HostPattern>,
    pub(crate) resolver: Arc<dyn HostResolver>,
}

impl FetchState {
    pub(crate) fn new(config: HttpFetchConfig, patterns: Vec<HostPattern>) -> Self {
        Self {
            config,
            patterns,
            resolver: Arc::new(SystemResolver),
        }
    }

    /// Override the DNS resolver seam (test-only scripted resolvers).
    #[cfg(test)]
    pub(crate) fn with_resolver(mut self, resolver: Arc<dyn HostResolver>) -> Self {
        self.resolver = resolver;
        self
    }
}

fn tool_error(code: &'static str, category: ErrorCategory, message: impl Into<String>) -> ToolError {
    ToolError::try_new(code, category, false, message.into(), Metadata::empty())
        .unwrap_or_else(Into::into)
}

fn deadline_elapsed(deadline: Option<Timestamp>) -> bool {
    let Some(deadline) = deadline else {
        return false;
    };
    now_unix_ms() >= deadline.as_unix_ms()
}

async fn wait_deadline(deadline: Option<Timestamp>) {
    let Some(deadline) = deadline else {
        std::future::pending::<()>().await;
        return;
    };
    let remaining = deadline.as_unix_ms().saturating_sub(now_unix_ms());
    let millis = u64::try_from(remaining).unwrap_or(0);
    if millis == 0 {
        return;
    }
    tokio::time::sleep(Duration::from_millis(millis)).await;
}

fn now_unix_ms() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_millis()),
    )
    .unwrap_or(i64::MAX)
}

fn timeout_error() -> ToolError {
    tool_error(
        FETCH_TIMEOUT,
        ErrorCategory::Deadline,
        "http fetch was cancelled or exceeded its deadline",
    )
}

fn map_vet_error(error: &NetGuardError) -> ToolError {
    match error {
        NetGuardError::InvalidUrl { reason } => tool_error(
            FETCH_INVALID_ARGUMENTS,
            ErrorCategory::Validation,
            format!("http fetch url is invalid: {reason}"),
        ),
        NetGuardError::DestinationBlocked { reason } => tool_error(
            FETCH_DESTINATION_BLOCKED,
            ErrorCategory::Validation,
            format!("http fetch destination is blocked: {reason}"),
        ),
        NetGuardError::ResolutionFailed => tool_error(
            FETCH_TRANSPORT_FAILED,
            ErrorCategory::Tool,
            "http fetch could not resolve the destination host",
        ),
        NetGuardError::ClientBuildFailed => tool_error(
            FETCH_TRANSPORT_FAILED,
            ErrorCategory::Tool,
            "http fetch could not build a pinned client",
        ),
        NetGuardError::TransportFailed => tool_error(
            FETCH_TRANSPORT_FAILED,
            ErrorCategory::Tool,
            "http fetch transport failed",
        ),
        NetGuardError::LimitExceeded => tool_error(
            FETCH_LIMIT_EXCEEDED,
            ErrorCategory::Limit,
            "http fetch response exceeds the configured byte limit",
        ),
    }
}

fn host_allowed(vetted: &VettedUrl, config: &HttpFetchConfig, patterns: &[HostPattern]) -> bool {
    if vetted.is_loopback && config.allow_loopback_http {
        return true;
    }
    patterns.iter().any(|pattern| pattern.matches(&vetted.host))
}

/// Reduce a non-2xx response body to one bounded, whitespace-collapsed line.
async fn rejection_detail(response: reqwest::Response) -> Option<String> {
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(Ok(chunk)) = stream.next().await {
        let remaining = ERROR_BODY_CAP.saturating_sub(body.len());
        if remaining == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
    }
    let text = String::from_utf8_lossy(&body);
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let detail = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if detail.is_empty() {
        return None;
    }
    Some(truncate_chars(&detail, ERROR_DETAIL_CHARS))
}

fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let kept: String = text.chars().take(max).collect();
    format!("{kept}...")
}

async fn endpoint_rejected(status: reqwest::StatusCode, response: reqwest::Response) -> ToolError {
    let message = match rejection_detail(response).await {
        Some(detail) => format!("http fetch endpoint rejected the request with HTTP {status}: {detail}"),
        None => format!("http fetch endpoint rejected the request with HTTP {status}"),
    };
    tool_error(FETCH_TRANSPORT_FAILED, ErrorCategory::Tool, message)
}

/// Lowercased Content-Type essence: strip `;` parameters and trim whitespace.
fn media_type_of(response: &reqwest::Response) -> String {
    response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| {
            value
                .split(';')
                .next()
                .unwrap_or(value)
                .trim()
                .to_ascii_lowercase()
        })
        .unwrap_or_default()
}

/// Run the bounded fetch request flow for one validated `http_fetch` call.
///
/// # Errors
///
/// Returns a [`ToolError`] carrying one of the crate's stable `FETCH_*`
/// codes: `FETCH_TIMEOUT` on cancellation/deadline, `FETCH_HOST_NOT_ALLOWLISTED`
/// when the parsed host is not permitted, `FETCH_INVALID_ARGUMENTS` /
/// `FETCH_DESTINATION_BLOCKED` from URL vetting, `FETCH_REDIRECT_DENIED` on a
/// 3xx response (redirect following lands in a later task), `FETCH_TRANSPORT_FAILED`
/// on resolution/client/transport failures or a non-2xx/3xx status, and
/// `FETCH_LIMIT_EXCEEDED` when the body exceeds the effective byte cap.
pub(crate) async fn execute_fetch(
    state: &FetchState,
    ctx: &ToolCallContext,
    args: FetchArguments,
) -> Result<serde_json::Value, ToolError> {
    if ctx.run.cancellation.is_cancelled() || deadline_elapsed(ctx.run.deadline) {
        return Err(timeout_error());
    }

    let policy = UrlPolicy {
        allow_loopback_http: state.config.allow_loopback_http,
    };
    let vetted = parse_and_vet_url(&args.url, &policy).map_err(|e| map_vet_error(&e))?;

    if !host_allowed(&vetted, &state.config, &state.patterns) {
        return Err(tool_error(
            FETCH_HOST_NOT_ALLOWLISTED,
            ErrorCategory::Validation,
            "http fetch host is not on the configured allowlist",
        ));
    }

    let addr = resolve_and_pin(&vetted, state.resolver.as_ref())
        .await
        .map_err(|e| map_vet_error(&e))?;
    let client = pinned_client(&vetted, addr, state.config.request_timeout).map_err(|e| map_vet_error(&e))?;

    let user_agent = state
        .config
        .user_agent
        .clone()
        .unwrap_or_else(default_user_agent);
    let mut request = client
        .get(vetted.url.as_str())
        .header(reqwest::header::USER_AGENT, user_agent);
    if let Some(headers) = state.config.per_host_headers.get(&vetted.host) {
        for (name, value) in headers {
            let name = HeaderName::from_bytes(name.as_bytes()).map_err(|_| {
                tool_error(
                    FETCH_TRANSPORT_FAILED,
                    ErrorCategory::Tool,
                    "http fetch per-host header name is invalid",
                )
            })?;
            let value = HeaderValue::from_str(value).map_err(|_| {
                tool_error(
                    FETCH_TRANSPORT_FAILED,
                    ErrorCategory::Tool,
                    "http fetch per-host header value is invalid",
                )
            })?;
            request = request.header(name, value);
        }
    }

    let send = request.send();
    let response = tokio::select! {
        () = ctx.run.cancellation.cancelled() => return Err(timeout_error()),
        () = wait_deadline(ctx.run.deadline) => return Err(timeout_error()),
        result = send => result.map_err(|_| tool_error(
            FETCH_TRANSPORT_FAILED,
            ErrorCategory::Tool,
            "http fetch transport failed",
        ))?,
    };

    let status = response.status();
    if status.is_redirection() {
        return Err(tool_error(
            FETCH_REDIRECT_DENIED,
            ErrorCategory::Validation,
            format!("http fetch endpoint returned a redirect (HTTP {status}), which this tool does not follow"),
        ));
    }
    if !status.is_success() {
        return Err(endpoint_rejected(status, response).await);
    }

    let media_type = media_type_of(&response);
    let final_url = vetted.url.to_string();
    let effective_cap = state
        .config
        .max_response_bytes
        .min(args.max_bytes.unwrap_or(usize::MAX));
    let body = read_body_bounded(response, effective_cap)
        .await
        .map_err(|e| map_vet_error(&e))?;
    let byte_length = body.len();
    let content = String::from_utf8_lossy(&body).into_owned();

    // `args.mode` will select text/markdown/artifact shaping once Task 9/10
    // land; every mode inlines text for now.
    match args.mode {
        FetchMode::Auto | FetchMode::Text | FetchMode::Markdown | FetchMode::Artifact => {}
    }

    Ok(serde_json::json!({
        "url": args.url,
        "final_url": final_url,
        "status": status.as_u16(),
        "media_type": media_type,
        "byte_length": byte_length,
        "content": content,
    }))
}
