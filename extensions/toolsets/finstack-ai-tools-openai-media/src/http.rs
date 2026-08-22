use std::time::{Duration, SystemTime, UNIX_EPOCH};

use finstack_ai_kernel::{ErrorCategory, Timestamp};
use finstack_ai_net_guard::{
    BodyReadInterrupt, NetGuardError, SystemResolver, UrlPolicy, parse_and_vet_url, pinned_client,
    read_body_bounded_interruptible, reject_literal_destination, resolve_and_pin,
};
use finstack_ai_runtime::ports::tool::{ToolCallContext, ToolError};
use serde::Deserialize;

use crate::{
    OPENAI_MEDIA_LIMIT_EXCEEDED, OPENAI_MEDIA_TIMEOUT, OPENAI_MEDIA_TRANSPORT_FAILED,
    invalid_arguments, tool_error,
};

pub(crate) const REQUEST_TIMEOUT: Duration = Duration::from_mins(2);

pub(crate) async fn send_json<T: for<'de> Deserialize<'de>>(
    client: &reqwest::Client,
    api_key: &str,
    method: reqwest::Method,
    url: &str,
    body: Option<&serde_json::Value>,
    ctx: &ToolCallContext,
    cap: usize,
) -> Result<T, ToolError> {
    let response = send(client, api_key, method, url, body, ctx).await?;
    read_bounded_json(response, cap, ctx).await
}

pub(crate) async fn send_bytes(
    client: &reqwest::Client,
    api_key: &str,
    method: reqwest::Method,
    url: &str,
    body: Option<&serde_json::Value>,
    ctx: &ToolCallContext,
    cap: usize,
) -> Result<(Vec<u8>, Option<String>), ToolError> {
    let response = send(client, api_key, method, url, body, ctx).await?;
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let bytes = fetch_bytes_bounded(response, cap, ctx).await?;
    Ok((bytes, content_type))
}

async fn send(
    client: &reqwest::Client,
    api_key: &str,
    method: reqwest::Method,
    url: &str,
    body: Option<&serde_json::Value>,
    ctx: &ToolCallContext,
) -> Result<reqwest::Response, ToolError> {
    check_interrupted(ctx)?;
    let mut request = client
        .request(method, url)
        .header("Authorization", format!("Bearer {api_key}"));
    if let Some(body) = body {
        request = request
            .header("Content-Type", "application/json")
            .json(body);
    }
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
    check_interrupted(ctx)?;
    let policy = download_url_policy();
    let vetted = parse_and_vet_url(url, &policy).map_err(map_net_guard_error)?;
    reject_literal_destination(&vetted, &policy).map_err(map_net_guard_error)?;
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

pub(crate) fn validate_download_url(value: &str) -> Result<(), ToolError> {
    let policy = download_url_policy();
    let vetted = parse_and_vet_url(value, &policy).map_err(|_| invalid_download_url())?;
    reject_literal_destination(&vetted, &policy).map_err(|_| invalid_download_url())
}

#[cfg(test)]
fn download_url_policy() -> UrlPolicy {
    UrlPolicy {
        allow_loopback_http: true,
        allow_nonstandard_https_port: true,
    }
}

#[cfg(not(test))]
fn download_url_policy() -> UrlPolicy {
    UrlPolicy {
        allow_loopback_http: false,
        allow_nonstandard_https_port: true,
    }
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

pub(crate) async fn wait_deadline(deadline: Option<Timestamp>) {
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

pub(crate) fn timeout_error() -> ToolError {
    tool_error(
        OPENAI_MEDIA_TIMEOUT,
        ErrorCategory::Deadline,
        "openai media request was cancelled or exceeded its deadline",
    )
}

fn transport_error(message: &'static str) -> ToolError {
    tool_error(OPENAI_MEDIA_TRANSPORT_FAILED, ErrorCategory::Tool, message)
}
