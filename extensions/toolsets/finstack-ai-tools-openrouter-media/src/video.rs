//! `openrouter_generate_video` and `openrouter_get_video` handlers.

use std::sync::Arc;
use std::time::Instant;

use base64::Engine as _;
use finstack_ai_kernel::{ArtifactRef, ErrorCategory};
use finstack_ai_runtime::artifact::{ArtifactScope, ArtifactStore, validate_retrieved_artifact};
use finstack_ai_runtime::ports::tool::{ToolCallContext, ToolError};
use reqwest::header::HeaderValue;
use serde::Deserialize;

use crate::config::{
    MAX_RESULT_BYTES_CEILING, OPENROUTER_MEDIA_LIMIT_EXCEEDED, OPENROUTER_MEDIA_STORE_REQUIRED,
    OPENROUTER_MEDIA_TRANSPORT_FAILED,
};
use crate::http::{
    BASE64_STANDARD, POLL_INTERVAL, deliver_media, invalid_arguments, parse_arguments, send_bytes,
    send_json, timeout_error, tool_error, wait_deadline,
};

pub(crate) const VIDEO_TOOL_ID: &str = "finstack.tools.openrouter_generate_video";
pub(crate) const VIDEO_TOOL_NAME: &str = "openrouter_generate_video";
pub(crate) const VIDEO_STATUS_TOOL_ID: &str = "finstack.tools.openrouter_get_video";
pub(crate) const VIDEO_STATUS_TOOL_NAME: &str = "openrouter_get_video";
pub(crate) const VIDEO_DOWNLOAD_TOOL_ID: &str = "finstack.tools.openrouter_download_video";
pub(crate) const VIDEO_DOWNLOAD_TOOL_NAME: &str = "openrouter_download_video";

/// Ceiling for inline base64 data-URI frame/reference images.
pub(crate) const MAX_INLINE_IMAGE_BYTES: usize = 8 * 1_048_576;

/// Test-shrunk inline-image ceiling so oversize-rejection tests stay fast.
fn inline_image_ceiling() -> usize {
    if cfg!(test) {
        1_024
    } else {
        MAX_INLINE_IMAGE_BYTES
    }
}

/// One frame or reference image: exactly one of a URL or a stored artifact.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ImageInput {
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    artifact: Option<ArtifactRef>,
}

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
    #[serde(default)]
    size: Option<String>,
    #[serde(default)]
    seed: Option<i64>,
    #[serde(default)]
    generate_audio: Option<bool>,
    #[serde(default)]
    first_frame: Option<ImageInput>,
    #[serde(default)]
    last_frame: Option<ImageInput>,
    #[serde(default)]
    reference_images: Option<Vec<ImageInput>>,
}

/// The Global Constraints artifact scope shared by artifact-input tools.
pub(crate) fn artifact_scope(ctx: &ToolCallContext) -> ArtifactScope {
    ArtifactScope {
        tenant_scope: Arc::clone(&ctx.run.locator.tenant_scope),
        session_id: ctx.run.locator.session_id,
        run_id: Some(ctx.run.locator.run_id),
        sensitivity: finstack_ai_kernel::Sensitivity::Internal,
    }
}

/// Resolve one frame/reference image input to a URL the endpoint accepts.
///
/// Exactly one of `url`/`artifact` must be set. An artifact input requires a
/// configured store and is read, verified, and bounded before being turned
/// into a data URI.
pub(crate) async fn resolve_image_input(
    store: Option<&Arc<dyn ArtifactStore>>,
    ctx: &ToolCallContext,
    input: &ImageInput,
) -> Result<String, ToolError> {
    match (&input.url, &input.artifact) {
        (Some(url), None) => Ok(url.clone()),
        (None, Some(artifact)) => {
            let Some(store) = store else {
                return Err(tool_error(
                    OPENROUTER_MEDIA_STORE_REQUIRED,
                    ErrorCategory::Configuration,
                    "openrouter media artifact frame inputs require an artifact store",
                ));
            };
            let scope = artifact_scope(ctx);
            let bytes = store
                .get(scope.clone(), artifact.clone())
                .await
                .map_err(|_| {
                    tool_error(
                        OPENROUTER_MEDIA_TRANSPORT_FAILED,
                        ErrorCategory::Tool,
                        "openrouter media frame artifact read failed",
                    )
                })?;
            validate_retrieved_artifact(&scope, artifact, &bytes).map_err(|_| {
                tool_error(
                    OPENROUTER_MEDIA_TRANSPORT_FAILED,
                    ErrorCategory::Tool,
                    "openrouter media frame artifact failed verification",
                )
            })?;
            if bytes.len() > inline_image_ceiling() {
                return Err(tool_error(
                    OPENROUTER_MEDIA_LIMIT_EXCEEDED,
                    ErrorCategory::Limit,
                    "openrouter media frame image exceeds the inline data-URI ceiling",
                ));
            }
            Ok(format!(
                "data:{};base64,{}",
                artifact.blob().media_type(),
                BASE64_STANDARD.encode(&bytes)
            ))
        }
        _ => Err(invalid_arguments(
            "openrouter media image input must set exactly one of url or artifact",
        )),
    }
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

#[allow(clippy::too_many_arguments)]
pub(crate) async fn handle_video_submit(
    client: &reqwest::Client,
    authorization: &HeaderValue,
    referer: Option<&str>,
    title: Option<&str>,
    endpoint: &str,
    store: Option<&Arc<dyn ArtifactStore>>,
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
        if let Some(size) = arguments.size {
            map.insert("size".into(), serde_json::Value::String(size));
        }
        if let Some(seed) = arguments.seed {
            map.insert("seed".into(), serde_json::Value::from(seed));
        }
        if let Some(generate_audio) = arguments.generate_audio {
            map.insert(
                "generate_audio".into(),
                serde_json::Value::from(generate_audio),
            );
        }
    }
    let mut frame_images = Vec::new();
    for (input, frame_type) in [
        (arguments.first_frame.as_ref(), "first_frame"),
        (arguments.last_frame.as_ref(), "last_frame"),
    ] {
        if let Some(input) = input {
            let url = resolve_image_input(store, ctx, input).await?;
            frame_images.push(serde_json::json!({
                "type": "image_url",
                "image_url": { "url": url },
                "frame_type": frame_type,
            }));
        }
    }
    let mut input_references = Vec::new();
    for input in arguments.reference_images.iter().flatten() {
        if input_references.len() >= 4 {
            return Err(invalid_arguments(
                "openrouter media accepts at most four reference images",
            ));
        }
        let url = resolve_image_input(store, ctx, input).await?;
        input_references.push(serde_json::json!({
            "type": "image_url",
            "image_url": { "url": url },
        }));
    }
    if let Some(map) = body.as_object_mut() {
        if !frame_images.is_empty() {
            map.insert(
                "frame_images".into(),
                serde_json::Value::Array(frame_images),
            );
        }
        if !input_references.is_empty() {
            map.insert(
                "input_references".into(),
                serde_json::Value::Array(input_references),
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct VideoDownloadArguments {
    id: String,
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn handle_video_download(
    client: &reqwest::Client,
    authorization: &HeaderValue,
    referer: Option<&str>,
    title: Option<&str>,
    endpoint: &str,
    store: Option<&Arc<dyn ArtifactStore>>,
    ctx: &ToolCallContext,
    arguments: &[u8],
) -> Result<serde_json::Value, ToolError> {
    let arguments: VideoDownloadArguments = parse_arguments(arguments)?;
    if arguments.id.is_empty() {
        return Err(invalid_arguments("openrouter media video id is empty"));
    }
    let Some(store) = store else {
        return Err(tool_error(
            OPENROUTER_MEDIA_STORE_REQUIRED,
            ErrorCategory::Configuration,
            "openrouter media video download requires an artifact store",
        ));
    };
    let cap = store.limits().max_artifact_bytes;
    let url = format!(
        "{endpoint}/api/v1/videos/{}/content",
        percent_encode_path_segment(&arguments.id)
    );
    let (bytes, content_type) = send_bytes(
        client,
        authorization,
        referer,
        title,
        reqwest::Method::GET,
        &url,
        None,
        ctx,
        cap,
    )
    .await?;
    let media_type = content_type.unwrap_or_else(|| "video/mp4".to_owned());
    let delivered = deliver_media(
        bytes,
        &media_type,
        "openrouter-video",
        Some(store),
        ctx,
        usize::MAX,
    )
    .await?;
    Ok(delivered.value)
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
