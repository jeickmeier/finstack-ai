//! T1 `OpenRouter` media-generation Toolset.
//!
//! Exposes `OpenRouter`'s image, video, speech, and transcription endpoints as
//! bounded agent tools. Construction requires an explicit API key and never
//! reads environment variables. Non-loopback endpoints must be HTTPS. Tool
//! results are bounded JSON: hosted URLs / job identifiers are preferred and
//! base64 payloads are returned only when they fit the result cap.

#![warn(missing_docs)]
#![forbid(unsafe_code)]
#![warn(clippy::float_cmp)]
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
#![deny(clippy::panic)]
#![deny(clippy::unreachable)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::indexing_slicing,
        clippy::float_cmp,
    )
)]
// Allow expect() in doc tests (they are test code)
#![doc(test(attr(allow(clippy::expect_used))))]

use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use finstack_ai_kernel::{
    ErrorCategory, Metadata, RawJson, RetrySafety, Sensitivity, Timestamp, ToolExecutionMode,
    ToolId, ValidatedToolCall,
};
use finstack_ai_runtime::{
    ApprovalMetadata, ApprovalRequirement, ArtifactMetadata, ArtifactScope, ArtifactStore, Bytes,
    PortFuture, SideEffectClass, ToolCallContext, ToolDeferralSupport, ToolError, ToolEventStream,
    ToolResult, ToolSpec, ToolStreamItem, Toolset, ToolsetDescriptor, stage_required_artifact,
    verify_authority,
};
use futures_util::{StreamExt, stream};
use serde::Deserialize;
use thiserror::Error;

const DEFAULT_ENDPOINT: &str = "https://openrouter.ai";
const REQUEST_TIMEOUT: Duration = Duration::from_mins(2);
const MAX_RESULT_BYTES_CEILING: usize = 8 * 1_048_576;
const MAX_AUDIO_DOWNLOAD_BYTES: usize = 25 * 1_048_576;
/// Bytes of an error response body read before giving up on a reason.
const ERROR_BODY_CAP: usize = 4096;
/// Characters of endpoint reason kept in a tool-error message.
const ERROR_DETAIL_CHARS: usize = 300;
/// Sent when fetching caller-supplied audio, which is an arbitrary host
/// rather than `OpenRouter`.
const DOWNLOAD_USER_AGENT: &str = concat!(
    "finstack-ai-tools-openrouter-media/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/jeickmeier/finstack-ai)"
);

#[cfg(not(test))]
const POLL_INTERVAL: Duration = Duration::from_secs(5);
#[cfg(test)]
const POLL_INTERVAL: Duration = Duration::from_millis(5);

const IMAGE_TOOL_ID: &str = "finstack.tools.openrouter_generate_image";
const IMAGE_TOOL_NAME: &str = "openrouter_generate_image";
const VIDEO_TOOL_ID: &str = "finstack.tools.openrouter_generate_video";
const VIDEO_TOOL_NAME: &str = "openrouter_generate_video";
const VIDEO_STATUS_TOOL_ID: &str = "finstack.tools.openrouter_get_video";
const VIDEO_STATUS_TOOL_NAME: &str = "openrouter_get_video";
const SPEECH_TOOL_ID: &str = "finstack.tools.openrouter_generate_speech";
const SPEECH_TOOL_NAME: &str = "openrouter_generate_speech";
const TRANSCRIBE_TOOL_ID: &str = "finstack.tools.openrouter_transcribe_audio";
const TRANSCRIBE_TOOL_NAME: &str = "openrouter_transcribe_audio";

/// Stable missing-credential code.
pub const OPENROUTER_MEDIA_CREDENTIAL_REQUIRED: &str = "openrouter_media_credential_required";
/// Stable endpoint-configuration code.
pub const OPENROUTER_MEDIA_ENDPOINT_INVALID: &str = "openrouter_media_endpoint_invalid";
/// Stable argument-validation code.
pub const OPENROUTER_MEDIA_INVALID_ARGUMENTS: &str = "openrouter_media_invalid_arguments";
/// Stable remote-transport code.
pub const OPENROUTER_MEDIA_TRANSPORT_FAILED: &str = "openrouter_media_transport_failed";
/// Stable output-limit code.
pub const OPENROUTER_MEDIA_LIMIT_EXCEEDED: &str = "openrouter_media_limit_exceeded";
/// Stable cancellation/deadline code.
pub const OPENROUTER_MEDIA_TIMEOUT: &str = "openrouter_media_timeout";

/// Explicit `OpenRouter` media route. Never populated from the environment.
#[derive(Clone)]
pub struct OpenRouterMediaConfig {
    /// Explicit API key. Empty values fail closed.
    pub api_key: String,
    /// HTTPS endpoint, or loopback HTTP for scripted fixtures. Empty selects
    /// `https://openrouter.ai`.
    pub endpoint: String,
    /// Optional non-secret `HTTP-Referer` attribution header.
    pub referer: Option<String>,
    /// Optional non-secret `X-Title` attribution header.
    pub title: Option<String>,
    /// Result-size cap in bytes. Images and speech come back base64, so
    /// hosts wanting inline media raise this. Zero or above 8 MiB fails
    /// construction; the SDK default is `262_144`.
    pub max_result_bytes: usize,
}

impl std::fmt::Debug for OpenRouterMediaConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenRouterMediaConfig")
            .field("api_key", &"[redacted]")
            .field("endpoint", &self.endpoint)
            .field("referer", &self.referer)
            .field("title", &self.title)
            .field("max_result_bytes", &self.max_result_bytes)
            .finish()
    }
}

/// Construction or route failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum OpenRouterMediaError {
    /// API key was omitted.
    #[error(
        "{OPENROUTER_MEDIA_CREDENTIAL_REQUIRED}: openrouter media construction requires an explicit API key"
    )]
    CredentialRequired,
    /// Endpoint scheme, host, components, or result cap are invalid.
    #[error("{OPENROUTER_MEDIA_ENDPOINT_INVALID}: {reason}")]
    EndpointInvalid {
        /// Stable non-secret reason.
        reason: &'static str,
    },
}

/// T1 `OpenRouter` media-generation Toolset.
pub struct OpenRouterMediaToolset {
    descriptor: ToolsetDescriptor,
    tools: Arc<[ToolSpec]>,
    image_tool_id: ToolId,
    video_tool_id: ToolId,
    video_status_tool_id: ToolId,
    speech_tool_id: ToolId,
    transcribe_tool_id: ToolId,
    api_key: String,
    endpoint: String,
    endpoint_is_loopback: bool,
    referer: Option<String>,
    title: Option<String>,
    max_result_bytes: usize,
    artifact_store: Option<Arc<dyn ArtifactStore>>,
    client: reqwest::Client,
}

impl std::fmt::Debug for OpenRouterMediaToolset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenRouterMediaToolset")
            .field("endpoint", &self.endpoint)
            .field("max_result_bytes", &self.max_result_bytes)
            .field("artifact_store", &self.artifact_store.is_some())
            .finish_non_exhaustive()
    }
}

impl OpenRouterMediaToolset {
    /// Construct the Toolset after validating the explicit route.
    ///
    /// # Errors
    ///
    /// Returns [`OpenRouterMediaError::CredentialRequired`] when `api_key` is empty.
    /// Returns [`OpenRouterMediaError::EndpointInvalid`] for a non-HTTP URL, userinfo,
    /// query, fragment, plaintext HTTP off loopback, or an out-of-range result cap.
    #[allow(clippy::too_many_lines)]
    pub fn try_new(config: OpenRouterMediaConfig) -> Result<Self, OpenRouterMediaError> {
        if config.api_key.is_empty() {
            return Err(OpenRouterMediaError::CredentialRequired);
        }
        if config.max_result_bytes == 0 || config.max_result_bytes > MAX_RESULT_BYTES_CEILING {
            return Err(OpenRouterMediaError::EndpointInvalid {
                reason: "result cap out of range",
            });
        }
        let endpoint = if config.endpoint.is_empty() {
            DEFAULT_ENDPOINT.to_owned()
        } else {
            config.endpoint
        };
        validate_endpoint(&endpoint)?;
        let endpoint = endpoint.trim_end_matches('/').to_owned();
        let endpoint_is_loopback = endpoint
            .split_once("://")
            .and_then(|(_, rest)| endpoint_host(rest))
            .is_some_and(is_loopback_host);

        let max_result_bytes_u64 = u64::try_from(config.max_result_bytes).unwrap_or(u64::MAX);

        let image_tool_id =
            ToolId::parse(IMAGE_TOOL_ID).map_err(|_| OpenRouterMediaError::EndpointInvalid {
                reason: "invalid_tool_id",
            })?;
        let video_tool_id =
            ToolId::parse(VIDEO_TOOL_ID).map_err(|_| OpenRouterMediaError::EndpointInvalid {
                reason: "invalid_tool_id",
            })?;
        let video_status_tool_id = ToolId::parse(VIDEO_STATUS_TOOL_ID).map_err(|_| {
            OpenRouterMediaError::EndpointInvalid {
                reason: "invalid_tool_id",
            }
        })?;
        let speech_tool_id =
            ToolId::parse(SPEECH_TOOL_ID).map_err(|_| OpenRouterMediaError::EndpointInvalid {
                reason: "invalid_tool_id",
            })?;
        let transcribe_tool_id = ToolId::parse(TRANSCRIBE_TOOL_ID).map_err(|_| {
            OpenRouterMediaError::EndpointInvalid {
                reason: "invalid_tool_id",
            }
        })?;

        let paid_approval = ApprovalMetadata {
            requirement: ApprovalRequirement::Policy,
            reason: Some(Arc::from("paid OpenRouter media generation")),
            attributes: Metadata::empty(),
        };

        let image_spec = ToolSpec {
            id: image_tool_id.clone(),
            model_name: Arc::from(IMAGE_TOOL_NAME),
            title: Arc::from("OpenRouter generate image"),
            description: Arc::from("Generate images via OpenRouter; returns base64 image data."),
            input_schema: RawJson::parse(
                br#"{"additionalProperties":false,"properties":{"aspect_ratio":{"description":"Aspect ratio such as 16:9 or 1:1. Null uses the model default.","type":["string","null"]},"model":{"description":"OpenRouter model id that produces image output, for example google/gemini-3-pro-image or openai/gpt-5-image. A text-only chat model id is rejected.","minLength":1,"type":"string"},"output_format":{"description":"Image container such as png or jpeg. Null uses the model default.","type":["string","null"]},"prompt":{"description":"Text description of the image to generate.","minLength":1,"type":"string"},"resolution":{"description":"Resolution token the chosen model accepts. Each model defines its own set, so prefer null unless a specific size is required: bytedance-seed/seedream-5-0-pro takes 512, 1K, 2K, or 4K, while other models take pixel pairs such as 1024x1024. A rejected value is reported with the accepted list.","type":["string","null"]}},"required":["aspect_ratio","model","output_format","prompt","resolution"],"type":"object"}"#,
            )
            .map_err(|_| OpenRouterMediaError::EndpointInvalid {
                reason: "invalid_input_schema",
            })?,
            output_schema: Some(
                RawJson::parse(
                    br#"{"additionalProperties":false,"properties":{"artifact":{"description":"Staged artifact reference holding the image bytes. Present when the host configured an artifact store; pass it to tools that accept an artifact.","type":"object"},"b64_data":{"description":"Base64 image bytes. Present only when no artifact store is configured.","type":"string"},"byte_length":{"type":"integer"},"media_type":{"type":"string"}},"required":["media_type","byte_length"],"type":"object"}"#,
                )
                .map_err(|_| OpenRouterMediaError::EndpointInvalid {
                    reason: "invalid_output_schema",
                })?,
            ),
            execution: ToolExecutionMode::Sequential,
            side_effect: SideEffectClass::NonIdempotentWrite,
            retry_safety: RetrySafety::AtMostOnce,
            approval: paid_approval.clone(),
            max_result_bytes: max_result_bytes_u64,
            metadata: Metadata::empty(),
            deferral: ToolDeferralSupport::Never,
        };
        image_spec
            .validate()
            .map_err(|_| OpenRouterMediaError::EndpointInvalid {
                reason: "invalid_tool_spec",
            })?;

        let video_spec = ToolSpec {
            id: video_tool_id.clone(),
            model_name: Arc::from(VIDEO_TOOL_NAME),
            title: Arc::from("OpenRouter generate video"),
            description: Arc::from(
                "Submit one asynchronous video-generation job via OpenRouter; it returns a job id immediately, then poll that id with openrouter_get_video. Every call starts a new separately billed job, so call this at most once per requested video: if a job is already pending, poll its id instead of submitting again.",
            ),
            input_schema: RawJson::parse(
                br#"{"additionalProperties":false,"properties":{"aspect_ratio":{"description":"Aspect ratio such as 16:9 or 9:16. Null uses the model default.","type":["string","null"]},"duration":{"description":"Clip length in seconds. Each model accepts a fixed set, most commonly 4 to 15; durations under 4 are supported by only a few models. Null uses the model default.","type":["integer","null"]},"model":{"description":"OpenRouter video model id, for example bytedance/seedance-2.0-mini, google/veo-3.1-fast, or openai/sora-2-pro. Video models are a separate catalogue from chat models; a chat model id is rejected.","minLength":1,"type":"string"},"prompt":{"description":"Text description of the video to generate.","minLength":1,"type":"string"},"resolution":{"description":"Resolution such as 480p, 720p, or 1080p. Must be supported by the chosen model. Null uses the model default.","type":["string","null"]}},"required":["aspect_ratio","duration","model","prompt","resolution"],"type":"object"}"#,
            )
            .map_err(|_| OpenRouterMediaError::EndpointInvalid {
                reason: "invalid_input_schema",
            })?,
            output_schema: Some(
                RawJson::parse(
                    br#"{"additionalProperties":false,"properties":{"id":{"type":"string"},"status":{"type":"string"}},"required":["id","status"],"type":"object"}"#,
                )
                .map_err(|_| OpenRouterMediaError::EndpointInvalid {
                    reason: "invalid_output_schema",
                })?,
            ),
            execution: ToolExecutionMode::Sequential,
            side_effect: SideEffectClass::NonIdempotentWrite,
            retry_safety: RetrySafety::AtMostOnce,
            approval: paid_approval.clone(),
            max_result_bytes: max_result_bytes_u64,
            metadata: Metadata::empty(),
            deferral: ToolDeferralSupport::Never,
        };
        video_spec
            .validate()
            .map_err(|_| OpenRouterMediaError::EndpointInvalid {
                reason: "invalid_tool_spec",
            })?;

        let video_status_spec = ToolSpec {
            id: video_status_tool_id.clone(),
            model_name: Arc::from(VIDEO_STATUS_TOOL_NAME),
            title: Arc::from("OpenRouter get video"),
            description: Arc::from(
                "Check one OpenRouter video job; returns download URLs when completed. Set wait_seconds (0-300) to keep polling inside this call so one call covers the whole job. Generation commonly takes 30 seconds to several minutes, so prefer a single call with wait_seconds=300 over repeated short calls. A pending or in_progress result means that budget ran out, not that the job failed: call this tool again with the same id. Never submit a new job because one is still running.",
            ),
            input_schema: RawJson::parse(
                br#"{"additionalProperties":false,"properties":{"id":{"description":"Job id returned by openrouter_generate_video.","minLength":1,"type":"string"},"wait_seconds":{"description":"Seconds to keep polling inside this call before returning whatever status the job has. 0 or null returns immediately; 300 waits out a typical generation in a single call.","maximum":300,"minimum":0,"type":["integer","null"]}},"required":["id","wait_seconds"],"type":"object"}"#,
            )
            .map_err(|_| OpenRouterMediaError::EndpointInvalid {
                reason: "invalid_input_schema",
            })?,
            output_schema: Some(
                RawJson::parse(
                    br#"{"additionalProperties":false,"properties":{"id":{"type":"string"},"status":{"type":"string"},"urls":{"items":{"type":"string"},"type":"array"}},"required":["id","status"],"type":"object"}"#,
                )
                .map_err(|_| OpenRouterMediaError::EndpointInvalid {
                    reason: "invalid_output_schema",
                })?,
            ),
            execution: ToolExecutionMode::Sequential,
            side_effect: SideEffectClass::ReadOnly,
            retry_safety: RetrySafety::AtMostOnce,
            approval: ApprovalMetadata {
                requirement: ApprovalRequirement::NotRequired,
                reason: None,
                attributes: Metadata::empty(),
            },
            max_result_bytes: max_result_bytes_u64,
            metadata: Metadata::empty(),
            deferral: ToolDeferralSupport::Never,
        };
        video_status_spec
            .validate()
            .map_err(|_| OpenRouterMediaError::EndpointInvalid {
                reason: "invalid_tool_spec",
            })?;

        let speech_spec = ToolSpec {
            id: speech_tool_id.clone(),
            model_name: Arc::from(SPEECH_TOOL_NAME),
            title: Arc::from("OpenRouter generate speech"),
            description: Arc::from(
                "Synthesize speech from text via OpenRouter. Returns the audio as base64 with its media type.",
            ),
            input_schema: RawJson::parse(
                br#"{"additionalProperties":false,"properties":{"input":{"description":"Text to speak.","minLength":1,"type":"string"},"model":{"description":"OpenRouter text-to-speech model id, for example x-ai/grok-voice-tts-1.0, deepgram/aura-2, minimax/speech-2.8-turbo, or hexgrad/kokoro-82m. Chat model ids and OpenAI ids such as openai/tts-1 are rejected. The current list is GET /api/v1/models?output_modalities=speech.","minLength":1,"type":"string"},"response_format":{"description":"Audio container. Null selects mp3, a self-contained file any player opens; pcm returns headerless samples that most players cannot open on their own.","enum":["mp3","pcm",null],"type":["string","null"]},"voice":{"description":"Voice name accepted by the chosen model, for example eve for x-ai/grok-voice-tts-1.0. Null uses the model default.","type":["string","null"]}},"required":["input","model","response_format","voice"],"type":"object"}"#,
            )
            .map_err(|_| OpenRouterMediaError::EndpointInvalid {
                reason: "invalid_input_schema",
            })?,
            output_schema: Some(
                RawJson::parse(
                    br#"{"additionalProperties":false,"properties":{"artifact":{"description":"Staged artifact reference holding the audio bytes. Present when the host configured an artifact store; pass it to tools that accept an artifact.","type":"object"},"b64_data":{"description":"Base64 audio bytes. Present only when no artifact store is configured.","type":"string"},"byte_length":{"type":"integer"},"media_type":{"type":"string"}},"required":["media_type","byte_length"],"type":"object"}"#,
                )
                .map_err(|_| OpenRouterMediaError::EndpointInvalid {
                    reason: "invalid_output_schema",
                })?,
            ),
            execution: ToolExecutionMode::Sequential,
            side_effect: SideEffectClass::NonIdempotentWrite,
            retry_safety: RetrySafety::AtMostOnce,
            approval: paid_approval.clone(),
            max_result_bytes: max_result_bytes_u64,
            metadata: Metadata::empty(),
            deferral: ToolDeferralSupport::Never,
        };
        speech_spec
            .validate()
            .map_err(|_| OpenRouterMediaError::EndpointInvalid {
                reason: "invalid_tool_spec",
            })?;

        let transcribe_spec = ToolSpec {
            id: transcribe_tool_id.clone(),
            model_name: Arc::from(TRANSCRIBE_TOOL_NAME),
            title: Arc::from("OpenRouter transcribe audio"),
            description: Arc::from(
                "Transcribe audio at an HTTPS URL via OpenRouter (the toolset downloads and base64-submits it; OpenRouter accepts no audio URLs).",
            ),
            input_schema: RawJson::parse(
                br#"{"additionalProperties":false,"properties":{"audio_url":{"description":"HTTPS URL of the audio to transcribe.","minLength":1,"type":"string"},"format":{"description":"Container format such as wav, mp3, flac, ogg, or m4a. The bytes must actually be in that container: headerless PCM labelled wav is rejected. Null derives the format from the URL extension.","type":["string","null"]},"model":{"description":"OpenRouter speech-to-text model id, for example openai/whisper-1 or deepgram/nova-3. Chat model ids are rejected by this endpoint.","minLength":1,"type":"string"}},"required":["audio_url","format","model"],"type":"object"}"#,
            )
            .map_err(|_| OpenRouterMediaError::EndpointInvalid {
                reason: "invalid_input_schema",
            })?,
            output_schema: Some(
                RawJson::parse(
                    br#"{"additionalProperties":false,"properties":{"text":{"type":"string"}},"required":["text"],"type":"object"}"#,
                )
                .map_err(|_| OpenRouterMediaError::EndpointInvalid {
                    reason: "invalid_output_schema",
                })?,
            ),
            execution: ToolExecutionMode::Sequential,
            side_effect: SideEffectClass::NonIdempotentWrite,
            retry_safety: RetrySafety::AtMostOnce,
            approval: paid_approval,
            max_result_bytes: max_result_bytes_u64,
            metadata: Metadata::empty(),
            deferral: ToolDeferralSupport::Never,
        };
        transcribe_spec
            .validate()
            .map_err(|_| OpenRouterMediaError::EndpointInvalid {
                reason: "invalid_tool_spec",
            })?;

        // Downloads (e.g. transcription audio_url) must not follow redirects
        // past the caller-supplied-URL validation performed before the
        // request is sent — a redirect could otherwise smuggle the request
        // to a host/scheme that validate_download_url never saw.
        let client = reqwest::Client::builder()
            .http1_only()
            .timeout(REQUEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| OpenRouterMediaError::EndpointInvalid {
                reason: "http_client",
            })?;

        Ok(Self {
            descriptor: ToolsetDescriptor {
                name: Arc::from("finstack-openrouter-media"),
                metadata: Metadata::empty(),
            },
            tools: Arc::from([
                image_spec,
                video_spec,
                video_status_spec,
                speech_spec,
                transcribe_spec,
            ]),
            image_tool_id,
            video_tool_id,
            video_status_tool_id,
            speech_tool_id,
            transcribe_tool_id,
            api_key: config.api_key,
            endpoint,
            endpoint_is_loopback,
            referer: config.referer,
            title: config.title,
            max_result_bytes: config.max_result_bytes,
            artifact_store: None,
            client,
        })
    }

    /// Stage generated audio and images instead of inlining them.
    ///
    /// With a store attached, image and speech results carry an `artifact`
    /// reference and the model never receives the base64 payload.
    #[must_use]
    pub fn with_artifact_store(mut self, store: Arc<dyn ArtifactStore>) -> Self {
        self.artifact_store = Some(store);
        self
    }
}

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

/// Hand generated media back to the model without inlining the bytes.
///
/// Generated audio and images are hundreds of kilobytes that a model cannot
/// read and must not have to carry; when a store is configured the bytes are
/// staged and only the reference travels in the result. Without a store the
/// payload is inlined as base64, still bounded by `max_result_bytes`.
async fn deliver_media(
    bytes: Vec<u8>,
    media_type: &str,
    name: &'static str,
    store: Option<&Arc<dyn ArtifactStore>>,
    ctx: &ToolCallContext,
    max_result_bytes: usize,
) -> Result<serde_json::Value, ToolError> {
    let byte_length = bytes.len();
    let Some(store) = store else {
        if byte_length.saturating_mul(4) / 3 > max_result_bytes {
            return Err(tool_error(
                OPENROUTER_MEDIA_LIMIT_EXCEEDED,
                ErrorCategory::Limit,
                "openrouter media result exceeds the configured byte limit",
            ));
        }
        return Ok(serde_json::json!({
            "b64_data": BASE64_STANDARD.encode(bytes),
            "media_type": media_type,
            "byte_length": byte_length,
        }));
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
    Ok(serde_json::json!({
        "artifact": artifact,
        "media_type": media_type,
        "byte_length": byte_length,
    }))
}

impl Toolset for OpenRouterMediaToolset {
    fn descriptor(&self) -> ToolsetDescriptor {
        self.descriptor.clone()
    }

    fn tools(&self) -> Arc<[ToolSpec]> {
        Arc::clone(&self.tools)
    }

    #[allow(clippy::too_many_lines)]
    fn call(
        &self,
        ctx: ToolCallContext,
        call: ValidatedToolCall,
    ) -> PortFuture<Result<ToolEventStream, ToolError>> {
        let client = self.client.clone();
        let api_key = self.api_key.clone();
        let endpoint = self.endpoint.clone();
        let endpoint_is_loopback = self.endpoint_is_loopback;
        let referer = self.referer.clone();
        let title = self.title.clone();
        let max_result_bytes = self.max_result_bytes;
        let artifact_store = self.artifact_store.clone();
        let image_tool_id = self.image_tool_id.clone();
        let video_tool_id = self.video_tool_id.clone();
        let video_status_tool_id = self.video_status_tool_id.clone();
        let speech_tool_id = self.speech_tool_id.clone();
        let transcribe_tool_id = self.transcribe_tool_id.clone();
        Box::pin(async move {
            verify_authority(&ctx)?;
            let tool_name = call.call.tool_name();
            let value = if call.tool_id == image_tool_id && tool_name == IMAGE_TOOL_NAME {
                handle_image(
                    &client,
                    &api_key,
                    referer.as_deref(),
                    title.as_deref(),
                    &endpoint,
                    max_result_bytes,
                    artifact_store.as_ref(),
                    &ctx,
                    call.call.arguments().as_bytes(),
                )
                .await?
            } else if call.tool_id == video_tool_id && tool_name == VIDEO_TOOL_NAME {
                handle_video_submit(
                    &client,
                    &api_key,
                    referer.as_deref(),
                    title.as_deref(),
                    &endpoint,
                    &ctx,
                    call.call.arguments().as_bytes(),
                )
                .await?
            } else if call.tool_id == video_status_tool_id && tool_name == VIDEO_STATUS_TOOL_NAME {
                handle_video_status(
                    &client,
                    &api_key,
                    referer.as_deref(),
                    title.as_deref(),
                    &endpoint,
                    &ctx,
                    call.call.arguments().as_bytes(),
                )
                .await?
            } else if call.tool_id == speech_tool_id && tool_name == SPEECH_TOOL_NAME {
                handle_speech(
                    &client,
                    &api_key,
                    referer.as_deref(),
                    title.as_deref(),
                    &endpoint,
                    max_result_bytes,
                    artifact_store.as_ref(),
                    &ctx,
                    call.call.arguments().as_bytes(),
                )
                .await?
            } else if call.tool_id == transcribe_tool_id && tool_name == TRANSCRIBE_TOOL_NAME {
                handle_transcribe(
                    &client,
                    &api_key,
                    referer.as_deref(),
                    title.as_deref(),
                    &endpoint,
                    endpoint_is_loopback,
                    &ctx,
                    call.call.arguments().as_bytes(),
                )
                .await?
            } else {
                return Err(tool_error(
                    OPENROUTER_MEDIA_INVALID_ARGUMENTS,
                    ErrorCategory::Validation,
                    "openrouter media call identity is invalid",
                ));
            };
            let output_bytes = serde_json::to_vec(&value).map_err(|_| {
                tool_error(
                    OPENROUTER_MEDIA_TRANSPORT_FAILED,
                    ErrorCategory::Internal,
                    "openrouter media result serialization failed",
                )
            })?;
            let result = ToolResult {
                output: RawJson::parse(output_bytes).map_err(|_| {
                    tool_error(
                        OPENROUTER_MEDIA_TRANSPORT_FAILED,
                        ErrorCategory::Internal,
                        "openrouter media result normalization failed",
                    )
                })?,
                is_error: false,
            };
            Ok(Box::pin(stream::once(async move {
                Ok(ToolStreamItem::Completed(result))
            })) as ToolEventStream)
        })
    }
}

fn invalid_arguments(message: &'static str) -> ToolError {
    tool_error(
        OPENROUTER_MEDIA_INVALID_ARGUMENTS,
        ErrorCategory::Validation,
        message,
    )
}

fn parse_arguments<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T, ToolError> {
    serde_json::from_slice(bytes)
        .map_err(|_| invalid_arguments("openrouter media arguments are invalid"))
}

#[allow(clippy::too_many_arguments)]
async fn handle_image(
    client: &reqwest::Client,
    api_key: &str,
    referer: Option<&str>,
    title: Option<&str>,
    endpoint: &str,
    max_result_bytes: usize,
    store: Option<&Arc<dyn ArtifactStore>>,
    ctx: &ToolCallContext,
    arguments: &[u8],
) -> Result<serde_json::Value, ToolError> {
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
        api_key,
        referer,
        title,
        reqwest::Method::POST,
        &format!("{endpoint}/api/v1/images"),
        Some(&body),
        ctx,
        MAX_RESULT_BYTES_CEILING,
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

async fn handle_video_submit(
    client: &reqwest::Client,
    api_key: &str,
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
        api_key,
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

async fn handle_video_status(
    client: &reqwest::Client,
    api_key: &str,
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
    let wait_deadline_local =
        (wait_seconds > 0).then(|| Instant::now() + Duration::from_secs(u64::from(wait_seconds)));
    loop {
        let response: VideoStatusResponse = send_json(
            client,
            api_key,
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

#[allow(clippy::too_many_arguments)]
async fn handle_speech(
    client: &reqwest::Client,
    api_key: &str,
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
        api_key,
        referer,
        title,
        reqwest::Method::POST,
        &format!("{endpoint}/api/v1/audio/speech"),
        Some(&body),
        ctx,
        MAX_RESULT_BYTES_CEILING,
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

#[allow(clippy::too_many_arguments)]
async fn send_bytes(
    client: &reqwest::Client,
    api_key: &str,
    referer: Option<&str>,
    title: Option<&str>,
    method: reqwest::Method,
    url: &str,
    body: Option<&serde_json::Value>,
    ctx: &ToolCallContext,
    cap: usize,
) -> Result<(Vec<u8>, Option<String>), ToolError> {
    if ctx.run.cancellation.is_cancelled() || deadline_elapsed(ctx.run.deadline) {
        return Err(timeout_error());
    }
    let mut request = client
        .request(method, url)
        .header("Authorization", format!("Bearer {api_key}"));
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
        return Err(endpoint_rejected("endpoint", status, response).await);
    }
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let bytes = fetch_bytes_bounded(response, cap).await?;
    Ok((bytes, content_type))
}

#[allow(clippy::too_many_arguments)]
async fn handle_transcribe(
    client: &reqwest::Client,
    api_key: &str,
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
    let downloaded =
        download_bytes(client, &arguments.audio_url, ctx, MAX_AUDIO_DOWNLOAD_BYTES).await?;
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
        api_key,
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

#[allow(clippy::too_many_arguments)]
async fn send_json<T: for<'de> Deserialize<'de>>(
    client: &reqwest::Client,
    api_key: &str,
    referer: Option<&str>,
    title: Option<&str>,
    method: reqwest::Method,
    url: &str,
    body: Option<&serde_json::Value>,
    ctx: &ToolCallContext,
    cap: usize,
) -> Result<T, ToolError> {
    if ctx.run.cancellation.is_cancelled() || deadline_elapsed(ctx.run.deadline) {
        return Err(timeout_error());
    }
    let mut request = client
        .request(method, url)
        .header("Authorization", format!("Bearer {api_key}"));
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
        return Err(endpoint_rejected("endpoint", status, response).await);
    }
    read_bounded_json(response, cap).await
}

async fn download_bytes(
    client: &reqwest::Client,
    url: &str,
    ctx: &ToolCallContext,
    cap: usize,
) -> Result<Vec<u8>, ToolError> {
    if ctx.run.cancellation.is_cancelled() || deadline_elapsed(ctx.run.deadline) {
        return Err(timeout_error());
    }
    // Identify the client: hosts serving public media commonly answer an
    // anonymous request with 403 rather than the file.
    let send = client
        .get(url)
        .header(reqwest::header::USER_AGENT, DOWNLOAD_USER_AGENT)
        .send();
    let response = tokio::select! {
        () = ctx.run.cancellation.cancelled() => return Err(timeout_error()),
        () = wait_deadline(ctx.run.deadline) => return Err(timeout_error()),
        result = send => result.map_err(|_| {
            tool_error(
                OPENROUTER_MEDIA_TRANSPORT_FAILED,
                ErrorCategory::Tool,
                "openrouter media audio download failed",
            )
        })?,
    };
    let status = response.status();
    if !status.is_success() {
        return Err(endpoint_rejected("audio host", status, response).await);
    }
    fetch_bytes_bounded(response, cap).await
}

async fn fetch_bytes_bounded(
    response: reqwest::Response,
    cap: usize,
) -> Result<Vec<u8>, ToolError> {
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| {
            tool_error(
                OPENROUTER_MEDIA_TRANSPORT_FAILED,
                ErrorCategory::Tool,
                "openrouter media response is invalid",
            )
        })?;
        if body.len().saturating_add(chunk.len()) > cap {
            return Err(tool_error(
                OPENROUTER_MEDIA_LIMIT_EXCEEDED,
                ErrorCategory::Limit,
                "openrouter media response exceeds the configured byte limit",
            ));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

async fn read_bounded_json<T: for<'de> Deserialize<'de>>(
    response: reqwest::Response,
    cap: usize,
) -> Result<T, ToolError> {
    let body = fetch_bytes_bounded(response, cap).await?;
    serde_json::from_slice(&body).map_err(|_| {
        tool_error(
            OPENROUTER_MEDIA_TRANSPORT_FAILED,
            ErrorCategory::Tool,
            "openrouter media response is invalid",
        )
    })
}

fn deadline_elapsed(deadline: Option<Timestamp>) -> bool {
    let Some(deadline) = deadline else {
        return false;
    };
    now_unix_ms() >= deadline.as_unix_ms()
}

async fn wait_deadline(deadline: Option<Timestamp>) {
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

fn timeout_error() -> ToolError {
    tool_error(
        OPENROUTER_MEDIA_TIMEOUT,
        ErrorCategory::Deadline,
        "openrouter media request was cancelled or exceeded its deadline",
    )
}

fn validate_endpoint(value: &str) -> Result<(), OpenRouterMediaError> {
    let Some((scheme, rest)) = value.split_once("://") else {
        return Err(OpenRouterMediaError::EndpointInvalid {
            reason: "endpoint must be an http or https URL",
        });
    };
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return Err(OpenRouterMediaError::EndpointInvalid {
            reason: "endpoint must be an http or https URL",
        });
    }
    if rest.contains('@') || rest.contains('?') || rest.contains('#') {
        return Err(OpenRouterMediaError::EndpointInvalid {
            reason: "endpoint contains forbidden components",
        });
    }
    let host = endpoint_host(rest).ok_or(OpenRouterMediaError::EndpointInvalid {
        reason: "endpoint host is missing",
    })?;
    if scheme.eq_ignore_ascii_case("http") && !is_loopback_host(host) {
        return Err(OpenRouterMediaError::EndpointInvalid {
            reason: "plaintext HTTP is allowed only for loopback endpoints",
        });
    }
    Ok(())
}

fn validate_download_url(value: &str, endpoint_is_loopback: bool) -> Result<(), ToolError> {
    let Some((scheme, rest)) = value.split_once("://") else {
        return Err(invalid_arguments(
            "openrouter media audio_url must be an http or https URL",
        ));
    };
    // Query strings are allowed: signed download URLs (e.g. presigned S3 or
    // GCS links) are a normal shape for caller-supplied audio_url values.
    if rest.contains('@') || rest.contains('#') {
        return Err(invalid_arguments(
            "openrouter media audio_url contains forbidden components",
        ));
    }
    let host = endpoint_host(rest)
        .ok_or_else(|| invalid_arguments("openrouter media audio_url host is missing"))?;
    if scheme.eq_ignore_ascii_case("https") {
        return Ok(());
    }
    if scheme.eq_ignore_ascii_case("http") && endpoint_is_loopback && is_loopback_host(host) {
        return Ok(());
    }
    Err(invalid_arguments(
        "openrouter media audio_url must be https (plaintext HTTP is allowed only for loopback fixtures when the toolset endpoint is also loopback)",
    ))
}

fn endpoint_host(rest: &str) -> Option<&str> {
    if let Some(rest) = rest.strip_prefix('[') {
        return rest.split(']').next().filter(|host| !host.is_empty());
    }
    rest.split(['/', ':'])
        .next()
        .filter(|host| !host.is_empty())
}

fn is_loopback_host(host: &str) -> bool {
    let host = host
        .strip_prefix('[')
        .map_or(host, |rest| rest.strip_suffix(']').unwrap_or(rest));
    host.eq_ignore_ascii_case("localhost")
        || host.parse::<IpAddr>().is_ok_and(|addr| addr.is_loopback())
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

fn tool_error(code: &'static str, category: ErrorCategory, message: &'static str) -> ToolError {
    ToolError::try_new(code, category, false, message, Metadata::empty()).unwrap_or_else(Into::into)
}

/// Reject an unsuccessful media response, naming the status and the endpoint's
/// own reason.
///
/// The message is the model's only self-correction signal. A status alone does
/// not say which argument was wrong, so the model reissues the identical call;
/// because these tools are approval-gated, every retry also re-prompts the
/// caller, and the run burns its cycles without progressing.
async fn endpoint_rejected(
    what: &'static str,
    status: reqwest::StatusCode,
    response: reqwest::Response,
) -> ToolError {
    let message = match rejection_detail(response).await {
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
async fn rejection_detail(response: reqwest::Response) -> Option<String> {
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(Ok(chunk)) = stream.next().await {
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use finstack_ai_kernel::{
        Digest, EffectId, EffectOutputContract, EffectOutputKind, LaneId, Metadata,
        OperationLocator, PrincipalRef, RawJson, RunId, SessionId, ToolBatchId, ToolCallBlock,
        ToolCallId, ToolFailurePolicy, ValidatedToolCall,
    };
    use finstack_ai_runtime::{AuthorizationContext, CancellationSignal, RunCallContext, Toolset};
    use futures_util::StreamExt;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::sync::mpsc;

    use super::{
        IMAGE_TOOL_NAME, OPENROUTER_MEDIA_CREDENTIAL_REQUIRED, OpenRouterMediaConfig,
        OpenRouterMediaError, OpenRouterMediaToolset, SPEECH_TOOL_NAME, TRANSCRIBE_TOOL_NAME,
        VIDEO_STATUS_TOOL_NAME, VIDEO_TOOL_NAME, validate_download_url,
    };

    use finstack_ai_context_memory::InProcessArtifactStore;

    use base64::Engine as _;

    use crate::{ArtifactStore, BASE64_STANDARD};

    const CANARY: &str = "or-media-secret-canary-046";

    fn base_config(endpoint: String) -> OpenRouterMediaConfig {
        OpenRouterMediaConfig {
            api_key: CANARY.into(),
            endpoint,
            referer: None,
            title: None,
            max_result_bytes: 256 * 1_024,
        }
    }

    fn tool_context() -> crate::ToolCallContext {
        crate::ToolCallContext {
            run: RunCallContext {
                locator: OperationLocator::try_new(
                    "tenant-a",
                    SessionId::from_bytes([1; 16]),
                    LaneId::from_bytes([2; 16]),
                    RunId::from_bytes([3; 16]),
                )
                .expect("locator"),
                authorization: AuthorizationContext {
                    principal: PrincipalRef::try_new("issuer", "subject", Some("tenant-a"))
                        .expect("principal"),
                    authentication_method: Arc::from("test"),
                    assurance_level: Arc::from("test"),
                    roles: Arc::from([]),
                    permitted_scopes: Arc::from([Arc::from("tenant-a")]),
                    safe_claims: Metadata::empty(),
                    policy_version: Arc::from("policy-v1"),
                    decision_id: Arc::from("decision-v1"),
                },
                effect_id: EffectId::from_bytes([4; 16]),
                attempt: 1,
                deadline: None,
                budget_scope_id: None,
                cancellation: CancellationSignal::new(),
            },
            tool_batch_id: ToolBatchId::from_bytes([5; 16]),
            tool_call_id: ToolCallId::from_bytes([6; 16]),
        }
    }

    fn call_for(spec: &crate::ToolSpec, args: &[u8]) -> ValidatedToolCall {
        ValidatedToolCall {
            call: ToolCallBlock::try_new(
                ToolCallId::from_bytes([6; 16]),
                spec.model_name.as_ref(),
                RawJson::parse(args).expect("args"),
            )
            .expect("call"),
            tool_id: spec.id.clone(),
            component: None,
            output_contract: EffectOutputContract {
                kind: EffectOutputKind::ToolResult,
                schema_version: 1,
                schema_digest: Digest::raw_json(b"{}"),
            },
            retry_safety: spec.retry_safety,
            deadline: None,
            execution: spec.execution,
            failure_policy: ToolFailurePolicy::ReturnToModel,
        }
    }

    fn find_spec(tools: &[crate::ToolSpec], name: &str) -> crate::ToolSpec {
        tools
            .iter()
            .find(|spec| spec.model_name.as_ref() == name)
            .cloned()
            .expect("tool spec present")
    }

    async fn respond(
        listener: &TcpListener,
        seen: &mpsc::UnboundedSender<String>,
        status: u16,
        body: &[u8],
        content_type: &str,
    ) {
        let (mut stream, _) = listener.accept().await.expect("accept");
        let mut buf = vec![0_u8; 8_192];
        let n = stream.read(&mut buf).await.expect("read");
        seen.send(String::from_utf8_lossy(&buf[..n]).into_owned())
            .expect("seen");
        let reason = match status {
            200 => "OK",
            202 => "Accepted",
            _ => "Error",
        };
        let mut response = format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .into_bytes();
        response.extend_from_slice(body);
        stream.write_all(&response).await.expect("write");
        stream.shutdown().await.expect("shutdown");
    }

    #[test]
    fn construction_rejects_a_missing_api_key() {
        let error = OpenRouterMediaToolset::try_new(OpenRouterMediaConfig {
            api_key: String::new(),
            ..base_config("https://openrouter.ai".into())
        })
        .expect_err("missing key");
        assert_eq!(error, OpenRouterMediaError::CredentialRequired);
        assert!(
            error
                .to_string()
                .contains(OPENROUTER_MEDIA_CREDENTIAL_REQUIRED)
        );
    }

    #[test]
    fn construction_rejects_plaintext_non_loopback() {
        let error = OpenRouterMediaToolset::try_new(base_config("http://8.8.8.8".into()))
            .expect_err("plaintext");
        assert!(error.to_string().contains("plaintext HTTP"));
        assert!(!error.to_string().contains(CANARY));
    }

    #[test]
    fn debug_does_not_leak_the_api_key() {
        let tools = OpenRouterMediaToolset::try_new(base_config("https://openrouter.ai".into()))
            .expect("tools");
        assert!(!format!("{tools:?}").contains(CANARY));
        let config = base_config("https://openrouter.ai".into());
        assert!(!format!("{config:?}").contains(CANARY));
    }

    #[test]
    fn construction_rejects_out_of_range_result_cap() {
        let error = OpenRouterMediaToolset::try_new(OpenRouterMediaConfig {
            max_result_bytes: 0,
            ..base_config("https://openrouter.ai".into())
        })
        .expect_err("zero cap");
        assert_eq!(
            error,
            OpenRouterMediaError::EndpointInvalid {
                reason: "result cap out of range"
            }
        );
    }

    #[tokio::test]
    async fn image_tool_returns_bounded_base64() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let (seen_tx, mut seen_rx) = mpsc::unbounded_channel();
        let server = tokio::spawn(async move {
            respond(
                &listener,
                &seen_tx,
                200,
                br#"{"data":[{"b64_json":"aGVsbG8=","media_type":"image/png"}]}"#,
                "application/json",
            )
            .await;
        });
        let tools =
            OpenRouterMediaToolset::try_new(base_config(format!("http://{addr}"))).expect("tools");
        let spec = find_spec(&tools.tools(), IMAGE_TOOL_NAME);
        let call = call_for(&spec, br#"{"model":"m","prompt":"a cat"}"#);
        let mut stream = tools
            .call(tool_context(), call)
            .await
            .expect("call started");
        let item = stream.next().await.expect("item").expect("ok");
        let crate::ToolStreamItem::Completed(result) = item else {
            panic!("expected completion");
        };
        assert!(!result.is_error);
        let payload: serde_json::Value =
            serde_json::from_slice(result.output.as_bytes()).expect("json");
        assert_eq!(payload["b64_data"], "aGVsbG8=");
        assert_eq!(payload["media_type"], "image/png");
        assert_eq!(payload["byte_length"], 5);
        let seen = seen_rx.recv().await.expect("request").to_ascii_lowercase();
        assert!(seen.contains("post /api/v1/images"));
        assert!(seen.contains(CANARY));
        server.await.expect("server");
    }

    #[tokio::test]
    async fn image_tool_enforces_the_result_cap() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let (seen_tx, _seen_rx) = mpsc::unbounded_channel();
        let big_b64 = "a".repeat(4_096);
        let body = format!(r#"{{"data":[{{"b64_json":"{big_b64}","media_type":"image/png"}}]}}"#);
        let server = tokio::spawn(async move {
            respond(
                &listener,
                &seen_tx,
                200,
                body.as_bytes(),
                "application/json",
            )
            .await;
        });
        let tools = OpenRouterMediaToolset::try_new(OpenRouterMediaConfig {
            max_result_bytes: 1_024,
            ..base_config(format!("http://{addr}"))
        })
        .expect("tools");
        let spec = find_spec(&tools.tools(), IMAGE_TOOL_NAME);
        let call = call_for(&spec, br#"{"model":"m","prompt":"a cat"}"#);
        let Err(error) = tools.call(tool_context(), call).await else {
            panic!("expected limit error");
        };
        assert_eq!(error.code(), crate::OPENROUTER_MEDIA_LIMIT_EXCEEDED);
        server.await.expect("server");
    }

    /// Generated media must not reach the model as inline base64.
    ///
    /// A model that receives hundreds of kilobytes of base64 it cannot read
    /// gains nothing actionable from the result, and these tools are
    /// approval-gated, so each reissued call re-prompts the caller.
    #[tokio::test]
    async fn a_configured_store_keeps_image_bytes_out_of_the_result() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let (seen_tx, _seen_rx) = mpsc::unbounded_channel();
        // Larger than max_result_bytes below: staging must not consult the cap.
        let payload = vec![7_u8; 8_192];
        let encoded = BASE64_STANDARD.encode(&payload);
        let body = format!(r#"{{"data":[{{"b64_json":"{encoded}","media_type":"image/png"}}]}}"#);
        let server = tokio::spawn(async move {
            respond(
                &listener,
                &seen_tx,
                200,
                body.as_bytes(),
                "application/json",
            )
            .await;
        });
        let store = Arc::new(InProcessArtifactStore::default());
        let tools = OpenRouterMediaToolset::try_new(OpenRouterMediaConfig {
            max_result_bytes: 1_024,
            ..base_config(format!("http://{addr}"))
        })
        .expect("tools")
        .with_artifact_store(Arc::clone(&store) as Arc<dyn ArtifactStore>);
        let spec = find_spec(&tools.tools(), IMAGE_TOOL_NAME);
        let call = call_for(&spec, br#"{"model":"m","prompt":"a cat"}"#);
        let mut stream = tools
            .call(tool_context(), call)
            .await
            .expect("call started");
        let item = stream.next().await.expect("item").expect("ok");
        let crate::ToolStreamItem::Completed(result) = item else {
            panic!("expected completion");
        };
        let value: serde_json::Value =
            serde_json::from_slice(result.output.as_bytes()).expect("json");
        assert!(
            value.get("b64_data").is_none(),
            "staged results must not inline the payload: {value}"
        );
        assert_eq!(value["byte_length"], 8_192);
        assert_eq!(value["media_type"], "image/png");
        assert!(
            !result
                .output
                .as_bytes()
                .windows(64)
                .any(|w| w == &encoded.as_bytes()[..64]),
            "the encoded payload must not appear anywhere in the result"
        );
        let artifact: finstack_ai_kernel::ArtifactRef =
            serde_json::from_value(value["artifact"].clone()).expect("artifact reference");
        assert_eq!(artifact.blob().media_type(), "image/png");
        server.await.expect("server");
    }

    /// A rejection has to carry the endpoint's reason, not just the status.
    ///
    /// These tools are approval-gated, and a model that cannot tell which
    /// argument was wrong reissues the identical call, re-prompting the caller
    /// on every attempt until the run exhausts its cycles.
    #[tokio::test]
    async fn rejection_surfaces_the_endpoint_reason() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let (seen_tx, _seen_rx) = mpsc::unbounded_channel();
        let server = tokio::spawn(async move {
            respond(
                &listener,
                &seen_tx,
                400,
                br#"{"error":{"message":"tts-1 is not a valid model ID","code":400}}"#,
                "application/json",
            )
            .await;
        });
        let tools =
            OpenRouterMediaToolset::try_new(base_config(format!("http://{addr}"))).expect("tools");
        let spec = find_spec(&tools.tools(), VIDEO_TOOL_NAME);
        let call = call_for(&spec, br#"{"model":"m","prompt":"a dog running"}"#);
        let Err(error) = tools.call(tool_context(), call).await else {
            panic!("expected rejection");
        };
        assert_eq!(error.code(), crate::OPENROUTER_MEDIA_TRANSPORT_FAILED);
        let message = error.message();
        assert!(message.contains("400"), "status missing from {message}");
        assert!(
            message.contains("tts-1 is not a valid model ID"),
            "endpoint reason missing from {message}"
        );
        server.await.expect("server");
    }

    #[tokio::test]
    async fn video_submit_returns_the_job_id() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let (seen_tx, mut seen_rx) = mpsc::unbounded_channel();
        let server = tokio::spawn(async move {
            respond(
                &listener,
                &seen_tx,
                202,
                br#"{"id":"vid-1","polling_url":"https://openrouter.test/api/v1/videos/vid-1","status":"pending"}"#,
                "application/json",
            )
            .await;
        });
        let tools =
            OpenRouterMediaToolset::try_new(base_config(format!("http://{addr}"))).expect("tools");
        let spec = find_spec(&tools.tools(), VIDEO_TOOL_NAME);
        let call = call_for(&spec, br#"{"model":"m","prompt":"a dog running"}"#);
        let mut stream = tools
            .call(tool_context(), call)
            .await
            .expect("call started");
        let item = stream.next().await.expect("item").expect("ok");
        let crate::ToolStreamItem::Completed(result) = item else {
            panic!("expected completion");
        };
        let payload: serde_json::Value =
            serde_json::from_slice(result.output.as_bytes()).expect("json");
        assert_eq!(payload["id"], "vid-1");
        assert_eq!(payload["status"], "pending");
        let seen = seen_rx.recv().await.expect("request").to_ascii_lowercase();
        assert!(seen.contains("post /api/v1/videos"));
        server.await.expect("server");
    }

    #[tokio::test]
    async fn video_status_returns_urls_when_completed() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let (seen_tx, mut seen_rx) = mpsc::unbounded_channel();
        let server = tokio::spawn(async move {
            respond(
                &listener,
                &seen_tx,
                200,
                br#"{"id":"vid-1","status":"completed","unsigned_urls":["https://openrouter.test/api/v1/videos/vid-1/content?index=0"]}"#,
                "application/json",
            )
            .await;
        });
        let tools =
            OpenRouterMediaToolset::try_new(base_config(format!("http://{addr}"))).expect("tools");
        let spec = find_spec(&tools.tools(), VIDEO_STATUS_TOOL_NAME);
        let call = call_for(&spec, br#"{"id":"vid-1"}"#);
        let mut stream = tools
            .call(tool_context(), call)
            .await
            .expect("call started");
        let item = stream.next().await.expect("item").expect("ok");
        let crate::ToolStreamItem::Completed(result) = item else {
            panic!("expected completion");
        };
        let payload: serde_json::Value =
            serde_json::from_slice(result.output.as_bytes()).expect("json");
        assert_eq!(payload["status"], "completed");
        assert_eq!(
            payload["urls"][0],
            "https://openrouter.test/api/v1/videos/vid-1/content?index=0"
        );
        let seen = seen_rx.recv().await.expect("request").to_ascii_lowercase();
        assert!(seen.contains("get /api/v1/videos/vid-1"));
        server.await.expect("server");
    }

    #[tokio::test]
    async fn video_status_wait_polls_until_terminal() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let (seen_tx, mut seen_rx) = mpsc::unbounded_channel();
        let server = tokio::spawn(async move {
            respond(
                &listener,
                &seen_tx,
                200,
                br#"{"id":"vid-1","status":"in_progress"}"#,
                "application/json",
            )
            .await;
            respond(
                &listener,
                &seen_tx,
                200,
                br#"{"id":"vid-1","status":"completed","unsigned_urls":["https://openrouter.test/x"]}"#,
                "application/json",
            )
            .await;
        });
        let tools =
            OpenRouterMediaToolset::try_new(base_config(format!("http://{addr}"))).expect("tools");
        let spec = find_spec(&tools.tools(), VIDEO_STATUS_TOOL_NAME);
        let call = call_for(&spec, br#"{"id":"vid-1","wait_seconds":30}"#);
        let mut stream = tools
            .call(tool_context(), call)
            .await
            .expect("call started");
        let item = stream.next().await.expect("item").expect("ok");
        let crate::ToolStreamItem::Completed(result) = item else {
            panic!("expected completion");
        };
        let payload: serde_json::Value =
            serde_json::from_slice(result.output.as_bytes()).expect("json");
        assert_eq!(payload["status"], "completed");
        assert_eq!(payload["urls"][0], "https://openrouter.test/x");
        let first = seen_rx.recv().await.expect("first").to_ascii_lowercase();
        let second = seen_rx.recv().await.expect("second").to_ascii_lowercase();
        assert!(first.contains("get /api/v1/videos/vid-1"));
        assert!(second.contains("get /api/v1/videos/vid-1"));
        server.await.expect("server");
    }

    #[tokio::test]
    async fn video_status_rejects_oversized_wait() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let (seen_tx, mut seen_rx) = mpsc::unbounded_channel::<String>();
        drop(seen_tx);
        drop(listener);
        let tools =
            OpenRouterMediaToolset::try_new(base_config(format!("http://{addr}"))).expect("tools");
        let spec = find_spec(&tools.tools(), VIDEO_STATUS_TOOL_NAME);
        let call = call_for(&spec, br#"{"id":"vid-1","wait_seconds":301}"#);
        let Err(error) = tools.call(tool_context(), call).await else {
            panic!("expected invalid arguments");
        };
        assert_eq!(error.code(), crate::OPENROUTER_MEDIA_INVALID_ARGUMENTS);
        assert!(
            seen_rx.try_recv().is_err(),
            "no HTTP must reach the fixture"
        );
    }

    #[tokio::test]
    async fn speech_tool_bounds_the_audio_payload() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let (seen_tx, _seen_rx) = mpsc::unbounded_channel();
        let big_body = vec![b'a'; 2_048];
        let server = tokio::spawn(async move {
            respond(&listener, &seen_tx, 200, &big_body, "audio/mpeg").await;
        });
        let tools = OpenRouterMediaToolset::try_new(OpenRouterMediaConfig {
            max_result_bytes: 1_024,
            ..base_config(format!("http://{addr}"))
        })
        .expect("tools");
        let spec = find_spec(&tools.tools(), SPEECH_TOOL_NAME);
        let call = call_for(&spec, br#"{"model":"m","input":"hello"}"#);
        let Err(error) = tools.call(tool_context(), call).await else {
            panic!("expected limit error");
        };
        assert_eq!(error.code(), crate::OPENROUTER_MEDIA_LIMIT_EXCEEDED);
        server.await.expect("server");
    }

    #[tokio::test]
    async fn speech_tool_asks_for_a_playable_container_by_default() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let (seen_tx, mut seen_rx) = mpsc::unbounded_channel();
        let server = tokio::spawn(async move {
            respond(&listener, &seen_tx, 200, b"audio", "audio/mpeg").await;
        });
        let tools =
            OpenRouterMediaToolset::try_new(base_config(format!("http://{addr}"))).expect("tools");
        let spec = find_spec(&tools.tools(), SPEECH_TOOL_NAME);
        let call = call_for(&spec, br#"{"model":"m","input":"hello"}"#);
        let mut stream = tools
            .call(tool_context(), call)
            .await
            .expect("call started");
        stream.next().await.expect("item").expect("ok");
        let seen = seen_rx.recv().await.expect("request");
        // The endpoint's own default is headerless PCM, which is bytes no
        // player opens as a file, so the tool asks for a container instead.
        assert!(
            seen.contains(r#""response_format":"mp3""#),
            "speech must request a playable container: {seen}"
        );
        server.await.expect("server");
    }

    #[tokio::test]
    async fn speech_tool_forwards_an_explicit_container() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let (seen_tx, mut seen_rx) = mpsc::unbounded_channel();
        let server = tokio::spawn(async move {
            respond(&listener, &seen_tx, 200, b"audio", "audio/pcm").await;
        });
        let tools =
            OpenRouterMediaToolset::try_new(base_config(format!("http://{addr}"))).expect("tools");
        let spec = find_spec(&tools.tools(), SPEECH_TOOL_NAME);
        let call = call_for(
            &spec,
            br#"{"model":"m","input":"hello","response_format":"pcm"}"#,
        );
        let mut stream = tools
            .call(tool_context(), call)
            .await
            .expect("call started");
        stream.next().await.expect("item").expect("ok");
        let seen = seen_rx.recv().await.expect("request");
        assert!(
            seen.contains(r#""response_format":"pcm""#),
            "an explicit container must survive the default: {seen}"
        );
        server.await.expect("server");
    }

    #[tokio::test]
    async fn transcribe_tool_downloads_then_submits_base64() {
        let audio_listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let audio_addr = audio_listener.local_addr().expect("addr");
        let (audio_tx, mut audio_rx) = mpsc::unbounded_channel();
        let audio_server = tokio::spawn(async move {
            respond(
                &audio_listener,
                &audio_tx,
                200,
                b"hello-audio-bytes",
                "audio/mpeg",
            )
            .await;
        });

        let api_listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let api_addr = api_listener.local_addr().expect("addr");
        let (api_tx, mut api_rx) = mpsc::unbounded_channel();
        let api_server = tokio::spawn(async move {
            respond(
                &api_listener,
                &api_tx,
                200,
                br#"{"text":"hello"}"#,
                "application/json",
            )
            .await;
        });

        let tools = OpenRouterMediaToolset::try_new(base_config(format!("http://{api_addr}")))
            .expect("tools");
        let spec = find_spec(&tools.tools(), TRANSCRIBE_TOOL_NAME);
        let args = format!(r#"{{"model":"m","audio_url":"http://{audio_addr}/audio.mp3"}}"#);
        let call = call_for(&spec, args.as_bytes());
        let mut stream = tools
            .call(tool_context(), call)
            .await
            .expect("call started");
        let item = stream.next().await.expect("item").expect("ok");
        let crate::ToolStreamItem::Completed(result) = item else {
            panic!("expected completion");
        };
        let payload: serde_json::Value =
            serde_json::from_slice(result.output.as_bytes()).expect("json");
        assert_eq!(payload["text"], "hello");
        let expected_b64 = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            b"hello-audio-bytes",
        );
        // Media hosts commonly answer an anonymous request with 403 rather
        // than the file: Wikimedia rejected a live download for exactly this.
        let fetched = audio_rx.recv().await.expect("download request");
        assert!(
            fetched.to_ascii_lowercase().contains("user-agent:"),
            "the audio download must identify the client: {fetched}"
        );
        let seen = api_rx.recv().await.expect("request");
        assert!(seen.contains(&expected_b64));
        assert!(
            seen.to_ascii_lowercase()
                .contains("post /api/v1/audio/transcriptions"),
            "transcription must use the transcriptions route: {seen}"
        );
        assert!(
            seen.contains(r#""input_audio":{"data":"#),
            "payload must be nested under input_audio: {seen}"
        );
        audio_server.await.expect("audio server");
        api_server.await.expect("api server");
    }

    #[tokio::test]
    async fn transcribe_tool_derives_format_from_a_presigned_url_ignoring_the_query_string() {
        let audio_listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let audio_addr = audio_listener.local_addr().expect("addr");
        let (audio_tx, _audio_rx) = mpsc::unbounded_channel();
        let audio_server = tokio::spawn(async move {
            respond(
                &audio_listener,
                &audio_tx,
                200,
                b"hello-audio-bytes",
                "audio/mpeg",
            )
            .await;
        });

        let api_listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let api_addr = api_listener.local_addr().expect("addr");
        let (api_tx, mut api_rx) = mpsc::unbounded_channel();
        let api_server = tokio::spawn(async move {
            respond(
                &api_listener,
                &api_tx,
                200,
                br#"{"text":"hello"}"#,
                "application/json",
            )
            .await;
        });

        let tools = OpenRouterMediaToolset::try_new(base_config(format!("http://{api_addr}")))
            .expect("tools");
        let spec = find_spec(&tools.tools(), TRANSCRIBE_TOOL_NAME);
        // A presigned-style URL: the real extension is followed by a query
        // string carrying an unrelated signature parameter.
        let args = format!(r#"{{"model":"m","audio_url":"http://{audio_addr}/a.mp3?X-Sig=abc"}}"#);
        let call = call_for(&spec, args.as_bytes());
        let mut stream = tools
            .call(tool_context(), call)
            .await
            .expect("call started");
        let item = stream.next().await.expect("item").expect("ok");
        let crate::ToolStreamItem::Completed(result) = item else {
            panic!("expected completion");
        };
        let payload: serde_json::Value =
            serde_json::from_slice(result.output.as_bytes()).expect("json");
        assert_eq!(payload["text"], "hello");
        let seen = api_rx.recv().await.expect("request");
        assert!(
            seen.contains(r#""format":"mp3""#),
            "format must be derived from the path, not the query string: {seen}"
        );
        audio_server.await.expect("audio server");
        api_server.await.expect("api server");
    }

    #[tokio::test]
    async fn transcribe_tool_rejects_plaintext_download_urls() {
        let tools = OpenRouterMediaToolset::try_new(base_config("https://openrouter.ai".into()))
            .expect("tools");
        let spec = find_spec(&tools.tools(), TRANSCRIBE_TOOL_NAME);
        let call = call_for(
            &spec,
            br#"{"model":"m","audio_url":"http://8.8.8.8/a.mp3"}"#,
        );
        let Err(error) = tools.call(tool_context(), call).await else {
            panic!("expected invalid arguments");
        };
        assert_eq!(error.code(), crate::OPENROUTER_MEDIA_INVALID_ARGUMENTS);
    }

    #[test]
    fn validate_download_url_accepts_https_query_strings() {
        // Signed download URLs (presigned S3/GCS links) are a normal shape
        // for a caller-supplied audio_url; the query string must not be
        // rejected as a "forbidden component".
        validate_download_url(
            "https://example-bucket.s3.amazonaws.com/a.mp3?X-Amz-Signature=abc123&X-Amz-Expires=900",
            false,
        )
        .expect("https download url with a query string is accepted");
    }

    #[tokio::test]
    async fn cancelled_call_does_not_reach_the_fixture() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let (seen_tx, mut seen_rx) = mpsc::unbounded_channel();
        let server = tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = vec![0_u8; 256];
                let n = stream.read(&mut buf).await.unwrap_or(0);
                let _ = seen_tx.send(String::from_utf8_lossy(&buf[..n]).into_owned());
            }
        });
        let tools =
            OpenRouterMediaToolset::try_new(base_config(format!("http://{addr}"))).expect("tools");
        let spec = find_spec(&tools.tools(), IMAGE_TOOL_NAME);
        let ctx = tool_context();
        ctx.run.cancellation.cancel();
        let call = call_for(&spec, br#"{"model":"m","prompt":"a cat"}"#);
        let Err(error) = tools.call(ctx, call).await else {
            panic!("cancelled");
        };
        assert_eq!(error.code(), crate::OPENROUTER_MEDIA_TIMEOUT);
        assert!(
            seen_rx.try_recv().is_err(),
            "no HTTP must reach the fixture"
        );
        server.abort();
    }

    #[test]
    fn media_tool_input_schemas_are_openai_strict_compatible() {
        let optional = [
            (
                IMAGE_TOOL_NAME,
                &["aspect_ratio", "output_format", "resolution"][..],
            ),
            (
                VIDEO_TOOL_NAME,
                &["aspect_ratio", "duration", "resolution"][..],
            ),
            (VIDEO_STATUS_TOOL_NAME, &["wait_seconds"][..]),
            (SPEECH_TOOL_NAME, &["voice"][..]),
            (TRANSCRIBE_TOOL_NAME, &["format"][..]),
        ];
        let tools = OpenRouterMediaToolset::try_new(base_config("https://openrouter.ai".into()))
            .expect("tools");
        for (name, optional_keys) in optional {
            let spec = find_spec(&tools.tools(), name);
            let schema: serde_json::Value =
                serde_json::from_slice(spec.input_schema.as_bytes()).expect("schema");
            let properties = schema["properties"].as_object().expect("properties");
            let required = schema["required"]
                .as_array()
                .expect("required")
                .iter()
                .filter_map(|value| value.as_str())
                .collect::<std::collections::BTreeSet<_>>();
            assert_eq!(schema["additionalProperties"], false);
            for key in properties.keys() {
                assert!(
                    required.contains(key.as_str()),
                    "{name} omits {key} from required"
                );
            }
            for key in optional_keys {
                let empty = Vec::new();
                let types = schema["properties"][key]["type"]
                    .as_array()
                    .unwrap_or(&empty)
                    .iter()
                    .filter_map(|value| value.as_str())
                    .collect::<Vec<_>>();
                assert!(
                    types.contains(&"null"),
                    "{name} {key} must be nullable for OpenAI strict mode"
                );
            }
        }
    }

    #[test]
    fn openrouter_media_is_not_a_wasm_host_sdk_dependency() {
        let manifest = include_str!("../../../../crates/finstack-ai/Cargo.toml");
        let wasm_host = manifest
            .lines()
            .find(|line| line.contains("wasm-host ="))
            .expect("wasm-host feature");
        assert!(
            !wasm_host.contains("finstack-ai-tools-openrouter-media"),
            "the openrouter media toolset must stay off the wasm-host feature graph"
        );
        assert!(manifest.contains("dep:finstack-ai-tools-openrouter-media"));
    }
}
