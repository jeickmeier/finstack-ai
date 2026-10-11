use std::sync::Arc;
use std::time::Duration;

use finstack_ai_kernel::{ErrorCategory, Metadata};
use finstack_ai_net_guard::{
    BodyReadInterrupt, NetGuardError, SystemResolver, UrlPolicy, VettedUrl, parse_and_vet_url,
    pinned_client, read_body_bounded_interruptible, reject_literal_destination, resolve_and_pin,
};
use finstack_ai_runtime::artifact::ArtifactStore;
use finstack_ai_runtime::ids::{Clock, SystemClock};
use finstack_ai_runtime::ports::tool::{ToolCallContext, ToolError};
use futures_util::StreamExt;
use reqwest::header::HeaderValue;
use serde::Deserialize;

use crate::{
    OPENAI_MEDIA_LIMIT_EXCEEDED, OPENAI_MEDIA_TIMEOUT, OPENAI_MEDIA_TRANSPORT_FAILED,
    invalid_arguments, tool_error,
};

pub(crate) const REQUEST_TIMEOUT: Duration = Duration::from_mins(2);
/// Bytes of an error response body read before giving up on a reason.
const ERROR_BODY_CAP: usize = 4096;
/// Characters of endpoint reason kept in a tool-error message.
const ERROR_DETAIL_CHARS: usize = 300;

/// Validated `OpenAI` route plus the delivery settings every handler reads.
#[derive(Clone)]
pub(crate) struct Route {
    pub(crate) client: reqwest::Client,
    /// `Authorization: Bearer ...` built once at construction and marked
    /// sensitive, so the raw key is neither re-formatted nor logged per call.
    pub(crate) authorization: HeaderValue,
    pub(crate) endpoint: String,
    pub(crate) max_result_bytes: usize,
    pub(crate) store: Option<Arc<dyn ArtifactStore>>,
}

pub(crate) async fn send_json<T: for<'de> Deserialize<'de>>(
    route: &Route,
    url: &str,
    body: &serde_json::Value,
    ctx: &ToolCallContext,
    cap: usize,
) -> Result<T, ToolError> {
    let request = route
        .client
        .post(url)
        .header("Content-Type", "application/json")
        .json(body);
    let response = send(route, request, ctx).await?;
    read_bounded_json(response, cap, ctx).await
}

pub(crate) async fn send_bytes(
    route: &Route,
    url: &str,
    body: &serde_json::Value,
    ctx: &ToolCallContext,
    cap: usize,
) -> Result<(Vec<u8>, Option<String>), ToolError> {
    let request = route
        .client
        .post(url)
        .header("Content-Type", "application/json")
        .json(body);
    let response = send(route, request, ctx).await?;
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let bytes = fetch_bytes_bounded(response, cap, ctx).await?;
    Ok((bytes, content_type))
}

/// Authorize and send one request to the configured endpoint, racing it
/// against cancellation and the run deadline.
pub(crate) async fn send(
    route: &Route,
    request: reqwest::RequestBuilder,
    ctx: &ToolCallContext,
) -> Result<reqwest::Response, ToolError> {
    check_interrupted(ctx)?;
    let request = request.header(reqwest::header::AUTHORIZATION, route.authorization.clone());
    let response = tokio::select! {
        () = ctx.run.interrupted() => return Err(timeout_error()),
        result = request.send() => result.map_err(|_| transport_error("openai media request failed"))?,
    };
    let status = response.status();
    if !status.is_success() {
        return Err(endpoint_rejected("endpoint", status, response, ctx).await);
    }
    Ok(response)
}

pub(crate) async fn download_bytes(
    url: &str,
    ctx: &ToolCallContext,
    cap: usize,
) -> Result<Vec<u8>, ToolError> {
    let vetted = validate_download_url(url)?;
    check_interrupted(ctx)?;
    let addr = tokio::select! {
        () = ctx.run.interrupted() => return Err(timeout_error()),
        result = resolve_and_pin(&vetted, &SystemResolver) => result.map_err(map_net_guard_error)?,
    };
    let client = pinned_client(&vetted, addr, REQUEST_TIMEOUT).map_err(map_net_guard_error)?;
    let response = tokio::select! {
        () = ctx.run.interrupted() => return Err(timeout_error()),
        result = client.get(vetted.url.as_str()).send() => {
            result.map_err(|_| transport_error("openai media audio download failed"))?
        },
    };
    let status = response.status();
    if !status.is_success() {
        return Err(endpoint_rejected("audio host", status, response, ctx).await);
    }
    read_interruptible(response, cap, ctx)
        .await
        .map_err(map_net_guard_error)
}

/// Vet a caller-supplied download URL before any network activity.
pub(crate) fn validate_download_url(value: &str) -> Result<VettedUrl, ToolError> {
    let policy = UrlPolicy {
        // Scripted fixtures serve audio from a loopback listener.
        allow_loopback_http: cfg!(test),
        allow_nonstandard_https_port: true,
    };
    let vetted = parse_and_vet_url(value, &policy).map_err(|_| invalid_download_url())?;
    reject_literal_destination(&vetted, &policy).map_err(|_| invalid_download_url())?;
    Ok(vetted)
}

fn invalid_download_url() -> ToolError {
    invalid_arguments("openai media audio_url is not allowed")
}

#[allow(clippy::needless_pass_by_value, reason = "used as a map_err function")]
fn map_net_guard_error(error: NetGuardError) -> ToolError {
    match error {
        NetGuardError::InvalidUrl { .. }
        | NetGuardError::DestinationBlocked { .. }
        | NetGuardError::ResolutionFailed => invalid_download_url(),
        NetGuardError::ClientBuildFailed | NetGuardError::TransportFailed => {
            transport_error("openai media audio download failed")
        }
        NetGuardError::LimitExceeded => tool_error(
            OPENAI_MEDIA_LIMIT_EXCEEDED,
            ErrorCategory::Limit,
            "openai media response exceeds the configured byte limit",
        ),
        NetGuardError::Cancelled | NetGuardError::DeadlineExceeded => timeout_error(),
    }
}

async fn fetch_bytes_bounded(
    response: reqwest::Response,
    cap: usize,
    ctx: &ToolCallContext,
) -> Result<Vec<u8>, ToolError> {
    read_interruptible(response, cap, ctx)
        .await
        .map_err(|error| match error {
            NetGuardError::Cancelled | NetGuardError::DeadlineExceeded => timeout_error(),
            NetGuardError::LimitExceeded => tool_error(
                OPENAI_MEDIA_LIMIT_EXCEEDED,
                ErrorCategory::Limit,
                "openai media response exceeds the configured byte limit",
            ),
            _ => transport_error("openai media response is invalid"),
        })
}

async fn read_interruptible(
    response: reqwest::Response,
    cap: usize,
    ctx: &ToolCallContext,
) -> Result<Vec<u8>, NetGuardError> {
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
}

pub(crate) async fn read_bounded_json<T: for<'de> Deserialize<'de>>(
    response: reqwest::Response,
    cap: usize,
    ctx: &ToolCallContext,
) -> Result<T, ToolError> {
    let body = fetch_bytes_bounded(response, cap, ctx).await?;
    serde_json::from_slice(&body).map_err(|_| transport_error("openai media response is invalid"))
}

/// Synchronous cancellation/deadline gate, evaluated before any network
/// activity so an already-interrupted call never reaches the wire.
///
/// A clock that cannot produce a timestamp is treated as an elapsed
/// deadline (fail closed), matching `RunCallContext::wait_deadline`.
pub(crate) fn check_interrupted(ctx: &ToolCallContext) -> Result<(), ToolError> {
    let interrupted = SystemClock
        .now()
        .map_or(true, |now| ctx.run.is_interrupted(now));
    if interrupted {
        return Err(timeout_error());
    }
    Ok(())
}

fn timeout_error() -> ToolError {
    tool_error(
        OPENAI_MEDIA_TIMEOUT,
        ErrorCategory::Deadline,
        "openai media request was cancelled or exceeded its deadline",
    )
}

fn transport_error(message: &'static str) -> ToolError {
    tool_error(OPENAI_MEDIA_TRANSPORT_FAILED, ErrorCategory::Tool, message)
}

/// Reject an unsuccessful media response, naming the status and the endpoint's
/// own reason.
///
/// The message is the model's only self-correction signal. A status alone does
/// not say which argument was wrong, so the model reissues the identical call;
/// because these tools are approval-gated, every retry also re-prompts the
/// caller, and the run burns its cycles without progressing.
pub(crate) async fn endpoint_rejected(
    what: &'static str,
    status: reqwest::StatusCode,
    response: reqwest::Response,
    ctx: &ToolCallContext,
) -> ToolError {
    let message = match rejection_detail(response, ctx).await {
        Some(detail) => {
            format!("openai media {what} rejected the request with HTTP {status}: {detail}")
        }
        None => format!("openai media {what} rejected the request with HTTP {status}"),
    };
    ToolError::try_new(
        OPENAI_MEDIA_TRANSPORT_FAILED,
        ErrorCategory::Tool,
        false,
        message,
        Metadata::empty(),
    )
    .unwrap_or_else(Into::into)
}

/// Reduce an error response body to one short line.
///
/// Reads at most [`ERROR_BODY_CAP`] bytes so a hostile or malformed endpoint
/// cannot stream an unbounded body into an error message.
async fn rejection_detail(response: reqwest::Response, ctx: &ToolCallContext) -> Option<String> {
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    loop {
        let next = tokio::select! {
            () = ctx.run.interrupted() => break,
            chunk = stream.next() => chunk,
        };
        let Some(Ok(chunk)) = next else {
            break;
        };
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
    // OpenAI reports failures as {"error":{"message":...}}; anything else is
    // surfaced verbatim so an unexpected shape still reaches the model.
    let detail = serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .and_then(|value| {
            value
                .get("error")
                .and_then(|error| error.get("message"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_else(|| text.to_owned());
    let detail = detail.split_whitespace().collect::<Vec<_>>().join(" ");
    if detail.is_empty() {
        return None;
    }
    if detail.chars().count() <= ERROR_DETAIL_CHARS {
        return Some(detail);
    }
    let kept: String = detail.chars().take(ERROR_DETAIL_CHARS).collect();
    Some(format!("{kept}..."))
}
