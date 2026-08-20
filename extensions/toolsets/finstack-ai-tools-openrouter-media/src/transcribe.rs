//! `openrouter_transcribe_audio` handler.

use base64::Engine as _;
use finstack_ai_runtime::{ToolCallContext, ToolError};
use reqwest::header::HeaderValue;
use serde::Deserialize;

use crate::config::MAX_RESULT_BYTES_CEILING;
use crate::http::{BASE64_STANDARD, invalid_arguments, parse_arguments, send_json};
use crate::url::{MAX_AUDIO_DOWNLOAD_BYTES, download_bytes, validate_download_url};

pub(crate) const TRANSCRIBE_TOOL_ID: &str = "finstack.tools.openrouter_transcribe_audio";
pub(crate) const TRANSCRIBE_TOOL_NAME: &str = "openrouter_transcribe_audio";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TranscribeArguments {
    model: String,
    audio_url: String,
    #[serde(default)]
    format: Option<String>,
}

#[derive(Deserialize)]
struct TranscribeResponse {
    text: String,
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn handle_transcribe(
    client: &reqwest::Client,
    authorization: &HeaderValue,
    referer: Option<&str>,
    title: Option<&str>,
    endpoint: &str,
    endpoint_is_loopback: bool,
    ctx: &ToolCallContext,
    arguments: &[u8],
) -> Result<serde_json::Value, ToolError> {
    let arguments: TranscribeArguments = parse_arguments(arguments)?;
    if arguments.model.is_empty() || arguments.audio_url.is_empty() {
        return Err(invalid_arguments(
            "openrouter media model or audio_url is empty",
        ));
    }
    validate_download_url(&arguments.audio_url, endpoint_is_loopback)?;
    let downloaded = download_bytes(
        &arguments.audio_url,
        endpoint_is_loopback,
        ctx,
        MAX_AUDIO_DOWNLOAD_BYTES,
    )
    .await?;
    let b64_audio = BASE64_STANDARD.encode(downloaded);
    let format = arguments.format.unwrap_or_else(|| {
        // Presigned/signed download URLs commonly carry a query string (and
        // occasionally a fragment) after the real file extension, e.g.
        // `https://bucket.example/a.mp3?X-Sig=...`; strip both before
        // deriving the extension so the signature doesn't leak into `format`.
        let path = arguments
            .audio_url
            .split(['?', '#'])
            .next()
            .unwrap_or(arguments.audio_url.as_str());
        path.rsplit('.')
            .next()
            .filter(|ext| !ext.is_empty() && !ext.contains('/'))
            .map_or_else(|| "mp3".to_owned(), str::to_owned)
    });
    // The endpoint takes the payload as a nested `input_audio` object; a
    // flat `audio`/`format` pair is rejected as a missing object.
    let body = serde_json::json!({
        "model": arguments.model,
        "input_audio": {"data": b64_audio, "format": format},
    });
    let response: TranscribeResponse = send_json(
        client,
        authorization,
        referer,
        title,
        reqwest::Method::POST,
        &format!("{endpoint}/api/v1/audio/transcriptions"),
        Some(&body),
        ctx,
        MAX_RESULT_BYTES_CEILING,
    )
    .await?;
    Ok(serde_json::json!({ "text": response.text }))
}
