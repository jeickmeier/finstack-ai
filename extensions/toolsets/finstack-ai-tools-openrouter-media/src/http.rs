//! Shared `OpenRouter` media HTTP helpers (`send_json` / `send_bytes`).

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
pub(crate) use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use finstack_ai_kernel::{ArtifactRef, ErrorCategory, Metadata, Sensitivity, Timestamp};
use finstack_ai_net_guard::{BodyReadInterrupt, NetGuardError, read_body_bounded_interruptible};
use finstack_ai_runtime::Bytes;
use finstack_ai_runtime::artifact::{
    ArtifactMetadata, ArtifactScope, ArtifactStore, stage_required_artifact,
};
use finstack_ai_runtime::ports::tool::{ToolCallContext, ToolError};
use futures_util::StreamExt;
use reqwest::header::HeaderValue;
use serde::Deserialize;

use crate::config::{
    MAX_RESULT_BYTES_CEILING, OPENROUTER_MEDIA_LIMIT_EXCEEDED, OPENROUTER_MEDIA_TIMEOUT,
    OPENROUTER_MEDIA_TRANSPORT_FAILED,
};

pub(crate) const REQUEST_TIMEOUT: Duration = Duration::from_mins(2);
/// Bytes of an error response body read before giving up on a reason.
const ERROR_BODY_CAP: usize = 4096;
/// Characters of endpoint reason kept in a tool-error message.
const ERROR_DETAIL_CHARS: usize = 300;

#[cfg(not(test))]
pub(crate) const POLL_INTERVAL: Duration = Duration::from_secs(5);
#[cfg(test)]
pub(crate) const POLL_INTERVAL: Duration = Duration::from_millis(5);

/// Exact standard-base64 length of `byte_length` raw bytes (`4 * n.div_ceil(3)`).
pub(crate) fn base64_encoded_len(byte_length: usize) -> usize {
    byte_length.div_ceil(3).saturating_mul(4)
}

pub(crate) struct DeliveredMedia {
    pub(crate) value: serde_json::Value,
    pub(crate) artifact: Option<ArtifactRef>,
}

/// Max raw bytes whose standard-base64 encoding still fits `max_result_bytes`.
fn max_raw_bytes_for_base64_cap(max_result_bytes: usize) -> usize {
    (max_result_bytes / 4).saturating_mul(3)
}

/// HTTP body cap when inlining generated media. Artifact staging may read up
/// to [`MAX_RESULT_BYTES_CEILING`]; inline results cap at `max_result_bytes`
/// (or the raw-byte inverse when the body will later be base64-encoded).
pub(crate) fn inline_http_read_cap(
    max_result_bytes: usize,
    store_attached: bool,
    body_is_raw_bytes: bool,
) -> usize {
    if store_attached {
        MAX_RESULT_BYTES_CEILING
    } else if body_is_raw_bytes {
        max_raw_bytes_for_base64_cap(max_result_bytes)
    } else {
        max_result_bytes
    }
}

pub(crate) fn tool_error(
    code: &'static str,
    category: ErrorCategory,
    message: &'static str,
) -> ToolError {
    ToolError::try_new(code, category, false, message, Metadata::empty()).unwrap_or_else(Into::into)
}

pub(crate) fn invalid_arguments(message: &'static str) -> ToolError {
    tool_error(
        crate::config::OPENROUTER_MEDIA_INVALID_ARGUMENTS,
        ErrorCategory::Validation,
        message,
    )
}

pub(crate) fn parse_arguments<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T, ToolError> {
    serde_json::from_slice(bytes)
        .map_err(|_| invalid_arguments("openrouter media arguments are invalid"))
}

/// Hand generated media back to the model without inlining the bytes.
///
/// Generated audio and images are hundreds of kilobytes that a model cannot
/// read and must not have to carry; when a store is configured the bytes are
/// staged and only the reference travels in the result. Without a store the
/// payload is inlined as base64, still bounded by `max_result_bytes`.
pub(crate) async fn deliver_media(
    bytes: Vec<u8>,
    media_type: &str,
    name: &'static str,
    store: Option<&Arc<dyn ArtifactStore>>,
    ctx: &ToolCallContext,
    max_result_bytes: usize,
) -> Result<DeliveredMedia, ToolError> {
    let byte_length = bytes.len();
    let Some(store) = store else {
        if base64_encoded_len(byte_length) > max_result_bytes {
            return Err(tool_error(
                OPENROUTER_MEDIA_LIMIT_EXCEEDED,
                ErrorCategory::Limit,
                "openrouter media result exceeds the configured byte limit",
            ));
        }
        return Ok(DeliveredMedia {
            value: serde_json::json!({
                "b64_data": BASE64_STANDARD.encode(bytes),
                "media_type": media_type,
                "byte_length": byte_length,
            }),
            artifact: None,
        });
    };
    let artifact = stage_required_artifact(
        store.as_ref(),
        ArtifactScope {
            tenant_scope: Arc::clone(&ctx.run.locator.tenant_scope),
            session_id: ctx.run.locator.session_id,
            run_id: Some(ctx.run.locator.run_id),
            sensitivity: Sensitivity::Internal,
        },
        Bytes::from(bytes),
        ArtifactMetadata {
            kind: Arc::from("tool-output"),
            media_type: Arc::from(media_type),
            name: Some(Arc::from(name)),
            attributes: Metadata::empty(),
        },
    )
    .await
    .map_err(|_| {
        tool_error(
            OPENROUTER_MEDIA_LIMIT_EXCEEDED,
            ErrorCategory::Tool,
            "openrouter media artifact staging failed",
        )
    })?;
    Ok(DeliveredMedia {
        value: serde_json::json!({
            "artifact": artifact,
            "media_type": media_type,
            "byte_length": byte_length,
        }),
        artifact: Some(artifact),
    })
}

#[allow(clippy::too_many_arguments)]
async fn dispatch(
    client: &reqwest::Client,
    authorization: &HeaderValue,
    referer: Option<&str>,
    title: Option<&str>,
    method: reqwest::Method,
    url: &str,
    body: Option<&serde_json::Value>,
    ctx: &ToolCallContext,
) -> Result<reqwest::Response, ToolError> {
    if ctx.run.cancellation.is_cancelled() || deadline_elapsed(ctx.run.deadline) {
        return Err(timeout_error());
    }
    let mut request = client
        .request(method, url)
        .header(reqwest::header::AUTHORIZATION, authorization.clone());
    if let Some(referer) = referer {
        request = request.header("HTTP-Referer", referer);
    }
    if let Some(title) = title {
        request = request.header("X-Title", title);
    }
    if let Some(body) = body {
        request = request
            .header("Content-Type", "application/json")
            .json(body);
    }
    let send = request.send();
    let response = tokio::select! {
        () = ctx.run.cancellation.cancelled() => return Err(timeout_error()),
        () = wait_deadline(ctx.run.deadline) => return Err(timeout_error()),
        result = send => result.map_err(|_| {
            tool_error(
                OPENROUTER_MEDIA_TRANSPORT_FAILED,
                ErrorCategory::Tool,
                "openrouter media request failed",
            )
        })?,
    };
    let status = response.status();
    if !status.is_success() {
        return Err(endpoint_rejected("endpoint", status, response, ctx).await);
    }
    Ok(response)
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn send_bytes(
    client: &reqwest::Client,
    authorization: &HeaderValue,
    referer: Option<&str>,
    title: Option<&str>,
    method: reqwest::Method,
    url: &str,
    body: Option<&serde_json::Value>,
    ctx: &ToolCallContext,
    cap: usize,
) -> Result<(Vec<u8>, Option<String>), ToolError> {
    let response = dispatch(
        client,
        authorization,
        referer,
        title,
        method,
        url,
        body,
        ctx,
    )
    .await?;
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let bytes = fetch_bytes_bounded(response, cap, ctx).await?;
    Ok((bytes, content_type))
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn send_json<T: for<'de> Deserialize<'de>>(
    client: &reqwest::Client,
    authorization: &HeaderValue,
    referer: Option<&str>,
    title: Option<&str>,
    method: reqwest::Method,
    url: &str,
    body: Option<&serde_json::Value>,
    ctx: &ToolCallContext,
    cap: usize,
) -> Result<T, ToolError> {
    let response = dispatch(
        client,
        authorization,
        referer,
        title,
        method,
        url,
        body,
        ctx,
    )
    .await?;
    read_bounded_json(response, cap, ctx).await
}

pub(crate) async fn fetch_bytes_bounded(
    response: reqwest::Response,
    cap: usize,
    ctx: &ToolCallContext,
) -> Result<Vec<u8>, ToolError> {
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
    .map_err(|error| match error {
        NetGuardError::Cancelled | NetGuardError::DeadlineExceeded => timeout_error(),
        NetGuardError::LimitExceeded => tool_error(
            OPENROUTER_MEDIA_LIMIT_EXCEEDED,
            ErrorCategory::Limit,
            "openrouter media response exceeds the configured byte limit",
        ),
        _ => tool_error(
            OPENROUTER_MEDIA_TRANSPORT_FAILED,
            ErrorCategory::Tool,
            "openrouter media response is invalid",
        ),
    })
}

async fn read_bounded_json<T: for<'de> Deserialize<'de>>(
    response: reqwest::Response,
    cap: usize,
    ctx: &ToolCallContext,
) -> Result<T, ToolError> {
    let body = fetch_bytes_bounded(response, cap, ctx).await?;
    serde_json::from_slice(&body).map_err(|_| {
        tool_error(
            OPENROUTER_MEDIA_TRANSPORT_FAILED,
            ErrorCategory::Tool,
            "openrouter media response is invalid",
        )
    })
}

pub(crate) fn deadline_elapsed(deadline: Option<Timestamp>) -> bool {
    let Some(deadline) = deadline else {
        return false;
    };
    now_unix_ms() >= deadline.as_unix_ms()
}

pub(crate) async fn wait_deadline(deadline: Option<Timestamp>) {
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

pub(crate) fn timeout_error() -> ToolError {
    tool_error(
        OPENROUTER_MEDIA_TIMEOUT,
        ErrorCategory::Deadline,
        "openrouter media request was cancelled or exceeded its deadline",
    )
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
            format!("openrouter media {what} rejected the request with HTTP {status}: {detail}")
        }
        None => format!("openrouter media {what} rejected the request with HTTP {status}"),
    };
    ToolError::try_new(
        OPENROUTER_MEDIA_TRANSPORT_FAILED,
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
            () = ctx.run.cancellation.cancelled() => break,
            () = wait_deadline(ctx.run.deadline) => break,
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
    // OpenRouter reports failures as {"error":{"message":...}}; anything else
    // is surfaced verbatim so an unexpected shape still reaches the model.
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
    Some(truncate_chars(&detail, ERROR_DETAIL_CHARS))
}

fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let kept: String = text.chars().take(max).collect();
    format!("{kept}...")
}
