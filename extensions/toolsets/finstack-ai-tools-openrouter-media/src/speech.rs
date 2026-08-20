//! `openrouter_generate_speech` handler.

use std::sync::Arc;

use finstack_ai_runtime::{ArtifactStore, ToolCallContext, ToolError};
use reqwest::header::HeaderValue;
use serde::Deserialize;

use crate::http::{
    deliver_media, inline_http_read_cap, invalid_arguments, parse_arguments, send_bytes,
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

#[allow(clippy::too_many_arguments)]
pub(crate) async fn handle_speech(
    client: &reqwest::Client,
    authorization: &HeaderValue,
    referer: Option<&str>,
    title: Option<&str>,
    endpoint: &str,
    max_result_bytes: usize,
    store: Option<&Arc<dyn ArtifactStore>>,
    ctx: &ToolCallContext,
    arguments: &[u8],
) -> Result<serde_json::Value, ToolError> {
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
        client,
        authorization,
        referer,
        title,
        reqwest::Method::POST,
        &format!("{endpoint}/api/v1/audio/speech"),
        Some(&body),
        ctx,
        inline_http_read_cap(max_result_bytes, store.is_some(), true),
    )
    .await?;
    let media_type = content_type.unwrap_or_else(|| "audio/mpeg".to_owned());
    deliver_media(
        bytes,
        &media_type,
        "openrouter-speech",
        store,
        ctx,
        max_result_bytes,
    )
    .await
}
