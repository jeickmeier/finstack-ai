//! `openrouter_generate_image` handler.

use base64::Engine as _;
use finstack_ai_kernel::ErrorCategory;
use finstack_ai_runtime::ports::tool::{ToolCallContext, ToolError};
use serde::Deserialize;

use crate::config::OPENROUTER_MEDIA_TRANSPORT_FAILED;
use crate::http::{
    BASE64_STANDARD, DeliveredMedia, Route, deliver_media, invalid_arguments, parse_arguments,
    send_json, tool_error,
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

pub(crate) async fn handle_image(
    route: &Route,
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
        for (key, value) in [
            ("resolution", arguments.resolution),
            ("aspect_ratio", arguments.aspect_ratio),
            ("output_format", arguments.output_format),
        ] {
            if let Some(value) = value {
                map.insert(key.into(), serde_json::Value::String(value));
            }
        }
    }
    let response: ImageResponse = send_json(
        route,
        reqwest::Method::POST,
        &format!("{}/api/v1/images", route.endpoint),
        Some(&body),
        ctx,
        route.inline_http_read_cap(false),
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
    deliver_media(route, bytes, &media_type, "openrouter-image", ctx).await
}
