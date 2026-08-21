//! `openrouter_generate_image` handler.

use std::sync::Arc;

use base64::Engine as _;
use finstack_ai_kernel::ErrorCategory;
use finstack_ai_runtime::{ArtifactStore, ToolCallContext, ToolError};
use reqwest::header::HeaderValue;
use serde::Deserialize;

use crate::config::OPENROUTER_MEDIA_TRANSPORT_FAILED;
use crate::http::{
    BASE64_STANDARD, DeliveredMedia, deliver_media, inline_http_read_cap, invalid_arguments,
    parse_arguments, send_json, tool_error,
};

pub(crate) const IMAGE_TOOL_ID: &str = "finstack.tools.openrouter_generate_image";
pub(crate) const IMAGE_TOOL_NAME: &str = "openrouter_generate_image";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ImageArguments {
    model: String,
    prompt: String,
    #[serde(default)]
    resolution: Option<String>,
    #[serde(default)]
    aspect_ratio: Option<String>,
    #[serde(default)]
    output_format: Option<String>,
}

#[derive(Deserialize)]
struct ImageResponseItem {
    b64_json: String,
    #[serde(default)]
    media_type: Option<String>,
}

#[derive(Deserialize)]
struct ImageResponse {
    data: Vec<ImageResponseItem>,
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn handle_image(
    client: &reqwest::Client,
    authorization: &HeaderValue,
    referer: Option<&str>,
    title: Option<&str>,
    endpoint: &str,
    max_result_bytes: usize,
    store: Option<&Arc<dyn ArtifactStore>>,
    ctx: &ToolCallContext,
    arguments: &[u8],
) -> Result<DeliveredMedia, ToolError> {
    let arguments: ImageArguments = parse_arguments(arguments)?;
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
        if let Some(resolution) = arguments.resolution {
            map.insert("resolution".into(), serde_json::Value::String(resolution));
        }
        if let Some(aspect_ratio) = arguments.aspect_ratio {
            map.insert(
                "aspect_ratio".into(),
                serde_json::Value::String(aspect_ratio),
            );
        }
        if let Some(output_format) = arguments.output_format {
            map.insert(
                "output_format".into(),
                serde_json::Value::String(output_format),
            );
        }
    }
    let response: ImageResponse = send_json(
        client,
        authorization,
        referer,
        title,
        reqwest::Method::POST,
        &format!("{endpoint}/api/v1/images"),
        Some(&body),
        ctx,
        inline_http_read_cap(max_result_bytes, store.is_some(), false),
    )
    .await?;
    let item = response.data.into_iter().next().ok_or_else(|| {
        tool_error(
            OPENROUTER_MEDIA_TRANSPORT_FAILED,
            ErrorCategory::Tool,
            "openrouter media image response omitted image data",
        )
    })?;
    let bytes = BASE64_STANDARD.decode(item.b64_json).map_err(|_| {
        tool_error(
            OPENROUTER_MEDIA_TRANSPORT_FAILED,
            ErrorCategory::Tool,
            "openrouter media image data is not valid base64",
        )
    })?;
    let media_type = item.media_type.unwrap_or_else(|| "image/png".to_owned());
    deliver_media(
        bytes,
        &media_type,
        "openrouter-image",
        store,
        ctx,
        max_result_bytes,
    )
    .await
}
