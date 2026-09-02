//! `openrouter_generate_speech` handler.

use finstack_ai_runtime::ports::tool::{ToolCallContext, ToolError};
use serde::Deserialize;

use crate::http::{
    DeliveredMedia, Route, deliver_media, invalid_arguments, parse_arguments, send_bytes,
};

pub(crate) const SPEECH_TOOL_ID: &str = "finstack.tools.openrouter_generate_speech";
pub(crate) const SPEECH_TOOL_NAME: &str = "openrouter_generate_speech";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SpeechArguments {
    model: String,
    input: String,
    #[serde(default)]
    voice: Option<String>,
    #[serde(default)]
    response_format: Option<String>,
}

pub(crate) async fn handle_speech(
    route: &Route,
    ctx: &ToolCallContext,
    arguments: &[u8],
) -> Result<DeliveredMedia, ToolError> {
    let arguments: SpeechArguments = parse_arguments(arguments)?;
    if arguments.model.is_empty() || arguments.input.is_empty() {
        return Err(invalid_arguments(
            "openrouter media model or input is empty",
        ));
    }
    // The endpoint defaults to headerless PCM, which is bytes no player
    // opens as a file. Ask for mp3 unless the caller wants the raw samples.
    let response_format = arguments
        .response_format
        .unwrap_or_else(|| "mp3".to_owned());
    let mut body = serde_json::json!({
        "model": arguments.model,
        "input": arguments.input,
        "response_format": response_format,
    });
    if let Some(map) = body.as_object_mut()
        && let Some(voice) = arguments.voice
    {
        map.insert("voice".into(), serde_json::Value::String(voice));
    }
    let (bytes, content_type) = send_bytes(
        route,
        reqwest::Method::POST,
        &format!("{}/api/v1/audio/speech", route.endpoint),
        Some(&body),
        ctx,
        route.inline_http_read_cap(true),
    )
    .await?;
    let media_type = content_type.unwrap_or_else(|| "audio/mpeg".to_owned());
    deliver_media(route, bytes, &media_type, "openrouter-speech", ctx).await
}
