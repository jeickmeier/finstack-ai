//! `openrouter_transcribe_audio` handler.

use base64::Engine as _;
use finstack_ai_runtime::ports::tool::{ToolCallContext, ToolError};
use serde::{Deserialize, Serialize};

use crate::config::MAX_RESULT_BYTES_CEILING;
use crate::http::{BASE64_STANDARD, Route, invalid_arguments, parse_arguments, send_json};
use crate::url::{MAX_AUDIO_DOWNLOAD_BYTES, download_bytes};

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

/// Request body for `/api/v1/audio/transcriptions`.
///
/// The endpoint takes the payload as a nested `input_audio` object; a flat
/// `audio`/`format` pair is rejected as a missing object. Fields borrow so
/// the base64 audio is encoded once and serialized once.
#[derive(Serialize)]
struct TranscribeBody<'a> {
    model: &'a str,
    input_audio: InputAudio<'a>,
}

#[derive(Serialize)]
struct InputAudio<'a> {
    data: &'a str,
    format: &'a str,
}

pub(crate) async fn handle_transcribe(
    route: &Route,
    ctx: &ToolCallContext,
    arguments: &[u8],
) -> Result<serde_json::Value, ToolError> {
    let arguments: TranscribeArguments = parse_arguments(arguments)?;
    if arguments.model.is_empty() || arguments.audio_url.is_empty() {
        return Err(invalid_arguments(
            "openrouter media model or audio_url is empty",
        ));
    }
    let downloaded = download_bytes(&arguments.audio_url, ctx, MAX_AUDIO_DOWNLOAD_BYTES).await?;
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
    let body = TranscribeBody {
        model: &arguments.model,
        input_audio: InputAudio {
            data: &b64_audio,
            format: &format,
        },
    };
    let response: TranscribeResponse = send_json(
        route,
        reqwest::Method::POST,
        &format!("{}/api/v1/audio/transcriptions", route.endpoint),
        Some(&body),
        ctx,
        MAX_RESULT_BYTES_CEILING,
    )
    .await?;
    Ok(serde_json::json!({ "text": response.text }))
}

#[cfg(test)]
mod tests {
    use super::{InputAudio, TranscribeBody};

    #[test]
    fn transcribe_body_shape_is_unchanged() {
        let body = TranscribeBody {
            model: "m",
            input_audio: InputAudio {
                data: "aGVsbG8=",
                format: "mp3",
            },
        };
        assert_eq!(
            serde_json::to_value(&body).expect("body"),
            serde_json::json!({
                "model": "m",
                "input_audio": {"data": "aGVsbG8=", "format": "mp3"},
            })
        );
    }
}
