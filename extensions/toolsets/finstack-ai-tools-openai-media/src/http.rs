use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use finstack_ai_kernel::{ErrorCategory, Timestamp};
use finstack_ai_net_guard::{
    BodyReadInterrupt, NetGuardError, SystemResolver, UrlPolicy, VettedUrl, parse_and_vet_url,
    pinned_client, read_body_bounded_interruptible, reject_literal_destination, resolve_and_pin,
};
use finstack_ai_runtime::artifact::ArtifactStore;
use finstack_ai_runtime::ports::tool::{ToolCallContext, ToolError};
use serde::Deserialize;

use crate::{
    OPENAI_MEDIA_LIMIT_EXCEEDED, OPENAI_MEDIA_TIMEOUT, OPENAI_MEDIA_TRANSPORT_FAILED,
    invalid_arguments, tool_error,
};

pub(crate) const REQUEST_TIMEOUT: Duration = Duration::from_mins(2);

/// Validated `OpenAI` route plus the delivery settings every handler reads.
#[derive(Clone)]
pub(crate) struct Route {
    pub(crate) client: reqwest::Client,
    pub(crate) api_key: String,
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
    let request = request.header("Authorization", format!("Bearer {}", route.api_key));
    let response = tokio::select! {
        () = ctx.run.cancellation.cancelled() => return Err(timeout_error()),
        () = wait_deadline(ctx.run.deadline) => return Err(timeout_error()),
        result = request.send() => result.map_err(|_| transport_error("openai media request failed"))?,
    };
    if !response.status().is_success() {
        return Err(transport_error(
            "openai media endpoint rejected the request",
        ));
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
        () = ctx.run.cancellation.cancelled() => return Err(timeout_error()),
        () = wait_deadline(ctx.run.deadline) => return Err(timeout_error()),
        result = resolve_and_pin(&vetted, &SystemResolver) => result.map_err(map_net_guard_error)?,
    };
    let client = pinned_client(&vetted, addr, REQUEST_TIMEOUT).map_err(map_net_guard_error)?;
    let response = tokio::select! {
        () = ctx.run.cancellation.cancelled() => return Err(timeout_error()),
        () = wait_deadline(ctx.run.deadline) => return Err(timeout_error()),
        result = client.get(vetted.url.as_str()).send() => {
            result.map_err(|_| transport_error("openai media audio download failed"))?
        },
    };
    if !response.status().is_success() {
        return Err(transport_error(
            "openai media audio host rejected the request",
        ));
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
            wait_deadline(ctx.run.deadline),
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

pub(crate) fn check_interrupted(ctx: &ToolCallContext) -> Result<(), ToolError> {
    if ctx.run.cancellation.is_cancelled() || deadline_elapsed(ctx.run.deadline) {
        return Err(timeout_error());
    }
    Ok(())
}

fn deadline_elapsed(deadline: Option<Timestamp>) -> bool {
    deadline.is_some_and(|deadline| now_unix_ms() >= deadline.as_unix_ms())
}

async fn wait_deadline(deadline: Option<Timestamp>) {
    let Some(deadline) = deadline else {
        std::future::pending::<()>().await;
        return;
    };
    let remaining = deadline.as_unix_ms().saturating_sub(now_unix_ms());
    let millis = u64::try_from(remaining).unwrap_or(0);
    if millis != 0 {
        tokio::time::sleep(Duration::from_millis(millis)).await;
    }
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
        OPENAI_MEDIA_TIMEOUT,
        ErrorCategory::Deadline,
        "openai media request was cancelled or exceeded its deadline",
    )
}

fn transport_error(message: &'static str) -> ToolError {
    tool_error(OPENAI_MEDIA_TRANSPORT_FAILED, ErrorCategory::Tool, message)
}
