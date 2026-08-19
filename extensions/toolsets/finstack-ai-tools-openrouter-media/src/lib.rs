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
use std::time::Duration;

use finstack_ai_kernel::{
    ErrorCategory, Metadata, RawJson, RetrySafety, ToolExecutionMode, ToolId, ValidatedToolCall,
};
use finstack_ai_runtime::{
    ApprovalMetadata, ApprovalRequirement, PortFuture, SideEffectClass, ToolCallContext,
    ToolDeferralSupport, ToolError, ToolEventStream, ToolSpec, Toolset, ToolsetDescriptor,
    verify_authority,
};
use thiserror::Error;

const DEFAULT_ENDPOINT: &str = "https://openrouter.ai";
const REQUEST_TIMEOUT: Duration = Duration::from_mins(2);
#[allow(dead_code)]
const DEFAULT_MAX_RESULT_BYTES: usize = 256 * 1_024;
const MAX_RESULT_BYTES_CEILING: usize = 8 * 1_048_576;
#[allow(dead_code)]
const MAX_AUDIO_DOWNLOAD_BYTES: usize = 25 * 1_048_576;

// The 5-second production poll interval for `openrouter_get_video`'s
// `wait_seconds` loop (shortened under `#[cfg(test)]`) is wired in Task 12
// alongside the poll loop itself.
#[allow(dead_code)]
#[cfg(not(test))]
const POLL_INTERVAL: Duration = Duration::from_secs(5);
#[allow(dead_code)]
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
// Several fields are only read by Task 12's `Toolset::call` dispatch.
#[allow(dead_code)]
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
    client: reqwest::Client,
}

impl std::fmt::Debug for OpenRouterMediaToolset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenRouterMediaToolset")
            .field("endpoint", &self.endpoint)
            .field("max_result_bytes", &self.max_result_bytes)
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
                br#"{"additionalProperties":false,"properties":{"aspect_ratio":{"type":"string"},"model":{"minLength":1,"type":"string"},"output_format":{"type":"string"},"prompt":{"minLength":1,"type":"string"},"resolution":{"type":"string"}},"required":["model","prompt"],"type":"object"}"#,
            )
            .map_err(|_| OpenRouterMediaError::EndpointInvalid {
                reason: "invalid_input_schema",
            })?,
            output_schema: Some(
                RawJson::parse(
                    br#"{"additionalProperties":false,"properties":{"b64_json":{"type":"string"},"media_type":{"type":"string"}},"required":["b64_json"],"type":"object"}"#,
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
                "Submit one asynchronous video-generation job via OpenRouter; poll it with openrouter_get_video.",
            ),
            input_schema: RawJson::parse(
                br#"{"additionalProperties":false,"properties":{"aspect_ratio":{"type":"string"},"duration":{"type":"integer"},"model":{"minLength":1,"type":"string"},"prompt":{"minLength":1,"type":"string"},"resolution":{"type":"string"}},"required":["model","prompt"],"type":"object"}"#,
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
                "Check one OpenRouter video job; returns download URLs when completed. Set wait_seconds (0-300, default 0) to keep polling inside this call until the job finishes or the time is up.",
            ),
            input_schema: RawJson::parse(
                br#"{"additionalProperties":false,"properties":{"id":{"minLength":1,"type":"string"},"wait_seconds":{"maximum":300,"minimum":0,"type":"integer"}},"required":["id"],"type":"object"}"#,
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
            description: Arc::from("Synthesize speech from text via OpenRouter."),
            input_schema: RawJson::parse(
                br#"{"additionalProperties":false,"properties":{"input":{"minLength":1,"type":"string"},"model":{"minLength":1,"type":"string"},"voice":{"type":"string"}},"required":["model","input"],"type":"object"}"#,
            )
            .map_err(|_| OpenRouterMediaError::EndpointInvalid {
                reason: "invalid_input_schema",
            })?,
            output_schema: Some(
                RawJson::parse(
                    br#"{"additionalProperties":false,"properties":{"b64_audio":{"type":"string"},"media_type":{"type":"string"}},"required":["b64_audio","media_type"],"type":"object"}"#,
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
                br#"{"additionalProperties":false,"properties":{"audio_url":{"minLength":1,"type":"string"},"format":{"type":"string"},"model":{"minLength":1,"type":"string"}},"required":["model","audio_url"],"type":"object"}"#,
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

        let client = reqwest::Client::builder()
            .http1_only()
            .timeout(REQUEST_TIMEOUT)
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
            client,
        })
    }
}

impl Toolset for OpenRouterMediaToolset {
    fn descriptor(&self) -> ToolsetDescriptor {
        self.descriptor.clone()
    }

    fn tools(&self) -> Arc<[ToolSpec]> {
        Arc::clone(&self.tools)
    }

    fn call(
        &self,
        ctx: ToolCallContext,
        _call: ValidatedToolCall,
    ) -> PortFuture<Result<ToolEventStream, ToolError>> {
        Box::pin(async move {
            verify_authority(&ctx)?;
            // Task 12 replaces this fail-closed stub with the real wire
            // dispatch (images, video submit/status, speech, transcription).
            Err(tool_error(
                OPENROUTER_MEDIA_INVALID_ARGUMENTS,
                ErrorCategory::Validation,
                "openrouter media call dispatch is not implemented",
            ))
        })
    }
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

