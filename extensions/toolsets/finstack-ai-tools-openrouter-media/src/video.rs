//! `openrouter_generate_video` and `openrouter_get_video` handlers.

use std::time::Instant;

use finstack_ai_runtime::{ToolCallContext, ToolError};
use reqwest::header::HeaderValue;
use serde::Deserialize;

use crate::config::MAX_RESULT_BYTES_CEILING;
use crate::http::{
    POLL_INTERVAL, invalid_arguments, parse_arguments, send_json, timeout_error, wait_deadline,
};

pub(crate) const VIDEO_TOOL_ID: &str = "finstack.tools.openrouter_generate_video";
pub(crate) const VIDEO_TOOL_NAME: &str = "openrouter_generate_video";
pub(crate) const VIDEO_STATUS_TOOL_ID: &str = "finstack.tools.openrouter_get_video";
pub(crate) const VIDEO_STATUS_TOOL_NAME: &str = "openrouter_get_video";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct VideoArguments {
    model: String,
    prompt: String,
    #[serde(default)]
    duration: Option<u32>,
    #[serde(default)]
    resolution: Option<String>,
    #[serde(default)]
    aspect_ratio: Option<String>,
}

#[derive(Deserialize)]
struct VideoSubmitResponse {
    id: String,
    status: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct VideoStatusArguments {
    id: String,
    #[serde(default)]
    wait_seconds: Option<u32>,
}

#[derive(Deserialize)]
struct VideoStatusResponse {
    id: String,
    status: String,
    #[serde(default)]
    unsigned_urls: Vec<String>,
}

pub(crate) async fn handle_video_submit(
    client: &reqwest::Client,
    authorization: &HeaderValue,
    referer: Option<&str>,
    title: Option<&str>,
    endpoint: &str,
    ctx: &ToolCallContext,
    arguments: &[u8],
) -> Result<serde_json::Value, ToolError> {
    let arguments: VideoArguments = parse_arguments(arguments)?;
    if arguments.model.is_empty() || arguments.prompt.is_empty() {
        return Err(invalid_arguments(
            "openrouter media model or prompt is empty",
        ));
    }
    let mut body = serde_json::json!({
        "model": arguments.model,
        "prompt": arguments.prompt,
    });
    if let Some(map) = body.as_object_mut() {
        if let Some(duration) = arguments.duration {
            map.insert("duration".into(), serde_json::Value::from(duration));
        }
        if let Some(resolution) = arguments.resolution {
            map.insert("resolution".into(), serde_json::Value::String(resolution));
        }
        if let Some(aspect_ratio) = arguments.aspect_ratio {
            map.insert(
                "aspect_ratio".into(),
                serde_json::Value::String(aspect_ratio),
            );
        }
    }
    let response: VideoSubmitResponse = send_json(
        client,
        authorization,
        referer,
        title,
        reqwest::Method::POST,
        &format!("{endpoint}/api/v1/videos"),
        Some(&body),
        ctx,
        MAX_RESULT_BYTES_CEILING,
    )
    .await?;
    Ok(serde_json::json!({ "id": response.id, "status": response.status }))
}

pub(crate) async fn handle_video_status(
    client: &reqwest::Client,
    authorization: &HeaderValue,
    referer: Option<&str>,
    title: Option<&str>,
    endpoint: &str,
    ctx: &ToolCallContext,
    arguments: &[u8],
) -> Result<serde_json::Value, ToolError> {
    let arguments: VideoStatusArguments = parse_arguments(arguments)?;
    if arguments.id.is_empty() {
        return Err(invalid_arguments("openrouter media video id is empty"));
    }
    let wait_seconds = arguments.wait_seconds.unwrap_or(0);
    if wait_seconds > 300 {
        return Err(invalid_arguments(
            "openrouter media wait_seconds exceeds 300",
        ));
    }
    let url = format!(
        "{endpoint}/api/v1/videos/{}",
        percent_encode_path_segment(&arguments.id)
    );
    let wait_deadline_local = (wait_seconds > 0)
        .then(|| Instant::now() + std::time::Duration::from_secs(u64::from(wait_seconds)));
    loop {
        let response: VideoStatusResponse = send_json(
            client,
            authorization,
            referer,
            title,
            reqwest::Method::GET,
            &url,
            None,
            ctx,
            MAX_RESULT_BYTES_CEILING,
        )
        .await?;
        let terminal = !matches!(response.status.as_str(), "pending" | "in_progress");
        if terminal || wait_deadline_local.is_none() {
            return Ok(serde_json::json!({
                "id": response.id,
                "status": response.status,
                "urls": response.unsigned_urls,
            }));
        }
        if let Some(local_deadline) = wait_deadline_local
            && Instant::now() >= local_deadline
        {
            return Ok(serde_json::json!({
                "id": response.id,
                "status": response.status,
                "urls": response.unsigned_urls,
            }));
        }
        tokio::select! {
            () = ctx.run.cancellation.cancelled() => return Err(timeout_error()),
            () = wait_deadline(ctx.run.deadline) => return Err(timeout_error()),
            () = tokio::time::sleep(POLL_INTERVAL) => {},
        }
    }
}

fn percent_encode_path_segment(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char);
            }
            _ => {
                out.push('%');
                let _ = std::fmt::Write::write_fmt(&mut out, format_args!("{byte:02X}"));
            }
        }
    }
    out
}
