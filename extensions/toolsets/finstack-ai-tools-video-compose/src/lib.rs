//! T1 declarative ffmpeg composition Toolset. Agents submit a bounded spec;
//! the toolset owns every ffmpeg argument.

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

mod graph;
mod spec;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use finstack_ai_kernel::{
    ErrorCategory, Metadata, RawJson, RetrySafety, ToolExecutionMode, ToolId, ValidatedToolCall,
};
use finstack_ai_runtime::artifact::ArtifactStore;
use finstack_ai_runtime::ports::PortFuture;
use finstack_ai_runtime::ports::model::{
    ApprovalMetadata, ApprovalRequirement, SideEffectClass, ToolDeferralSupport, ToolSpec,
};
use finstack_ai_runtime::ports::tool::{
    ToolCallContext, ToolError, ToolEventStream, Toolset, ToolsetDescriptor, verify_authority,
};
use thiserror::Error;

/// Ceiling for [`VideoComposeConfig::render_timeout`].
const MAX_RENDER_TIMEOUT: Duration = Duration::from_hours(1);

const COMPOSE_TOOL_NAME: &str = "compose_video";
const COMPOSE_TOOL_ID: &str = "finstack.tools.compose_video";
const PROBE_TOOL_NAME: &str = "probe_media";
const PROBE_TOOL_ID: &str = "finstack.tools.probe_media";

/// Stable construction-failure code.
pub const VIDEO_COMPOSE_CONFIG_INVALID: &str = "video_compose_config_invalid";
/// Stable argument-validation code.
pub const VIDEO_COMPOSE_INVALID_ARGUMENTS: &str = "video_compose_invalid_arguments";
/// Stable code for a `validate_spec` rejection (Task 5 attaches the reason).
pub const VIDEO_COMPOSE_SPEC_INVALID: &str = "video_compose_spec_invalid";
/// Stable code for an `ffmpeg`/`ffprobe` process failure.
pub const VIDEO_COMPOSE_FFMPEG_FAILED: &str = "video_compose_ffmpeg_failed";
/// Stable code for a render exceeding `render_timeout`.
pub const VIDEO_COMPOSE_TIMEOUT: &str = "video_compose_timeout";
/// Stable code for a stored-media failure: missing artifact, or an
/// artifact-store read/write error.
pub const VIDEO_COMPOSE_MEDIA_FAILURE: &str = "video_compose_media_failure";
/// Stable output-limit code.
pub const VIDEO_COMPOSE_LIMIT_EXCEEDED: &str = "video_compose_limit_exceeded";

/// Explicit video-compose route. Never populated from the environment.
pub struct VideoComposeConfig {
    /// Absolute path to a host-supplied `ffmpeg` binary.
    pub ffmpeg_path: PathBuf,
    /// Absolute path to a host-supplied `ffprobe` binary.
    pub ffprobe_path: PathBuf,
    /// Artifact store backing every clip, audio, subtitle, and rendered
    /// output artifact. Required: this toolset never reads or writes a
    /// caller-supplied filesystem path.
    pub artifact_store: Arc<dyn ArtifactStore>,
    /// Scratch directory for intermediate render files. Created (recursively)
    /// during construction if it does not already exist.
    pub scratch_dir: PathBuf,
    /// Wall-clock ceiling for one render. Must be in `(0, 1 hour]`.
    pub render_timeout: Duration,
}

impl std::fmt::Debug for VideoComposeConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VideoComposeConfig")
            .field("ffmpeg_path", &self.ffmpeg_path)
            .field("ffprobe_path", &self.ffprobe_path)
            .field("artifact_store", &"<configured>")
            .field("scratch_dir", &self.scratch_dir)
            .field("render_timeout", &self.render_timeout)
            .finish()
    }
}

/// Construction failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum VideoComposeError {
    /// An `ffmpeg`/`ffprobe` path was not absolute, `render_timeout` was
    /// zero or over one hour, or `scratch_dir` could not be created.
    #[error("{VIDEO_COMPOSE_CONFIG_INVALID}: {reason}")]
    ConfigInvalid {
        /// Stable non-secret reason.
        reason: &'static str,
    },
}

/// T1 declarative ffmpeg composition Toolset.
pub struct VideoComposeToolset {
    descriptor: ToolsetDescriptor,
    tools: Arc<[ToolSpec]>,
    compose_tool_id: ToolId,
    probe_tool_id: ToolId,
    ffmpeg_path: PathBuf,
    ffprobe_path: PathBuf,
    artifact_store: Arc<dyn ArtifactStore>,
    scratch_dir: PathBuf,
    render_timeout: Duration,
}

impl std::fmt::Debug for VideoComposeToolset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VideoComposeToolset")
            .field("compose_tool_id", &self.compose_tool_id)
            .field("probe_tool_id", &self.probe_tool_id)
            .field("ffmpeg_path", &self.ffmpeg_path)
            .field("ffprobe_path", &self.ffprobe_path)
            .field(
                "artifact_store_refs",
                &Arc::strong_count(&self.artifact_store),
            )
            .field("scratch_dir", &self.scratch_dir)
            .field("render_timeout", &self.render_timeout)
            .finish_non_exhaustive()
    }
}

impl VideoComposeToolset {
    /// Construct the Toolset after validating the explicit route.
    ///
    /// # Errors
    ///
    /// Returns [`VideoComposeError::ConfigInvalid`] when `ffmpeg_path` or
    /// `ffprobe_path` is not absolute, `render_timeout` is zero or exceeds
    /// one hour, `scratch_dir` cannot be created, or a built-in
    /// [`ToolSpec`] fails its own validation.
    #[allow(clippy::too_many_lines)]
    pub fn try_new(config: VideoComposeConfig) -> Result<Self, VideoComposeError> {
        if !config.ffmpeg_path.is_absolute() {
            return Err(VideoComposeError::ConfigInvalid {
                reason: "ffmpeg_path must be an absolute path",
            });
        }
        if !config.ffprobe_path.is_absolute() {
            return Err(VideoComposeError::ConfigInvalid {
                reason: "ffprobe_path must be an absolute path",
            });
        }
        if config.render_timeout.is_zero() || config.render_timeout > MAX_RENDER_TIMEOUT {
            return Err(VideoComposeError::ConfigInvalid {
                reason: "render_timeout must be greater than zero and at most one hour",
            });
        }
        std::fs::create_dir_all(&config.scratch_dir).map_err(|_| {
            VideoComposeError::ConfigInvalid {
                reason: "scratch_dir could not be created",
            }
        })?;

        let compose_tool_id =
            ToolId::parse(COMPOSE_TOOL_ID).map_err(|_| VideoComposeError::ConfigInvalid {
                reason: "invalid_tool_id",
            })?;
        let probe_tool_id =
            ToolId::parse(PROBE_TOOL_ID).map_err(|_| VideoComposeError::ConfigInvalid {
                reason: "invalid_tool_id",
            })?;

        let compose_spec = ToolSpec {
            id: compose_tool_id.clone(),
            model_name: Arc::from(COMPOSE_TOOL_NAME),
            title: Arc::from("Compose video"),
            description: Arc::from(
                "Merge stored clips into one movie with declarative transitions, audio, and subtitles.",
            ),
            input_schema: RawJson::parse(
                br#"{"additionalProperties":false,"properties":{"audio":{"additionalProperties":false,"properties":{"artifact":{"type":"object"},"gain_db":{"type":"number"},"mode":{"enum":["replace","mix"],"type":"string"}},"required":["artifact","mode"],"type":"object"},"clips":{"items":{"additionalProperties":false,"properties":{"artifact":{"type":"object"},"trim":{"additionalProperties":false,"properties":{"end_s":{"type":"number"},"start_s":{"type":"number"}},"required":["end_s","start_s"],"type":"object"}},"required":["artifact"],"type":"object"},"type":"array"},"output":{"additionalProperties":false,"properties":{"container":{"enum":["mp4","webm"],"type":"string"},"fps":{"type":"integer"},"resolution":{"type":"string"}},"required":["container"],"type":"object"},"subtitles":{"additionalProperties":false,"properties":{"artifact":{"type":"object"},"mode":{"enum":["burn_in","mux"],"type":"string"},"style":{"additionalProperties":false,"properties":{"font_size":{"type":"integer"},"margin_v":{"type":"integer"}},"type":"object"}},"required":["artifact","mode"],"type":"object"},"transitions":{"items":{"additionalProperties":false,"properties":{"duration_s":{"type":"number"},"type":{"enum":["cut","crossfade","fade_to_black"],"type":"string"}},"required":["type"],"type":"object"},"type":"array"},"version":{"type":"integer"}},"required":["version","clips","output"],"type":"object"}"#,
            )
            .map_err(|_| VideoComposeError::ConfigInvalid {
                reason: "invalid_input_schema",
            })?,
            output_schema: Some(
                RawJson::parse(
                    br#"{"additionalProperties":false,"properties":{"artifact":{"type":"object"},"byte_length":{"type":"integer"},"duration_s":{"type":"number"}},"required":["artifact","duration_s","byte_length"],"type":"object"}"#,
                )
                .map_err(|_| VideoComposeError::ConfigInvalid {
                    reason: "invalid_output_schema",
                })?,
            ),
            execution: ToolExecutionMode::Sequential,
            side_effect: SideEffectClass::NonIdempotentWrite,
            retry_safety: RetrySafety::AtMostOnce,
            approval: ApprovalMetadata {
                requirement: ApprovalRequirement::Policy,
                reason: Some(Arc::from("local media rendering")),
                attributes: Metadata::empty(),
            },
            max_result_bytes: 262_144,
            metadata: Metadata::empty(),
            deferral: ToolDeferralSupport::Never,
        };
        compose_spec
            .validate()
            .map_err(|_| VideoComposeError::ConfigInvalid {
                reason: "invalid_tool_spec",
            })?;

        let probe_spec = ToolSpec {
            id: probe_tool_id.clone(),
            model_name: Arc::from(PROBE_TOOL_NAME),
            title: Arc::from("Probe media"),
            description: Arc::from(
                "Probe duration, dimensions, and streams of one stored media artifact.",
            ),
            input_schema: RawJson::parse(
                br#"{"additionalProperties":false,"properties":{"artifact":{"type":"object"}},"required":["artifact"],"type":"object"}"#,
            )
            .map_err(|_| VideoComposeError::ConfigInvalid {
                reason: "invalid_input_schema",
            })?,
            output_schema: Some(
                RawJson::parse(
                    br#"{"additionalProperties":false,"properties":{"duration_s":{"type":"number"},"fps":{"type":"number"},"has_audio":{"type":"boolean"},"height":{"type":"integer"},"media_type":{"type":"string"},"width":{"type":"integer"}},"required":["duration_s","has_audio"],"type":"object"}"#,
                )
                .map_err(|_| VideoComposeError::ConfigInvalid {
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
            max_result_bytes: 65_536,
            metadata: Metadata::empty(),
            deferral: ToolDeferralSupport::Never,
        };
        probe_spec
            .validate()
            .map_err(|_| VideoComposeError::ConfigInvalid {
                reason: "invalid_tool_spec",
            })?;

        Ok(Self {
            descriptor: ToolsetDescriptor {
                name: Arc::from("finstack-video-compose"),
                metadata: Metadata::empty(),
            },
            tools: Arc::from([compose_spec, probe_spec]),
            compose_tool_id,
            probe_tool_id,
            ffmpeg_path: config.ffmpeg_path,
            ffprobe_path: config.ffprobe_path,
            artifact_store: config.artifact_store,
            scratch_dir: config.scratch_dir,
            render_timeout: config.render_timeout,
        })
    }
}

impl Toolset for VideoComposeToolset {
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
        // Execution (the filtergraph builder and process runner) lands in a
        // later task. Until then every call is rejected uniformly, whether
        // it names `compose_tool_id` or `probe_tool_id`.
        let _known_tool =
            call.tool_id == self.compose_tool_id || call.tool_id == self.probe_tool_id;
        Box::pin(async move {
            verify_authority(&ctx)?;
            Err(ToolError::try_new(
                VIDEO_COMPOSE_INVALID_ARGUMENTS,
                ErrorCategory::Validation,
                false,
                "video compose call execution is not yet implemented",
                Metadata::empty(),
            )
            .unwrap_or_else(Into::into))
        })
    }
}
