//! T1 native `OpenAI` media-generation Toolset.
//!
//! Exposes `OpenAI`'s image, speech, and transcription endpoints as bounded
//! agent tools. Construction requires an explicit API key and never reads
//! environment variables. Non-loopback endpoints must be HTTPS. Tool results
//! are bounded JSON: hosted image URLs are preferred over base64 payloads,
//! and base64 payloads are returned only when they fit the result cap.

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
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use finstack_ai_kernel::{
    ErrorCategory, Metadata, RawJson, RetrySafety, Sensitivity, Timestamp, ToolExecutionMode,
    ToolId, ValidatedToolCall,
};
use finstack_ai_net_guard::{
    NetGuardError, SystemResolver, UrlPolicy, parse_and_vet_url, pinned_client, read_body_bounded,
    reject_literal_destination, resolve_and_pin,
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

const DEFAULT_ENDPOINT: &str = "https://api.openai.com";
const REQUEST_TIMEOUT: Duration = Duration::from_mins(2);
#[allow(dead_code)]
const DEFAULT_MAX_RESULT_BYTES: usize = 256 * 1_024;
const MAX_RESULT_BYTES_CEILING: usize = 8 * 1_048_576;
// OpenAI documents an audio-file upload limit in the 26 MB region.
const MAX_AUDIO_DOWNLOAD_BYTES: usize = 26 * 1_048_576;

const IMAGE_TOOL_ID: &str = "finstack.tools.openai_generate_image";
const IMAGE_TOOL_NAME: &str = "openai_generate_image";
const SPEECH_TOOL_ID: &str = "finstack.tools.openai_generate_speech";
const SPEECH_TOOL_NAME: &str = "openai_generate_speech";
const TRANSCRIBE_TOOL_ID: &str = "finstack.tools.openai_transcribe_audio";
const TRANSCRIBE_TOOL_NAME: &str = "openai_transcribe_audio";

/// Stable missing-credential code.
pub const OPENAI_MEDIA_CREDENTIAL_REQUIRED: &str = "openai_media_credential_required";
/// Stable endpoint-configuration code.
pub const OPENAI_MEDIA_ENDPOINT_INVALID: &str = "openai_media_endpoint_invalid";
/// Stable argument-validation code.
pub const OPENAI_MEDIA_INVALID_ARGUMENTS: &str = "openai_media_invalid_arguments";
/// Stable remote-transport code.
pub const OPENAI_MEDIA_TRANSPORT_FAILED: &str = "openai_media_transport_failed";
/// Stable output-limit code.
pub const OPENAI_MEDIA_LIMIT_EXCEEDED: &str = "openai_media_limit_exceeded";
/// Stable cancellation/deadline code.
pub const OPENAI_MEDIA_TIMEOUT: &str = "openai_media_timeout";

/// Explicit `OpenAI` media route. Never populated from the environment.
#[derive(Clone)]
pub struct OpenAiMediaConfig {
    /// Explicit API key. Empty values fail closed.
    pub api_key: String,
    /// HTTPS endpoint, or loopback HTTP for scripted fixtures. Empty selects
    /// `https://api.openai.com`.
    pub endpoint: String,
    /// Result-size cap in bytes. Images and speech come back base64, so
    /// hosts wanting inline media raise this. Zero or above 8 MiB fails
    /// construction; the SDK default is `262_144`.
    pub max_result_bytes: usize,
}

impl std::fmt::Debug for OpenAiMediaConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiMediaConfig")
            .field("api_key", &"[redacted]")
            .field("endpoint", &self.endpoint)
            .field("max_result_bytes", &self.max_result_bytes)
            .finish()
    }
}

/// Construction or route failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum OpenAiMediaError {
    /// API key was omitted.
    #[error(
        "{OPENAI_MEDIA_CREDENTIAL_REQUIRED}: openai media construction requires an explicit API key"
    )]
    CredentialRequired,
    /// Endpoint scheme, host, components, or result cap are invalid.
    #[error("{OPENAI_MEDIA_ENDPOINT_INVALID}: {reason}")]
    EndpointInvalid {
        /// Stable non-secret reason.
        reason: &'static str,
    },
}

/// T1 native `OpenAI` media-generation Toolset.
pub struct OpenAiMediaToolset {
    descriptor: ToolsetDescriptor,
    tools: Arc<[ToolSpec]>,
    image_tool_id: ToolId,
    speech_tool_id: ToolId,
    transcribe_tool_id: ToolId,
    api_key: String,
    endpoint: String,
    max_result_bytes: usize,
    artifact_store: Option<Arc<dyn ArtifactStore>>,
    client: reqwest::Client,
}

impl std::fmt::Debug for OpenAiMediaToolset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiMediaToolset")
            .field("endpoint", &self.endpoint)
            .field("max_result_bytes", &self.max_result_bytes)
            .field("artifact_store", &self.artifact_store.is_some())
            .finish_non_exhaustive()
    }
}

impl OpenAiMediaToolset {
    /// Construct the Toolset after validating the explicit route.
    ///
    /// # Errors
    ///
    /// Returns [`OpenAiMediaError::CredentialRequired`] when `api_key` is empty.
    /// Returns [`OpenAiMediaError::EndpointInvalid`] for a non-HTTP URL, userinfo,
    /// query, fragment, plaintext HTTP off loopback, or an out-of-range result cap.
    #[allow(clippy::too_many_lines)]
    pub fn try_new(config: OpenAiMediaConfig) -> Result<Self, OpenAiMediaError> {
        if config.api_key.is_empty() {
            return Err(OpenAiMediaError::CredentialRequired);
        }
        if config.max_result_bytes == 0 || config.max_result_bytes > MAX_RESULT_BYTES_CEILING {
            return Err(OpenAiMediaError::EndpointInvalid {
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

        let max_result_bytes_u64 = u64::try_from(config.max_result_bytes).unwrap_or(u64::MAX);

        let image_tool_id =
            ToolId::parse(IMAGE_TOOL_ID).map_err(|_| OpenAiMediaError::EndpointInvalid {
                reason: "invalid_tool_id",
            })?;
        let speech_tool_id =
            ToolId::parse(SPEECH_TOOL_ID).map_err(|_| OpenAiMediaError::EndpointInvalid {
                reason: "invalid_tool_id",
            })?;
        let transcribe_tool_id =
            ToolId::parse(TRANSCRIBE_TOOL_ID).map_err(|_| OpenAiMediaError::EndpointInvalid {
                reason: "invalid_tool_id",
            })?;

        let paid_approval = ApprovalMetadata {
            requirement: ApprovalRequirement::Policy,
            reason: Some(Arc::from("paid OpenAI media generation")),
            attributes: Metadata::empty(),
        };

        let image_spec = ToolSpec {
            id: image_tool_id.clone(),
            model_name: Arc::from(IMAGE_TOOL_NAME),
            title: Arc::from("OpenAI generate image"),
            description: Arc::from(
                "Generate an image via OpenAI and return the first data[] item only (a hosted URL when the model provides one, otherwise base64 or a staged artifact). Additional images in the response are discarded.",
            ),
            input_schema: RawJson::parse(
                br#"{"additionalProperties":false,"properties":{"model":{"minLength":1,"type":"string"},"prompt":{"minLength":1,"type":"string"},"size":{"type":["string","null"]}},"required":["model","prompt","size"],"type":"object"}"#,
            )
            .map_err(|_| OpenAiMediaError::EndpointInvalid {
                reason: "invalid_input_schema",
            })?,
            output_schema: Some(
                RawJson::parse(
                    br#"{"additionalProperties":false,"properties":{"artifact":{"description":"Staged artifact reference holding the image bytes. Present when the host configured an artifact store and the model returned base64.","type":"object"},"b64_json":{"type":"string"},"byte_length":{"type":"integer"},"media_type":{"type":"string"},"url":{"type":"string"}},"type":"object"}"#,
                )
                .map_err(|_| OpenAiMediaError::EndpointInvalid {
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
            .map_err(|_| OpenAiMediaError::EndpointInvalid {
                reason: "invalid_tool_spec",
            })?;

        let speech_spec = ToolSpec {
            id: speech_tool_id.clone(),
            model_name: Arc::from(SPEECH_TOOL_NAME),
            title: Arc::from("OpenAI generate speech"),
            description: Arc::from("Synthesize speech from text via OpenAI."),
            input_schema: RawJson::parse(
                br#"{"additionalProperties":false,"properties":{"input":{"minLength":1,"type":"string"},"model":{"minLength":1,"type":"string"},"voice":{"type":["string","null"]}},"required":["model","input","voice"],"type":"object"}"#,
            )
            .map_err(|_| OpenAiMediaError::EndpointInvalid {
                reason: "invalid_input_schema",
            })?,
            output_schema: Some(
                RawJson::parse(
                    br#"{"additionalProperties":false,"properties":{"artifact":{"description":"Staged artifact reference holding the audio bytes. Present when the host configured an artifact store.","type":"object"},"b64_audio":{"description":"Base64 audio bytes. Present only when no artifact store is configured.","type":"string"},"byte_length":{"type":"integer"},"media_type":{"type":"string"}},"required":["media_type"],"type":"object"}"#,
                )
                .map_err(|_| OpenAiMediaError::EndpointInvalid {
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
            .map_err(|_| OpenAiMediaError::EndpointInvalid {
                reason: "invalid_tool_spec",
            })?;

        let transcribe_spec = ToolSpec {
            id: transcribe_tool_id.clone(),
            model_name: Arc::from(TRANSCRIBE_TOOL_NAME),
            title: Arc::from("OpenAI transcribe audio"),
            description: Arc::from(
                "Transcribe audio at an HTTPS URL via OpenAI (the toolset downloads it, then uploads it as multipart/form-data).",
            ),
            input_schema: RawJson::parse(
                br#"{"additionalProperties":false,"properties":{"audio_url":{"minLength":1,"type":"string"},"model":{"minLength":1,"type":"string"}},"required":["model","audio_url"],"type":"object"}"#,
            )
            .map_err(|_| OpenAiMediaError::EndpointInvalid {
                reason: "invalid_input_schema",
            })?,
            output_schema: Some(
                RawJson::parse(
                    br#"{"additionalProperties":false,"properties":{"text":{"type":"string"}},"required":["text"],"type":"object"}"#,
                )
                .map_err(|_| OpenAiMediaError::EndpointInvalid {
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
            .map_err(|_| OpenAiMediaError::EndpointInvalid {
                reason: "invalid_tool_spec",
            })?;

        // This client serves the toolset's own OpenAI API calls (image and
        // speech generation, plus the transcription POST after the audio
        // is downloaded) — NOT the caller-supplied audio_url download,
        // which builds its own address-pinned client per request via
        // `finstack_ai_net_guard::pinned_client` (see `download_bytes`).
        // Redirects stay disabled here too, defense in depth: the
        // configured `endpoint` is not caller-supplied, but there is no
        // reason for a same-provider API call to ever redirect either.
        let client = reqwest::Client::builder()
            .http1_only()
            .timeout(REQUEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| OpenAiMediaError::EndpointInvalid {
                reason: "http_client",
            })?;

        Ok(Self {
            descriptor: ToolsetDescriptor {
                name: Arc::from("finstack-openai-media"),
                metadata: Metadata::empty(),
            },
            tools: Arc::from([image_spec, speech_spec, transcribe_spec]),
            image_tool_id,
            speech_tool_id,
            transcribe_tool_id,
            api_key: config.api_key,
            endpoint,
            max_result_bytes: config.max_result_bytes,
            artifact_store: None,
            client,
        })
    }

    /// Stage generated audio and images instead of inlining them.
    ///
    /// With a store attached, speech and base64 image results carry an
    /// `artifact` reference and the model never receives the base64 payload.
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
    size: Option<String>,
}

#[derive(Deserialize)]
struct ImageResponseItem {
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    b64_json: Option<String>,
}

#[derive(Deserialize)]
struct ImageResponse {
    data: Vec<ImageResponseItem>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SpeechArguments {
    model: String,
    input: String,
    #[serde(default)]
    voice: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TranscribeArguments {
    model: String,
    audio_url: String,
}

#[derive(Deserialize)]
struct TranscribeResponse {
    text: String,
}

impl Toolset for OpenAiMediaToolset {
    fn descriptor(&self) -> ToolsetDescriptor {
        self.descriptor.clone()
    }

    fn tools(&self) -> Arc<[ToolSpec]> {
        Arc::clone(&self.tools)
    }

    fn call(
        &self,
        ctx: ToolCallContext,
        call: ValidatedToolCall,
    ) -> PortFuture<Result<ToolEventStream, ToolError>> {
        let client = self.client.clone();
        let api_key = self.api_key.clone();
        let endpoint = self.endpoint.clone();
        let max_result_bytes = self.max_result_bytes;
        let artifact_store = self.artifact_store.clone();
        let image_tool_id = self.image_tool_id.clone();
        let speech_tool_id = self.speech_tool_id.clone();
        let transcribe_tool_id = self.transcribe_tool_id.clone();
        Box::pin(async move {
            verify_authority(&ctx)?;
            let tool_name = call.call.tool_name();
            let value = if call.tool_id == image_tool_id && tool_name == IMAGE_TOOL_NAME {
                handle_image(
                    &client,
                    &api_key,
                    &endpoint,
                    max_result_bytes,
                    artifact_store.as_ref(),
                    &ctx,
                    call.call.arguments().as_bytes(),
                )
                .await?
            } else if call.tool_id == speech_tool_id && tool_name == SPEECH_TOOL_NAME {
                handle_speech(
                    &client,
                    &api_key,
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
                    &endpoint,
                    &ctx,
                    call.call.arguments().as_bytes(),
                )
                .await?
            } else {
                return Err(tool_error(
                    OPENAI_MEDIA_INVALID_ARGUMENTS,
                    ErrorCategory::Validation,
                    "openai media call identity is invalid",
                ));
            };
            let output_bytes = serde_json::to_vec(&value).map_err(|_| {
                tool_error(
                    OPENAI_MEDIA_TRANSPORT_FAILED,
                    ErrorCategory::Internal,
                    "openai media result serialization failed",
                )
            })?;
            let result = ToolResult {
                output: RawJson::parse(output_bytes).map_err(|_| {
                    tool_error(
                        OPENAI_MEDIA_TRANSPORT_FAILED,
                        ErrorCategory::Internal,
                        "openai media result normalization failed",
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
        OPENAI_MEDIA_INVALID_ARGUMENTS,
        ErrorCategory::Validation,
        message,
    )
}

fn parse_arguments<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T, ToolError> {
    serde_json::from_slice(bytes)
        .map_err(|_| invalid_arguments("openai media arguments are invalid"))
}

fn base64_encoded_len(byte_length: usize) -> usize {
    byte_length.div_ceil(3).saturating_mul(4)
}

/// Hand generated media back without inlining the bytes when a store is set.
async fn deliver_media(
    bytes: Vec<u8>,
    media_type: &str,
    inline_field: &'static str,
    name: &'static str,
    store: Option<&Arc<dyn ArtifactStore>>,
    ctx: &ToolCallContext,
    max_result_bytes: usize,
) -> Result<serde_json::Value, ToolError> {
    let byte_length = bytes.len();
    let Some(store) = store else {
        if base64_encoded_len(byte_length) > max_result_bytes {
            return Err(tool_error(
                OPENAI_MEDIA_LIMIT_EXCEEDED,
                ErrorCategory::Limit,
                "openai media result exceeds the configured byte limit",
            ));
        }
        return Ok(serde_json::json!({
            inline_field: BASE64_STANDARD.encode(bytes),
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
            OPENAI_MEDIA_LIMIT_EXCEEDED,
            ErrorCategory::Tool,
            "openai media artifact staging failed",
        )
    })?;
    Ok(serde_json::json!({
        "artifact": artifact,
        "media_type": media_type,
        "byte_length": byte_length,
    }))
}

async fn handle_image(
    client: &reqwest::Client,
    api_key: &str,
    endpoint: &str,
    max_result_bytes: usize,
    store: Option<&Arc<dyn ArtifactStore>>,
    ctx: &ToolCallContext,
    arguments: &[u8],
) -> Result<serde_json::Value, ToolError> {
    let arguments: ImageArguments = parse_arguments(arguments)?;
    if arguments.model.is_empty() || arguments.prompt.is_empty() {
        return Err(invalid_arguments("openai media model or prompt is empty"));
    }
    let mut body = serde_json::json!({
        "model": arguments.model,
        "prompt": arguments.prompt,
    });
    if let Some(map) = body.as_object_mut()
        && let Some(size) = arguments.size
    {
        map.insert("size".into(), serde_json::Value::String(size));
    }
    let response: ImageResponse = send_json(
        client,
        api_key,
        reqwest::Method::POST,
        &format!("{endpoint}/v1/images/generations"),
        Some(&body),
        ctx,
        MAX_RESULT_BYTES_CEILING,
    )
    .await?;
    let item = response.data.into_iter().next().ok_or_else(|| {
        tool_error(
            OPENAI_MEDIA_TRANSPORT_FAILED,
            ErrorCategory::Tool,
            "openai media image response omitted image data",
        )
    })?;
    if let Some(url) = item.url {
        let value = serde_json::json!({ "url": url });
        let size = serde_json::to_vec(&value).map_or(usize::MAX, |bytes| bytes.len());
        if size > max_result_bytes {
            return Err(tool_error(
                OPENAI_MEDIA_LIMIT_EXCEEDED,
                ErrorCategory::Limit,
                "openai media image result exceeds the configured byte limit",
            ));
        }
        return Ok(value);
    }
    let Some(b64_json) = item.b64_json else {
        return Err(tool_error(
            OPENAI_MEDIA_TRANSPORT_FAILED,
            ErrorCategory::Tool,
            "openai media image response omitted url and b64_json",
        ));
    };
    let bytes = BASE64_STANDARD.decode(b64_json).map_err(|_| {
        tool_error(
            OPENAI_MEDIA_TRANSPORT_FAILED,
            ErrorCategory::Tool,
            "openai media image data is not valid base64",
        )
    })?;
    deliver_media(
        bytes,
        "image/png",
        "b64_json",
        "openai-image",
        store,
        ctx,
        max_result_bytes,
    )
    .await
}

async fn handle_speech(
    client: &reqwest::Client,
    api_key: &str,
    endpoint: &str,
    max_result_bytes: usize,
    store: Option<&Arc<dyn ArtifactStore>>,
    ctx: &ToolCallContext,
    arguments: &[u8],
) -> Result<serde_json::Value, ToolError> {
    let arguments: SpeechArguments = parse_arguments(arguments)?;
    if arguments.model.is_empty() || arguments.input.is_empty() {
        return Err(invalid_arguments("openai media model or input is empty"));
    }
    let mut body = serde_json::json!({
        "model": arguments.model,
        "input": arguments.input,
    });
    if let Some(map) = body.as_object_mut()
        && let Some(voice) = arguments.voice
    {
        map.insert("voice".into(), serde_json::Value::String(voice));
    }
    let (bytes, content_type) = send_bytes(
        client,
        api_key,
        reqwest::Method::POST,
        &format!("{endpoint}/v1/audio/speech"),
        Some(&body),
        ctx,
        if store.is_some() {
            MAX_RESULT_BYTES_CEILING
        } else {
            (max_result_bytes / 4).saturating_mul(3)
        },
    )
    .await?;
    let media_type = content_type.unwrap_or_else(|| "audio/mpeg".to_owned());
    deliver_media(
        bytes,
        &media_type,
        "b64_audio",
        "openai-speech",
        store,
        ctx,
        max_result_bytes,
    )
    .await
}

async fn handle_transcribe(
    client: &reqwest::Client,
    api_key: &str,
    endpoint: &str,
    ctx: &ToolCallContext,
    arguments: &[u8],
) -> Result<serde_json::Value, ToolError> {
    let arguments: TranscribeArguments = parse_arguments(arguments)?;
    if arguments.model.is_empty() || arguments.audio_url.is_empty() {
        return Err(invalid_arguments(
            "openai media model or audio_url is empty",
        ));
    }
    validate_download_url(&arguments.audio_url)?;
    let downloaded = download_bytes(&arguments.audio_url, ctx, MAX_AUDIO_DOWNLOAD_BYTES).await?;
    // Presigned/signed download URLs commonly carry a query string (and
    // occasionally a fragment) after the real file name, e.g.
    // `https://bucket.example/a.mp3?X-Sig=...`; strip both before deriving
    // the file name so the signature doesn't leak into the multipart part.
    let path = arguments
        .audio_url
        .split(['?', '#'])
        .next()
        .unwrap_or(arguments.audio_url.as_str());
    let file_name = path
        .rsplit('/')
        .next()
        .filter(|segment| !segment.is_empty())
        .unwrap_or("audio")
        .to_owned();

    if ctx.run.cancellation.is_cancelled() || deadline_elapsed(ctx.run.deadline) {
        return Err(timeout_error());
    }
    let form = reqwest::multipart::Form::new()
        .text("model", arguments.model)
        .part(
            "file",
            reqwest::multipart::Part::bytes(downloaded).file_name(file_name),
        );
    let request = client
        .post(format!("{endpoint}/v1/audio/transcriptions"))
        .header("Authorization", format!("Bearer {api_key}"))
        .multipart(form);
    let send = request.send();
    let response = tokio::select! {
        () = ctx.run.cancellation.cancelled() => return Err(timeout_error()),
        () = wait_deadline(ctx.run.deadline) => return Err(timeout_error()),
        result = send => result.map_err(|_| {
            tool_error(
                OPENAI_MEDIA_TRANSPORT_FAILED,
                ErrorCategory::Tool,
                "openai media request failed",
            )
        })?,
    };
    let status = response.status();
    if !status.is_success() {
        return Err(tool_error(
            OPENAI_MEDIA_TRANSPORT_FAILED,
            ErrorCategory::Tool,
            "openai media endpoint rejected the request",
        ));
    }
    let response: TranscribeResponse =
        read_bounded_json(response, MAX_RESULT_BYTES_CEILING).await?;
    Ok(serde_json::json!({ "text": response.text }))
}

async fn send_json<T: for<'de> Deserialize<'de>>(
    client: &reqwest::Client,
    api_key: &str,
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
                OPENAI_MEDIA_TRANSPORT_FAILED,
                ErrorCategory::Tool,
                "openai media request failed",
            )
        })?,
    };
    let status = response.status();
    if !status.is_success() {
        return Err(tool_error(
            OPENAI_MEDIA_TRANSPORT_FAILED,
            ErrorCategory::Tool,
            "openai media endpoint rejected the request",
        ));
    }
    read_bounded_json(response, cap).await
}

async fn send_bytes(
    client: &reqwest::Client,
    api_key: &str,
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
                OPENAI_MEDIA_TRANSPORT_FAILED,
                ErrorCategory::Tool,
                "openai media request failed",
            )
        })?,
    };
    let status = response.status();
    if !status.is_success() {
        return Err(tool_error(
            OPENAI_MEDIA_TRANSPORT_FAILED,
            ErrorCategory::Tool,
            "openai media endpoint rejected the request",
        ));
    }
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let bytes = fetch_bytes_bounded(response, cap).await?;
    Ok((bytes, content_type))
}

/// Address-pinned, vetted download of a caller-supplied `audio_url`.
///
/// Backed by `finstack-ai-net-guard`: `parse_and_vet_url` (scheme/component
/// vetting) &rarr; `resolve_and_pin` (DNS resolve-and-pin, private/loopback
/// deny) &rarr; `pinned_client` (address-pinned, redirect-disabled,
/// proxy-disabled). Unlike the crate's former unpinned `client.get(url)`
/// download (which reused the shared, unpinned `OpenAI` API client and
/// performed no destination vetting at all beyond the URL-string checks in
/// `validate_download_url`), this closes the DNS-rebinding/SSRF gap: a
/// hostname that resolves to a private or loopback address is now rejected
/// even when the URL string itself looked like a public HTTPS host.
async fn download_bytes(
    url: &str,
    ctx: &ToolCallContext,
    cap: usize,
) -> Result<Vec<u8>, ToolError> {
    if ctx.run.cancellation.is_cancelled() || deadline_elapsed(ctx.run.deadline) {
        return Err(timeout_error());
    }
    let policy = download_url_policy();
    let vetted = parse_and_vet_url(url, &policy).map_err(map_net_guard_error)?;
    reject_literal_destination(&vetted, &policy).map_err(map_net_guard_error)?;
    let addr = resolve_and_pin(&vetted, &SystemResolver)
        .await
        .map_err(map_net_guard_error)?;
    let client = pinned_client(&vetted, addr, REQUEST_TIMEOUT).map_err(map_net_guard_error)?;
    let send = client.get(vetted.url.as_str()).send();
    let response = tokio::select! {
        () = ctx.run.cancellation.cancelled() => return Err(timeout_error()),
        () = wait_deadline(ctx.run.deadline) => return Err(timeout_error()),
        result = send => result.map_err(|_| {
            tool_error(
                OPENAI_MEDIA_TRANSPORT_FAILED,
                ErrorCategory::Tool,
                "openai media audio download failed",
            )
        })?,
    };
    let status = response.status();
    if !status.is_success() {
        return Err(tool_error(
            OPENAI_MEDIA_TRANSPORT_FAILED,
            ErrorCategory::Tool,
            "openai media audio host rejected the request",
        ));
    }
    read_body_bounded(response, cap)
        .await
        .map_err(map_net_guard_error)
}

/// Loopback-http policy for caller-supplied download URLs: production is
/// HTTPS-only with no loopback exception, and the loopback allowance for
/// scripted `http://127.0.0.1` fixtures is compiled in only for
/// `#[cfg(test)]` builds, so that branch does not exist in a release
/// binary (same property the crate's former `validate_download_url` had).
/// Loopback-http policy for caller-supplied download URLs: production is
/// HTTPS-only with no loopback exception, and the loopback allowance for
/// scripted `http://127.0.0.1` fixtures is compiled in only for
/// `#[cfg(test)]` builds, so that branch does not exist in a release
/// binary (same property the crate's former `validate_download_url` had).
///
/// `allow_nonstandard_https_port: true` restores the crate's prior
/// any-port behavior: net-guard's own default is 443-only (right for a
/// model-supplied URL), but this crate's `audio_url` is a
/// caller-supplied, potentially presigned download URL, and a
/// self-hosted/non-standard-port host is a normal shape those take.
#[cfg(test)]
fn download_url_policy() -> UrlPolicy {
    UrlPolicy {
        allow_loopback_http: true,
        allow_nonstandard_https_port: true,
    }
}

#[cfg(not(test))]
fn download_url_policy() -> UrlPolicy {
    UrlPolicy {
        allow_loopback_http: false,
        allow_nonstandard_https_port: true,
    }
}

fn invalid_download_url() -> ToolError {
    invalid_arguments("openai media audio_url is not allowed")
}

/// Map every other net-guard failure (client construction, transport,
/// oversize body) onto the crate's existing transport/limit codes.
#[allow(clippy::needless_pass_by_value)] // used as a `map_err` function pointer
fn map_net_guard_error(error: NetGuardError) -> ToolError {
    match error {
        NetGuardError::InvalidUrl { .. }
        | NetGuardError::DestinationBlocked { .. }
        | NetGuardError::ResolutionFailed => invalid_download_url(),
        NetGuardError::ClientBuildFailed | NetGuardError::TransportFailed => tool_error(
            OPENAI_MEDIA_TRANSPORT_FAILED,
            ErrorCategory::Tool,
            "openai media audio download failed",
        ),
        NetGuardError::LimitExceeded => tool_error(
            OPENAI_MEDIA_LIMIT_EXCEEDED,
            ErrorCategory::Limit,
            "openai media response exceeds the configured byte limit",
        ),
    }
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
                OPENAI_MEDIA_TRANSPORT_FAILED,
                ErrorCategory::Tool,
                "openai media response is invalid",
            )
        })?;
        if body.len().saturating_add(chunk.len()) > cap {
            return Err(tool_error(
                OPENAI_MEDIA_LIMIT_EXCEEDED,
                ErrorCategory::Limit,
                "openai media response exceeds the configured byte limit",
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
            OPENAI_MEDIA_TRANSPORT_FAILED,
            ErrorCategory::Tool,
            "openai media response is invalid",
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
        OPENAI_MEDIA_TIMEOUT,
        ErrorCategory::Deadline,
        "openai media request was cancelled or exceeded its deadline",
    )
}

fn validate_endpoint(value: &str) -> Result<(), OpenAiMediaError> {
    let Some((scheme, rest)) = value.split_once("://") else {
        return Err(OpenAiMediaError::EndpointInvalid {
            reason: "endpoint must be an http or https URL",
        });
    };
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return Err(OpenAiMediaError::EndpointInvalid {
            reason: "endpoint must be an http or https URL",
        });
    }
    if rest.contains('@') || rest.contains('?') || rest.contains('#') {
        return Err(OpenAiMediaError::EndpointInvalid {
            reason: "endpoint contains forbidden components",
        });
    }
    let host = endpoint_host(rest).ok_or(OpenAiMediaError::EndpointInvalid {
        reason: "endpoint host is missing",
    })?;
    if scheme.eq_ignore_ascii_case("http") && !is_loopback_host(host) {
        return Err(OpenAiMediaError::EndpointInvalid {
            reason: "plaintext HTTP is allowed only for loopback endpoints",
        });
    }
    Ok(())
}

/// Validates the audio-download URL for `openai_transcribe_audio`.
///
/// Backed by `finstack-ai-net-guard`'s `parse_and_vet_url` plus its
/// synchronous literal-destination check
/// ([`reject_literal_destination`]). Production behavior is HTTPS-only,
/// with no loopback exception — unlike the toolset's own configured
/// `endpoint` (which may be loopback HTTP for scripted fixtures), the
/// *download* target must always be HTTPS. The one exception is compiled
/// in only for `#[cfg(test)]` builds (see [`download_url_policy`]), where
/// scripted fixtures need to serve audio bytes over `http://127.0.0.1`;
/// that branch does not exist in a release binary.
///
/// Net-guard tightening beyond the crate's former checks, both strictly
/// narrowing and controller-ruled safe: literal or resolved unspecified
/// (`0.0.0.0`), broadcast, and multicast addresses are denied (no
/// legitimate media host is one of these), and a private/loopback literal
/// destination is now rejected for `https` URLs too, not only `http` —
/// the crate's former check permitted `https://127.0.0.1/...` unconditionally
/// in every build, since it never inspected the host once the scheme was
/// `https`. Non-standard `https` ports remain accepted
/// (`allow_nonstandard_https_port: true` in [`download_url_policy`]),
/// matching the crate's prior any-port behavior.
fn validate_download_url(value: &str) -> Result<(), ToolError> {
    let policy = download_url_policy();
    let vetted = parse_and_vet_url(value, &policy).map_err(|_| invalid_download_url())?;
    reject_literal_destination(&vetted, &policy).map_err(|_| invalid_download_url())
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

fn tool_error(code: &'static str, category: ErrorCategory, message: &'static str) -> ToolError {
    ToolError::try_new(code, category, false, message, Metadata::empty()).unwrap_or_else(Into::into)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use base64::Engine as _;
    use finstack_ai_kernel::{
        Digest, EffectId, EffectOutputContract, EffectOutputKind, LaneId, Metadata,
        OperationLocator, PrincipalRef, RawJson, RunId, SessionId, ToolBatchId, ToolCallBlock,
        ToolCallId, ToolFailurePolicy, ValidatedToolCall,
    };
    use finstack_ai_memory::InProcessArtifactStore;
    use finstack_ai_runtime::{
        ApprovalState, ArtifactStore, AuthorizationContext, CancellationSignal,
        JsonSchemaToolValidatorCompiler, ResolvedToolCatalog, RunCallContext, ToolCatalogPlan,
        ToolExecutionPolicy, ToolPolicyDecision, Toolset, ToolsetRegistration,
    };
    use futures_util::StreamExt;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::sync::mpsc;

    use super::{
        BASE64_STANDARD, IMAGE_TOOL_NAME, OPENAI_MEDIA_CREDENTIAL_REQUIRED, OpenAiMediaConfig,
        OpenAiMediaError, OpenAiMediaToolset, SPEECH_TOOL_NAME, TRANSCRIBE_TOOL_NAME,
        validate_download_url,
    };

    const CANARY: &str = "oa-media-secret-canary-046";

    fn base_config(endpoint: String) -> OpenAiMediaConfig {
        OpenAiMediaConfig {
            api_key: CANARY.into(),
            endpoint,
            max_result_bytes: 256 * 1_024,
        }
    }

    fn tool_context() -> crate::ToolCallContext {
        crate::ToolCallContext {
            run: RunCallContext {
                relation_depth: 0,
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
        let reason = if status == 200 { "OK" } else { "Error" };
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
        let error = OpenAiMediaToolset::try_new(OpenAiMediaConfig {
            api_key: String::new(),
            ..base_config("https://api.openai.com".into())
        })
        .expect_err("missing key");
        assert_eq!(error, OpenAiMediaError::CredentialRequired);
        assert!(error.to_string().contains(OPENAI_MEDIA_CREDENTIAL_REQUIRED));
    }

    #[test]
    fn construction_rejects_plaintext_non_loopback() {
        let error = OpenAiMediaToolset::try_new(base_config("http://8.8.8.8".into()))
            .expect_err("plaintext");
        assert!(error.to_string().contains("plaintext HTTP"));
        assert!(!error.to_string().contains(CANARY));
    }

    #[test]
    fn debug_does_not_leak_the_api_key() {
        let tools = OpenAiMediaToolset::try_new(base_config("https://api.openai.com".into()))
            .expect("tools");
        assert!(!format!("{tools:?}").contains(CANARY));
        let config = base_config("https://api.openai.com".into());
        assert!(!format!("{config:?}").contains(CANARY));
    }

    #[test]
    fn construction_rejects_out_of_range_result_cap() {
        let error = OpenAiMediaToolset::try_new(OpenAiMediaConfig {
            max_result_bytes: 0,
            ..base_config("https://api.openai.com".into())
        })
        .expect_err("zero cap");
        assert_eq!(
            error,
            OpenAiMediaError::EndpointInvalid {
                reason: "result cap out of range"
            }
        );
        let error = OpenAiMediaToolset::try_new(OpenAiMediaConfig {
            max_result_bytes: 8 * 1_048_576 + 1,
            ..base_config("https://api.openai.com".into())
        })
        .expect_err("cap too large");
        assert_eq!(
            error,
            OpenAiMediaError::EndpointInvalid {
                reason: "result cap out of range"
            }
        );
    }

    #[tokio::test]
    async fn image_tool_prefers_a_hosted_url() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let (seen_tx, mut seen_rx) = mpsc::unbounded_channel();
        let server = tokio::spawn(async move {
            respond(
                &listener,
                &seen_tx,
                200,
                br#"{"data":[{"url":"https://cdn.openai.test/image.png"}]}"#,
                "application/json",
            )
            .await;
        });
        let tools =
            OpenAiMediaToolset::try_new(base_config(format!("http://{addr}"))).expect("tools");
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
        assert_eq!(payload["url"], "https://cdn.openai.test/image.png");
        assert!(payload.get("b64_json").is_none());
        let seen = seen_rx.recv().await.expect("request").to_ascii_lowercase();
        assert!(seen.contains("post /v1/images/generations"));
        assert!(seen.contains(CANARY));
        server.await.expect("server");
    }

    #[tokio::test]
    async fn image_tool_falls_back_to_b64_json_within_the_cap() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let (seen_tx, mut seen_rx) = mpsc::unbounded_channel();
        let server = tokio::spawn(async move {
            respond(
                &listener,
                &seen_tx,
                200,
                br#"{"data":[{"b64_json":"aGVsbG8="}]}"#,
                "application/json",
            )
            .await;
        });
        let tools =
            OpenAiMediaToolset::try_new(base_config(format!("http://{addr}"))).expect("tools");
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
        let payload: serde_json::Value =
            serde_json::from_slice(result.output.as_bytes()).expect("json");
        assert_eq!(payload["b64_json"], "aGVsbG8=");
        assert!(payload.get("url").is_none());
        let seen = seen_rx.recv().await.expect("request");
        assert!(seen.contains(CANARY));
        server.await.expect("server");
    }

    #[tokio::test]
    async fn a_configured_store_keeps_image_bytes_out_of_the_result() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let (seen_tx, _seen_rx) = mpsc::unbounded_channel();
        let payload = vec![7_u8; 8_192];
        let encoded = BASE64_STANDARD.encode(&payload);
        let body = format!(r#"{{"data":[{{"b64_json":"{encoded}"}}]}}"#);
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
        let tools = OpenAiMediaToolset::try_new(OpenAiMediaConfig {
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
            value.get("b64_json").is_none(),
            "staged results must not inline the payload: {value}"
        );
        assert_eq!(value["byte_length"], 8_192);
        assert!(value.get("artifact").is_some());
        server.await.expect("server");
    }

    #[tokio::test]
    async fn image_tool_enforces_the_result_cap_on_b64_json() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let (seen_tx, _seen_rx) = mpsc::unbounded_channel();
        let big_b64 = "a".repeat(4_096);
        let body = format!(r#"{{"data":[{{"b64_json":"{big_b64}"}}]}}"#);
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
        let tools = OpenAiMediaToolset::try_new(OpenAiMediaConfig {
            max_result_bytes: 1_024,
            ..base_config(format!("http://{addr}"))
        })
        .expect("tools");
        let spec = find_spec(&tools.tools(), IMAGE_TOOL_NAME);
        let call = call_for(&spec, br#"{"model":"m","prompt":"a cat"}"#);
        let Err(error) = tools.call(tool_context(), call).await else {
            panic!("expected limit error");
        };
        assert_eq!(error.code(), crate::OPENAI_MEDIA_LIMIT_EXCEEDED);
        server.await.expect("server");
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
        let tools = OpenAiMediaToolset::try_new(OpenAiMediaConfig {
            max_result_bytes: 1_024,
            ..base_config(format!("http://{addr}"))
        })
        .expect("tools");
        let spec = find_spec(&tools.tools(), SPEECH_TOOL_NAME);
        let call = call_for(&spec, br#"{"model":"m","input":"hello"}"#);
        let Err(error) = tools.call(tool_context(), call).await else {
            panic!("expected limit error");
        };
        assert_eq!(error.code(), crate::OPENAI_MEDIA_LIMIT_EXCEEDED);
        server.await.expect("server");
    }

    #[tokio::test]
    async fn transcribe_tool_downloads_then_uploads_multipart() {
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
            // The multipart request body is large enough (boundary headers +
            // file part) that it may arrive in more than one TCP segment;
            // read until we see the terminal boundary or hit EOF.
            let (mut stream, _) = api_listener.accept().await.expect("accept");
            let mut received = Vec::new();
            let mut buf = vec![0_u8; 8_192];
            loop {
                let n = stream.read(&mut buf).await.expect("read");
                if n == 0 {
                    break;
                }
                received.extend_from_slice(&buf[..n]);
                if received.windows(4).any(|w| w == b"\r\n\r\n")
                    && String::from_utf8_lossy(&received).contains("hello-audio-bytes")
                {
                    break;
                }
            }
            api_tx
                .send(String::from_utf8_lossy(&received).into_owned())
                .expect("seen");
            let body = br#"{"text":"hello"}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream
                .write_all(response.as_bytes())
                .await
                .expect("write head");
            stream.write_all(body).await.expect("write body");
            stream.shutdown().await.expect("shutdown");
        });

        let tools =
            OpenAiMediaToolset::try_new(base_config(format!("http://{api_addr}"))).expect("tools");
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
        let seen = api_rx.recv().await.expect("request");
        assert!(seen.contains("hello-audio-bytes"));
        assert!(seen.contains(CANARY));
        audio_server.await.expect("audio server");
        api_server.await.expect("api server");
    }

    #[tokio::test]
    async fn transcribe_tool_derives_file_name_from_a_presigned_url_ignoring_the_query_string() {
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
            let (mut stream, _) = api_listener.accept().await.expect("accept");
            let mut received = Vec::new();
            let mut buf = vec![0_u8; 8_192];
            loop {
                let n = stream.read(&mut buf).await.expect("read");
                if n == 0 {
                    break;
                }
                received.extend_from_slice(&buf[..n]);
                if received.windows(4).any(|w| w == b"\r\n\r\n")
                    && String::from_utf8_lossy(&received).contains("hello-audio-bytes")
                {
                    break;
                }
            }
            api_tx
                .send(String::from_utf8_lossy(&received).into_owned())
                .expect("seen");
            let body = br#"{"text":"hello"}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream
                .write_all(response.as_bytes())
                .await
                .expect("write head");
            stream.write_all(body).await.expect("write body");
            stream.shutdown().await.expect("shutdown");
        });

        let tools =
            OpenAiMediaToolset::try_new(base_config(format!("http://{api_addr}"))).expect("tools");
        let spec = find_spec(&tools.tools(), TRANSCRIBE_TOOL_NAME);
        // A presigned-style URL: the real file name is followed by a query
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
            seen.contains(r#"filename="a.mp3""#),
            "file name must be derived from the path, not the query string: {seen}"
        );
        audio_server.await.expect("audio server");
        api_server.await.expect("api server");
    }

    #[tokio::test]
    async fn transcribe_tool_rejects_plaintext_non_loopback_download_urls() {
        // Covers both the simple malformed-host case and the documented
        // negative that a bare `http:` audio_url pointing at a real
        // (but unreachable-over-HTTP) listener is rejected by
        // `validate_download_url` before any HTTP traffic is attempted; the
        // http://127.0.0.1 test-only exception is exercised positively by
        // `transcribe_tool_downloads_then_uploads_multipart`.
        let tools = OpenAiMediaToolset::try_new(base_config("https://api.openai.com".into()))
            .expect("tools");
        let spec = find_spec(&tools.tools(), TRANSCRIBE_TOOL_NAME);
        let call = call_for(
            &spec,
            br#"{"model":"m","audio_url":"http://8.8.8.8/a.mp3"}"#,
        );
        let Err(error) = tools.call(tool_context(), call).await else {
            panic!("expected invalid arguments");
        };
        assert_eq!(error.code(), crate::OPENAI_MEDIA_INVALID_ARGUMENTS);
    }

    #[test]
    fn validate_download_url_accepts_https_query_strings() {
        // Signed download URLs (presigned S3/GCS links) are a normal shape
        // for a caller-supplied audio_url; the query string must not be
        // rejected as a "forbidden component".
        validate_download_url(
            "https://example-bucket.s3.amazonaws.com/a.mp3?X-Amz-Signature=abc123&X-Amz-Expires=900",
        )
        .expect("https download url with a query string is accepted");
    }

    #[test]
    fn validate_download_url_accepts_a_nonstandard_https_port() {
        // A presigned URL against a self-hosted/MinIO-style object store on
        // a non-standard port is a normal shape for a caller-supplied
        // audio_url; net-guard's own default is 443-only, but this crate's
        // download policy opts back in to the crate's prior any-port
        // behavior.
        validate_download_url("https://media.example.com:8443/a.mp3")
            .expect("https download url on a non-standard port is accepted");
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
            OpenAiMediaToolset::try_new(base_config(format!("http://{addr}"))).expect("tools");
        let spec = find_spec(&tools.tools(), IMAGE_TOOL_NAME);
        let ctx = tool_context();
        ctx.run.cancellation.cancel();
        let call = call_for(&spec, br#"{"model":"m","prompt":"a cat"}"#);
        let Err(error) = tools.call(ctx, call).await else {
            panic!("cancelled");
        };
        assert_eq!(error.code(), crate::OPENAI_MEDIA_TIMEOUT);
        assert!(
            seen_rx.try_recv().is_err(),
            "no HTTP must reach the fixture"
        );
        server.abort();
    }

    fn host_allow_catalog(toolset: Arc<dyn Toolset>) -> ResolvedToolCatalog {
        let policies = toolset
            .tools()
            .iter()
            .map(|spec| {
                (
                    spec.id.clone(),
                    ToolExecutionPolicy {
                        failure_policy: ToolFailurePolicy::ReturnToModel,
                        approval: ToolPolicyDecision::Allow,
                        max_concurrency: 1,
                    },
                )
            })
            .collect();
        ResolvedToolCatalog::try_new(
            [ToolsetRegistration {
                toolset,
                policies,
                components: BTreeMap::new(),
            }],
            &BTreeMap::new(),
            &JsonSchemaToolValidatorCompiler,
        )
        .expect("catalog")
    }

    #[test]
    fn policy_image_requires_approval_under_host_allow() {
        let tools = Arc::new(
            OpenAiMediaToolset::try_new(base_config("https://api.openai.com".into()))
                .expect("tools"),
        );
        let catalog = host_allow_catalog(tools);
        assert_eq!(
            catalog.decide_plan(
                ToolCallBlock::try_new(
                    ToolCallId::from_bytes([9; 16]),
                    IMAGE_TOOL_NAME,
                    RawJson::parse(br#"{"model":"gpt-image-1","prompt":"a cat","size":null}"#)
                        .expect("args"),
                )
                .expect("call"),
                None,
                None,
                ApprovalState::Unpaid,
            ),
            ToolCatalogPlan::RequireApproval
        );
    }

    #[test]
    fn openai_media_is_not_a_wasm_host_sdk_dependency() {
        let manifest = include_str!("../../../../crates/finstack-ai/Cargo.toml");
        let wasm_host = manifest
            .lines()
            .find(|line| line.contains("wasm-host ="))
            .expect("wasm-host feature");
        assert!(
            !wasm_host.contains("finstack-ai-tools-openai-media"),
            "the openai media toolset must stay off the wasm-host feature graph"
        );
        assert!(manifest.contains("dep:finstack-ai-tools-openai-media"));
    }
}
