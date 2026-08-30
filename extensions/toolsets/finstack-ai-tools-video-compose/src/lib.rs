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

mod exec;
mod graph;
mod spec;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use finstack_ai_kernel::{
    ArtifactRef, ErrorCategory, Metadata, RawJson, RetrySafety, Sensitivity, ToolExecutionMode,
    ToolId, ValidatedToolCall,
};
use finstack_ai_runtime::artifact::{
    ArtifactError, ArtifactMetadata, ArtifactScope, ArtifactStore, stage_required_artifact,
    validate_retrieved_artifact,
};
use finstack_ai_runtime::ports::PortFuture;
use finstack_ai_runtime::ports::model::{
    ApprovalMetadata, ApprovalRequirement, SideEffectClass, ToolDeferralSupport, ToolSpec,
};
use finstack_ai_runtime::ports::tool::{
    ToolCallContext, ToolError, ToolEventStream, ToolResult, ToolStreamItem, Toolset,
    ToolsetDescriptor, verify_authority,
};
use futures_util::stream;
use serde::Deserialize;
use thiserror::Error;

use crate::graph::{ClipInput, build_ffmpeg_args};
use crate::spec::{CompositionSpec, Container, validate_spec};

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
        let compose_tool_id = self.compose_tool_id.clone();
        let probe_tool_id = self.probe_tool_id.clone();
        let ffmpeg_path = self.ffmpeg_path.clone();
        let ffprobe_path = self.ffprobe_path.clone();
        let artifact_store = Arc::clone(&self.artifact_store);
        let scratch_dir = self.scratch_dir.clone();
        let render_timeout = self.render_timeout;
        Box::pin(async move {
            verify_authority(&ctx)?;
            if call.tool_id == compose_tool_id && call.call.tool_name() == COMPOSE_TOOL_NAME {
                return handle_compose(
                    &ctx,
                    &call,
                    &ffmpeg_path,
                    &ffprobe_path,
                    &artifact_store,
                    &scratch_dir,
                    render_timeout,
                )
                .await;
            }
            if call.tool_id == probe_tool_id && call.call.tool_name() == PROBE_TOOL_NAME {
                return handle_probe(
                    &ctx,
                    &call,
                    &ffprobe_path,
                    &artifact_store,
                    &scratch_dir,
                    render_timeout,
                )
                .await;
            }
            Err(tool_error(
                VIDEO_COMPOSE_INVALID_ARGUMENTS,
                ErrorCategory::Validation,
                "video compose call identity is invalid",
            ))
        })
    }
}

/// Exact scope binding for every artifact operation this Toolset performs.
pub(crate) fn artifact_scope(ctx: &ToolCallContext) -> ArtifactScope {
    ArtifactScope {
        tenant_scope: Arc::clone(&ctx.run.locator.tenant_scope),
        session_id: ctx.run.locator.session_id,
        run_id: Some(ctx.run.locator.run_id),
        sensitivity: Sensitivity::Internal,
    }
}

/// Fetch one stored artifact to a scratch file and return its path.
///
/// # Errors
///
/// Returns [`VIDEO_COMPOSE_MEDIA_FAILURE`] when the store read fails, the
/// returned bytes fail independent verification, or the scratch write fails.
pub(crate) async fn fetch_artifact_to_file(
    store: &Arc<dyn ArtifactStore>,
    scope: &ArtifactScope,
    artifact: &ArtifactRef,
    dir: &Path,
    stem: &str,
) -> Result<PathBuf, ToolError> {
    let content = store
        .get(scope.clone(), artifact.clone())
        .await
        .map_err(|_| {
            tool_error(
                VIDEO_COMPOSE_MEDIA_FAILURE,
                ErrorCategory::Tool,
                "artifact fetch failed",
            )
        })?;
    validate_retrieved_artifact(scope, artifact, &content).map_err(|_| {
        tool_error(
            VIDEO_COMPOSE_MEDIA_FAILURE,
            ErrorCategory::Tool,
            "artifact integrity check failed",
        )
    })?;
    let digest_hex = artifact.content_digest().to_hex();
    let short = digest_hex.get(..16).unwrap_or(digest_hex.as_str());
    let path = dir.join(format!("{stem}-{short}"));
    tokio::fs::write(&path, &content).await.map_err(|_| {
        tool_error(
            VIDEO_COMPOSE_MEDIA_FAILURE,
            ErrorCategory::Tool,
            "scratch write failed",
        )
    })?;
    Ok(path)
}

/// Stage a rendered scratch file as a required output artifact.
///
/// # Errors
///
/// Returns [`VIDEO_COMPOSE_LIMIT_EXCEEDED`] when the store rejects the
/// content as oversized, or [`VIDEO_COMPOSE_MEDIA_FAILURE`] for any other
/// read or staging failure.
pub(crate) async fn stage_output(
    store: &Arc<dyn ArtifactStore>,
    scope: &ArtifactScope,
    path: &Path,
    media_type: &str,
) -> Result<(ArtifactRef, u64), ToolError> {
    let content = tokio::fs::read(path).await.map_err(|_| {
        tool_error(
            VIDEO_COMPOSE_MEDIA_FAILURE,
            ErrorCategory::Tool,
            "render output read failed",
        )
    })?;
    let byte_length = u64::try_from(content.len()).unwrap_or(u64::MAX);
    let metadata = ArtifactMetadata {
        kind: Arc::from("tool-output"),
        media_type: Arc::from(media_type),
        name: None,
        attributes: Metadata::empty(),
    };
    let artifact = stage_required_artifact(
        store.as_ref(),
        scope.clone(),
        Bytes::from(content),
        metadata,
    )
    .await
    .map_err(|error| match error {
        ArtifactError::TooLarge { .. } => tool_error(
            VIDEO_COMPOSE_LIMIT_EXCEEDED,
            ErrorCategory::Limit,
            "render output exceeds the artifact store limit",
        ),
        _ => tool_error(
            VIDEO_COMPOSE_MEDIA_FAILURE,
            ErrorCategory::Tool,
            "render output staging failed",
        ),
    })?;
    Ok((artifact, byte_length))
}

/// Best-effort scratch-file cleanup. Tracked files are removed when the
/// guard drops, whether the enclosing call succeeded or failed early.
#[derive(Default)]
struct ScratchGuard {
    paths: Vec<PathBuf>,
}

impl ScratchGuard {
    fn track(&mut self, path: PathBuf) -> PathBuf {
        self.paths.push(path.clone());
        path
    }
}

impl Drop for ScratchGuard {
    fn drop(&mut self) {
        for path in &self.paths {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProbeMediaArguments {
    artifact: ArtifactRef,
}

async fn handle_probe(
    ctx: &ToolCallContext,
    call: &ValidatedToolCall,
    ffprobe_path: &Path,
    artifact_store: &Arc<dyn ArtifactStore>,
    scratch_dir: &Path,
    render_timeout: Duration,
) -> Result<ToolEventStream, ToolError> {
    let args: ProbeMediaArguments = serde_json::from_slice(call.call.arguments().as_bytes())
        .map_err(|_| {
            tool_error(
                VIDEO_COMPOSE_INVALID_ARGUMENTS,
                ErrorCategory::Validation,
                "probe_media arguments are invalid",
            )
        })?;
    let scope = artifact_scope(ctx);
    let mut scratch = ScratchGuard::default();
    let path = scratch.track(
        fetch_artifact_to_file(artifact_store, &scope, &args.artifact, scratch_dir, "probe")
            .await?,
    );
    let probe = exec::probe(ffprobe_path, &path, render_timeout, &ctx.run.cancellation).await?;

    let media_type = args.artifact.blob().media_type();
    let mut fields = serde_json::Map::new();
    fields.insert("duration_s".to_owned(), serde_json::json!(probe.duration_s));
    if let Some(width) = probe.width {
        fields.insert("width".to_owned(), serde_json::json!(width));
    }
    if let Some(height) = probe.height {
        fields.insert("height".to_owned(), serde_json::json!(height));
    }
    if let Some(fps) = probe.fps {
        fields.insert("fps".to_owned(), serde_json::json!(fps));
    }
    fields.insert("has_audio".to_owned(), serde_json::json!(probe.has_audio));
    fields.insert("media_type".to_owned(), serde_json::json!(media_type));
    completed_stream(&serde_json::Value::Object(fields))
}

#[allow(clippy::too_many_lines)]
async fn handle_compose(
    ctx: &ToolCallContext,
    call: &ValidatedToolCall,
    ffmpeg_path: &Path,
    ffprobe_path: &Path,
    artifact_store: &Arc<dyn ArtifactStore>,
    scratch_dir: &Path,
    render_timeout: Duration,
) -> Result<ToolEventStream, ToolError> {
    let spec: CompositionSpec =
        serde_json::from_slice(call.call.arguments().as_bytes()).map_err(|_| {
            tool_error(
                VIDEO_COMPOSE_INVALID_ARGUMENTS,
                ErrorCategory::Validation,
                "compose_video arguments are invalid",
            )
        })?;
    validate_spec(&spec).map_err(|reason| {
        tool_error(
            VIDEO_COMPOSE_SPEC_INVALID,
            ErrorCategory::Validation,
            reason,
        )
    })?;

    let scope = artifact_scope(ctx);
    let mut scratch = ScratchGuard::default();

    let mut clip_paths = Vec::with_capacity(spec.clips.len());
    for (idx, clip) in spec.clips.iter().enumerate() {
        let path = scratch.track(
            fetch_artifact_to_file(
                artifact_store,
                &scope,
                &clip.artifact,
                scratch_dir,
                &format!("clip-{idx}"),
            )
            .await?,
        );
        clip_paths.push(path);
    }

    let audio_path = match &spec.audio {
        Some(audio) => Some(
            scratch.track(
                fetch_artifact_to_file(
                    artifact_store,
                    &scope,
                    &audio.artifact,
                    scratch_dir,
                    "audio",
                )
                .await?,
            ),
        ),
        None => None,
    };

    let subtitles_path = match &spec.subtitles {
        Some(subtitles) => {
            let staged = scratch.track(
                fetch_artifact_to_file(
                    artifact_store,
                    &scope,
                    &subtitles.artifact,
                    scratch_dir,
                    "subs",
                )
                .await?,
            );
            let srt_path = scratch.track(staged.with_extension("srt"));
            tokio::fs::rename(&staged, &srt_path).await.map_err(|_| {
                tool_error(
                    VIDEO_COMPOSE_MEDIA_FAILURE,
                    ErrorCategory::Tool,
                    "subtitles scratch rename failed",
                )
            })?;
            Some(srt_path)
        }
        None => None,
    };

    let mut clip_inputs = Vec::with_capacity(clip_paths.len());
    for path in &clip_paths {
        let probed = exec::probe(ffprobe_path, path, render_timeout, &ctx.run.cancellation).await?;
        clip_inputs.push(ClipInput {
            path: path.clone(),
            duration_s: probed.duration_s,
        });
    }

    let ext = match spec.output.container {
        Container::Mp4 => "mp4",
        Container::Webm => "webm",
    };
    let output_path =
        scratch.track(scratch_dir.join(format!("render-{:x?}.{ext}", ctx.run.effect_id)));

    let args = build_ffmpeg_args(
        &spec,
        &clip_inputs,
        audio_path.as_deref(),
        subtitles_path.as_deref(),
        &output_path,
    )
    .map_err(|reason| {
        tool_error(
            VIDEO_COMPOSE_SPEC_INVALID,
            ErrorCategory::Validation,
            reason,
        )
    })?;

    exec::run_bounded(ffmpeg_path, &args, render_timeout, &ctx.run.cancellation).await?;

    let probed = exec::probe(
        ffprobe_path,
        &output_path,
        render_timeout,
        &ctx.run.cancellation,
    )
    .await?;

    let media_type = match spec.output.container {
        Container::Mp4 => "video/mp4",
        Container::Webm => "video/webm",
    };
    let (artifact, byte_length) =
        stage_output(artifact_store, &scope, &output_path, media_type).await?;
    drop(scratch);

    let artifact_json = serde_json::to_value(&artifact).map_err(|_| {
        tool_error(
            VIDEO_COMPOSE_MEDIA_FAILURE,
            ErrorCategory::Internal,
            "artifact reference serialization failed",
        )
    })?;
    completed_stream(&serde_json::json!({
        "artifact": artifact_json,
        "duration_s": probed.duration_s,
        "byte_length": byte_length,
    }))
}

fn completed_stream(value: &serde_json::Value) -> Result<ToolEventStream, ToolError> {
    let bytes = serde_json::to_vec(value).map_err(|_| {
        tool_error(
            VIDEO_COMPOSE_MEDIA_FAILURE,
            ErrorCategory::Internal,
            "result serialization failed",
        )
    })?;
    let output = RawJson::parse(bytes).map_err(|_| {
        tool_error(
            VIDEO_COMPOSE_MEDIA_FAILURE,
            ErrorCategory::Internal,
            "result normalization failed",
        )
    })?;
    let result = ToolResult {
        output,
        is_error: false,
    };
    Ok(Box::pin(stream::once(async move {
        Ok(ToolStreamItem::Completed(result))
    })) as ToolEventStream)
}

/// Stable adapter error for a cancelled or overrun bounded process.
pub(crate) fn timeout_error() -> ToolError {
    tool_error(
        VIDEO_COMPOSE_TIMEOUT,
        ErrorCategory::Deadline,
        "compose process was cancelled or exceeded its render timeout",
    )
}

fn tool_error(code: &'static str, category: ErrorCategory, message: impl AsRef<str>) -> ToolError {
    ToolError::try_new(code, category, false, message, Metadata::empty()).unwrap_or_else(Into::into)
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::Duration;

    use bytes::Bytes;
    use finstack_ai_kernel::{
        ArtifactId, BlobRef, Digest, EffectId, EffectOutputContract, EffectOutputKind, LaneId,
        Metadata, OperationLocator, PrincipalRef, RawJson, RunId, Sensitivity, SessionId,
        ToolBatchId, ToolCallBlock, ToolCallId, ToolFailurePolicy, ValidatedToolCall,
    };
    use finstack_ai_runtime::artifact::{
        ArtifactMetadata, ArtifactScope, ArtifactStore, InProcessArtifactStore,
        stage_required_artifact,
    };
    use finstack_ai_runtime::ports::model::ToolSpec;
    use finstack_ai_runtime::ports::model::{
        AuthorizationContext, CancellationSignal, RunCallContext,
    };
    use finstack_ai_runtime::ports::tool::{ToolCallContext, ToolStreamItem, Toolset};
    use futures_util::StreamExt;
    use tempfile::tempdir;

    use super::{
        ArtifactRef, VIDEO_COMPOSE_FFMPEG_FAILED, VIDEO_COMPOSE_LIMIT_EXCEEDED,
        VIDEO_COMPOSE_SPEC_INVALID, VIDEO_COMPOSE_TIMEOUT, VideoComposeConfig, VideoComposeToolset,
    };

    fn write_stub(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("stub");
        let mut permissions = std::fs::metadata(&path).expect("meta").permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&path, permissions).expect("chmod");
        path
    }

    const FFPROBE_STUB_BODY: &str = r#"printf '%s' '{"format":{"duration":"4.0"},"streams":[{"codec_type":"video","width":640,"height":360,"avg_frame_rate":"24/1"},{"codec_type":"audio"}]}'"#;
    const FFMPEG_STUB_BODY: &str = r#"d=$(dirname "$0"); printf '%s\n' "$@" > "$d/ffmpeg-args.txt"; eval last=\"\${$#}\"; printf 'x' > "$last""#;

    fn scope() -> ArtifactScope {
        ArtifactScope {
            tenant_scope: Arc::from("tenant-a"),
            session_id: SessionId::from_bytes([1; 16]),
            run_id: Some(RunId::from_bytes([3; 16])),
            sensitivity: Sensitivity::Internal,
        }
    }

    fn tool_context() -> ToolCallContext {
        ToolCallContext {
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
                relation_depth: 0,
            },
            tool_batch_id: ToolBatchId::from_bytes([5; 16]),
            tool_call_id: ToolCallId::from_bytes([6; 16]),
        }
    }

    async fn stage(
        store: &InProcessArtifactStore,
        content: &[u8],
        kind: &str,
        media_type: &str,
    ) -> ArtifactRef {
        stage_required_artifact(
            store,
            scope(),
            Bytes::from(content.to_vec()),
            ArtifactMetadata {
                kind: Arc::from(kind),
                media_type: Arc::from(media_type),
                name: None,
                attributes: Metadata::empty(),
            },
        )
        .await
        .expect("stage")
    }

    /// A structurally valid but unstaged artifact reference: usable only
    /// when a test expects rejection before any store fetch happens.
    fn unstaged_artifact_ref() -> ArtifactRef {
        let content = b"clip".as_slice();
        let digest = Digest::blob_content(content);
        let blob = BlobRef::try_new(
            "blob-1",
            "video/mp4",
            u64::try_from(content.len()).expect("length"),
            Some(digest),
            None::<&str>,
        )
        .expect("blob");
        ArtifactRef::try_new(
            ArtifactId::from_bytes([9; 16]),
            "video",
            blob,
            digest,
            Digest::raw_json(b"scope"),
            Metadata::empty(),
        )
        .expect("artifact")
    }

    fn toolset(
        ffmpeg: PathBuf,
        ffprobe: PathBuf,
        scratch: PathBuf,
        timeout: Duration,
    ) -> VideoComposeToolset {
        toolset_with_store(
            ffmpeg,
            ffprobe,
            scratch,
            timeout,
            Arc::new(InProcessArtifactStore::default()),
        )
    }

    fn toolset_with_store(
        ffmpeg: PathBuf,
        ffprobe: PathBuf,
        scratch: PathBuf,
        timeout: Duration,
        store: Arc<dyn ArtifactStore>,
    ) -> VideoComposeToolset {
        VideoComposeToolset::try_new(VideoComposeConfig {
            ffmpeg_path: ffmpeg,
            ffprobe_path: ffprobe,
            artifact_store: store,
            scratch_dir: scratch,
            render_timeout: timeout,
        })
        .expect("toolset")
    }

    fn spec_for(tool_id_name: &str, spec: &[ToolSpec]) -> ToolSpec {
        spec.iter()
            .find(|s| s.model_name.as_ref() == tool_id_name)
            .cloned()
            .expect("tool spec")
    }

    fn validated_call(spec: &ToolSpec, args: &serde_json::Value) -> ValidatedToolCall {
        let bytes = serde_json::to_vec(args).expect("json");
        ValidatedToolCall {
            call: ToolCallBlock::try_new(
                ToolCallId::from_bytes([6; 16]),
                spec.model_name.as_ref(),
                RawJson::parse(bytes).expect("raw"),
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

    #[tokio::test]
    async fn compose_renders_through_the_stub_and_stores_the_output() {
        let stub_dir = tempdir().expect("stub dir");
        let scratch_dir = tempdir().expect("scratch dir");
        write_stub(stub_dir.path(), "ffprobe", FFPROBE_STUB_BODY);
        write_stub(stub_dir.path(), "ffmpeg", FFMPEG_STUB_BODY);
        let store = Arc::new(InProcessArtifactStore::default());
        let clip_a = stage(&store, b"clip-a", "video", "video/mp4").await;
        let clip_b = stage(&store, b"clip-b", "video", "video/mp4").await;
        let tools = toolset_with_store(
            stub_dir.path().join("ffmpeg"),
            stub_dir.path().join("ffprobe"),
            scratch_dir.path().to_path_buf(),
            Duration::from_secs(5),
            store.clone(),
        );
        let spec = spec_for(super::COMPOSE_TOOL_NAME, &tools.tools());
        let args = serde_json::json!({
            "version": 1,
            "clips": [{"artifact": clip_a}, {"artifact": clip_b}],
            "transitions": [{"type": "crossfade", "duration_s": 0.5}],
            "output": {"container": "mp4"},
        });
        let call = validated_call(&spec, &args);
        let mut stream = tools
            .call(tool_context(), call)
            .await
            .expect("call started");
        let item = stream.next().await.expect("item").expect("ok");
        let ToolStreamItem::Completed(result) = item else {
            panic!("expected completion");
        };
        assert!(!result.is_error);
        let payload: serde_json::Value =
            serde_json::from_slice(result.output.as_bytes()).expect("json");
        let artifact: ArtifactRef =
            serde_json::from_value(payload["artifact"].clone()).expect("artifact ref");
        assert_eq!(payload["byte_length"], 1);
        let bytes = store
            .get(scope(), artifact)
            .await
            .expect("round trip fetch");
        assert_eq!(bytes.as_ref(), b"x");

        let args_text =
            std::fs::read_to_string(stub_dir.path().join("ffmpeg-args.txt")).expect("args file");
        assert!(args_text.contains("xfade=transition=fade"));
        assert!(args_text.contains("clip-0-"));
        assert!(args_text.contains("clip-1-"));
    }

    #[tokio::test]
    async fn burned_in_subtitles_reach_ffmpeg() {
        let stub_dir = tempdir().expect("stub dir");
        let scratch_dir = tempdir().expect("scratch dir");
        write_stub(stub_dir.path(), "ffprobe", FFPROBE_STUB_BODY);
        write_stub(stub_dir.path(), "ffmpeg", FFMPEG_STUB_BODY);
        let store = Arc::new(InProcessArtifactStore::default());
        let clip = stage(&store, b"clip", "video", "video/mp4").await;
        let subs = stage(
            &store,
            b"1\n00:00:00,000 --> 00:00:01,000\nHi\n",
            "subtitles",
            "text/srt",
        )
        .await;
        let tools = toolset_with_store(
            stub_dir.path().join("ffmpeg"),
            stub_dir.path().join("ffprobe"),
            scratch_dir.path().to_path_buf(),
            Duration::from_secs(5),
            store,
        );
        let spec = spec_for(super::COMPOSE_TOOL_NAME, &tools.tools());
        let args = serde_json::json!({
            "version": 1,
            "clips": [{"artifact": clip}],
            "subtitles": {"artifact": subs, "mode": "burn_in"},
            "output": {"container": "mp4"},
        });
        let call = validated_call(&spec, &args);
        let mut stream = tools
            .call(tool_context(), call)
            .await
            .expect("call started");
        let item = stream.next().await.expect("item").expect("ok");
        let ToolStreamItem::Completed(result) = item else {
            panic!("expected completion");
        };
        assert!(!result.is_error);
        let args_text =
            std::fs::read_to_string(stub_dir.path().join("ffmpeg-args.txt")).expect("args file");
        assert!(args_text.contains("subtitles="));
        assert!(
            args_text
                .lines()
                .any(|line| line.starts_with("[vc]subtitles=") && line.contains(".srt"))
                || args_text.contains(".srt"),
        );
    }

    #[tokio::test]
    async fn ffmpeg_failure_surfaces_bounded_stderr() {
        let stub_dir = tempdir().expect("stub dir");
        let scratch_dir = tempdir().expect("scratch dir");
        write_stub(stub_dir.path(), "ffprobe", FFPROBE_STUB_BODY);
        write_stub(
            stub_dir.path(),
            "ffmpeg",
            r#"echo "boom: filter parse error" >&2; exit 1"#,
        );
        let store = Arc::new(InProcessArtifactStore::default());
        let clip = stage(&store, b"clip", "video", "video/mp4").await;
        let tools = toolset_with_store(
            stub_dir.path().join("ffmpeg"),
            stub_dir.path().join("ffprobe"),
            scratch_dir.path().to_path_buf(),
            Duration::from_secs(5),
            store,
        );
        let spec = spec_for(super::COMPOSE_TOOL_NAME, &tools.tools());
        let args = serde_json::json!({
            "version": 1,
            "clips": [{"artifact": clip}],
            "output": {"container": "mp4"},
        });
        let call = validated_call(&spec, &args);
        let Err(error) = tools.call(tool_context(), call).await else {
            panic!("ffmpeg must fail")
        };
        assert_eq!(error.code(), VIDEO_COMPOSE_FFMPEG_FAILED);
        assert!(error.message().contains("filter parse error"));
        assert!(error.message().len() <= 2 * 1024);
    }

    #[tokio::test]
    async fn render_timeout_kills_the_child() {
        let stub_dir = tempdir().expect("stub dir");
        let scratch_dir = tempdir().expect("scratch dir");
        write_stub(stub_dir.path(), "ffprobe", FFPROBE_STUB_BODY);
        write_stub(stub_dir.path(), "ffmpeg", "sleep 30");
        let store = Arc::new(InProcessArtifactStore::default());
        let clip = stage(&store, b"clip", "video", "video/mp4").await;
        let tools = toolset_with_store(
            stub_dir.path().join("ffmpeg"),
            stub_dir.path().join("ffprobe"),
            scratch_dir.path().to_path_buf(),
            Duration::from_millis(200),
            store,
        );
        let spec = spec_for(super::COMPOSE_TOOL_NAME, &tools.tools());
        let args = serde_json::json!({
            "version": 1,
            "clips": [{"artifact": clip}],
            "output": {"container": "mp4"},
        });
        let call = validated_call(&spec, &args);
        let Err(error) = tools.call(tool_context(), call).await else {
            panic!("must time out")
        };
        assert_eq!(error.code(), VIDEO_COMPOSE_TIMEOUT);
    }

    #[tokio::test]
    async fn probe_media_maps_ffprobe_json() {
        let stub_dir = tempdir().expect("stub dir");
        let scratch_dir = tempdir().expect("scratch dir");
        write_stub(stub_dir.path(), "ffprobe", FFPROBE_STUB_BODY);
        write_stub(stub_dir.path(), "ffmpeg", FFMPEG_STUB_BODY);
        let store = Arc::new(InProcessArtifactStore::default());
        let clip = stage(&store, b"clip", "video", "video/mp4").await;
        let tools = toolset_with_store(
            stub_dir.path().join("ffmpeg"),
            stub_dir.path().join("ffprobe"),
            scratch_dir.path().to_path_buf(),
            Duration::from_secs(5),
            store,
        );
        let spec = spec_for(super::PROBE_TOOL_NAME, &tools.tools());
        let call = validated_call(&spec, &serde_json::json!({ "artifact": clip }));
        let mut stream = tools
            .call(tool_context(), call)
            .await
            .expect("call started");
        let item = stream.next().await.expect("item").expect("ok");
        let ToolStreamItem::Completed(result) = item else {
            panic!("expected completion");
        };
        let payload: serde_json::Value =
            serde_json::from_slice(result.output.as_bytes()).expect("json");
        assert_eq!(payload["duration_s"], 4.0);
        assert_eq!(payload["width"], 640);
        assert_eq!(payload["fps"], 24.0);
        assert!(payload["has_audio"].as_bool().expect("has_audio bool"));
        assert_eq!(payload["media_type"], "video/mp4");
    }

    #[tokio::test]
    async fn invalid_spec_is_rejected_before_any_process_runs() {
        let stub_dir = tempdir().expect("stub dir");
        let scratch_dir = tempdir().expect("scratch dir");
        write_stub(stub_dir.path(), "ffprobe", FFPROBE_STUB_BODY);
        write_stub(stub_dir.path(), "ffmpeg", FFMPEG_STUB_BODY);
        let tools = toolset(
            stub_dir.path().join("ffmpeg"),
            stub_dir.path().join("ffprobe"),
            scratch_dir.path().to_path_buf(),
            Duration::from_secs(5),
        );
        let spec = spec_for(super::COMPOSE_TOOL_NAME, &tools.tools());
        let clip = unstaged_artifact_ref();
        let args = serde_json::json!({
            "version": 1,
            "clips": [{"artifact": clip.clone()}, {"artifact": clip.clone()}, {"artifact": clip}],
            "transitions": [{"type": "cut"}],
            "output": {"container": "mp4"},
        });
        let call = validated_call(&spec, &args);
        let Err(error) = tools.call(tool_context(), call).await else {
            panic!("mismatched transitions must be rejected")
        };
        assert_eq!(error.code(), VIDEO_COMPOSE_SPEC_INVALID);
        assert!(!stub_dir.path().join("ffmpeg-args.txt").exists());
    }

    #[tokio::test]
    async fn oversized_render_output_fails_closed() {
        let stub_dir = tempdir().expect("stub dir");
        let scratch_dir = tempdir().expect("scratch dir");
        write_stub(stub_dir.path(), "ffprobe", FFPROBE_STUB_BODY);
        write_stub(
            stub_dir.path(),
            "ffmpeg",
            r#"d=$(dirname "$0"); printf '%s\n' "$@" > "$d/ffmpeg-args.txt"; eval last=\"\${$#}\"; head -c 64 /dev/zero > "$last""#,
        );
        // A ceiling small enough to reject the 64-byte render output while
        // still accepting the 1-byte fixture clip staged below.
        let store: Arc<dyn ArtifactStore> =
            Arc::new(InProcessArtifactStore::default().with_max_artifact_bytes(8));
        let clip = stage_required_artifact(
            store.as_ref(),
            scope(),
            Bytes::from_static(b"c"),
            ArtifactMetadata {
                kind: Arc::from("video"),
                media_type: Arc::from("video/mp4"),
                name: None,
                attributes: Metadata::empty(),
            },
        )
        .await
        .expect("stage tiny clip");
        let tools = toolset_with_store(
            stub_dir.path().join("ffmpeg"),
            stub_dir.path().join("ffprobe"),
            scratch_dir.path().to_path_buf(),
            Duration::from_secs(5),
            store,
        );
        let spec = spec_for(super::COMPOSE_TOOL_NAME, &tools.tools());
        let args = serde_json::json!({
            "version": 1,
            "clips": [{"artifact": clip}],
            "output": {"container": "mp4"},
        });
        let call = validated_call(&spec, &args);
        let Err(error) = tools.call(tool_context(), call).await else {
            panic!("oversized render output must fail closed")
        };
        assert_eq!(error.code(), VIDEO_COMPOSE_LIMIT_EXCEEDED);
    }

    #[test]
    fn video_compose_is_not_a_wasm_host_sdk_dependency() {
        let manifest = include_str!("../../../../crates/finstack-ai/Cargo.toml");
        let wasm_host = manifest
            .lines()
            .find(|line| line.contains("wasm-host ="))
            .expect("wasm-host feature");
        assert!(
            !wasm_host.contains("finstack-ai-tools-video-compose"),
            "video compose must stay off the wasm-host feature graph"
        );
        assert!(manifest.contains("dep:finstack-ai-tools-video-compose"));
    }
}
