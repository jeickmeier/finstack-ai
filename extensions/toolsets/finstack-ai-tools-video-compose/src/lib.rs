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

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use finstack_ai_kernel::{
    ArtifactRef, EffectId, ErrorCategory, Metadata, RawJson, RetrySafety, Sensitivity,
    ToolExecutionMode, ToolId, ValidatedToolCall,
};
use finstack_ai_runtime::artifact::{
    ArtifactError, ArtifactMetadata, ArtifactScope, ArtifactStore, stage_required_artifact,
    validate_retrieved_artifact,
};
use finstack_ai_runtime::ids::{Clock as _, SystemClock};
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

use crate::exec::{Budget, InputContainer, sniff_container};
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
/// Stable code for a `validate_spec` rejection (the message carries the reason).
pub const VIDEO_COMPOSE_SPEC_INVALID: &str = "video_compose_spec_invalid";
/// Stable code for an `ffmpeg`/`ffprobe` process failure: a non-zero exit,
/// or a child that started but could not be waited on.
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
    /// Scratch root for intermediate render files. Created (recursively)
    /// during construction if it does not already exist. Each call works in
    /// its own subdirectory, removed when the call finishes. The path may
    /// only contain `[A-Za-z0-9/_.-]`: it is spliced into `ffmpeg` filter
    /// options, so no other character is allowed to reach a filtergraph.
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
    /// zero or over one hour, or `scratch_dir` contained a character outside
    /// `[A-Za-z0-9/_.-]` or could not be created.
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
    ffmpeg_path: PathBuf,
    ffprobe_path: PathBuf,
    artifact_store: Arc<dyn ArtifactStore>,
    scratch_dir: PathBuf,
    render_timeout: Duration,
}

impl std::fmt::Debug for VideoComposeToolset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VideoComposeToolset")
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
    /// one hour, `scratch_dir` is empty, contains a character outside
    /// `[A-Za-z0-9/_.-]`, or cannot be created, or a built-in [`ToolSpec`]
    /// fails its own validation.
    #[allow(clippy::too_many_lines)]
    pub fn try_new(config: VideoComposeConfig) -> Result<Self, VideoComposeError> {
        if !scratch_dir_is_filter_safe(&config.scratch_dir) {
            return Err(VideoComposeError::ConfigInvalid {
                reason: "scratch_dir must be non-empty and contain only [A-Za-z0-9/_.-]",
            });
        }
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

        let compose_spec = ToolSpec {
            id: ToolId::parse(COMPOSE_TOOL_ID).map_err(|_| VideoComposeError::ConfigInvalid {
                reason: "invalid_tool_id",
            })?,
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
            id: ToolId::parse(PROBE_TOOL_ID).map_err(|_| VideoComposeError::ConfigInvalid {
                reason: "invalid_tool_id",
            })?,
            model_name: Arc::from(PROBE_TOOL_NAME),
            title: Arc::from("Probe media"),
            description: Arc::from(
                "Probe duration, dimensions, and streams of one stored mp4 or matroska/webm artifact.",
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
            ffmpeg_path: config.ffmpeg_path,
            ffprobe_path: config.ffprobe_path,
            artifact_store: config.artifact_store,
            scratch_dir: config.scratch_dir,
            render_timeout: config.render_timeout,
        })
    }
}

/// Whether `path` can be spliced verbatim into an `ffmpeg` filter option:
/// non-empty UTF-8 drawn from `[A-Za-z0-9/_.-]` only.
fn scratch_dir_is_filter_safe(path: &Path) -> bool {
    path.to_str().is_some_and(|text| {
        !text.is_empty()
            && text
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'_' | b'.' | b'-'))
    })
}

/// The wall-clock budget for one call: the configured `render_timeout`,
/// shortened to whatever remains of the call's semantic deadline.
///
/// A call without a deadline gets the full timeout. If the system clock
/// cannot be read while a deadline is set, the budget is zero (fail closed).
fn call_budget(ctx: &ToolCallContext, render_timeout: Duration) -> Budget {
    let from_deadline = if ctx.run.deadline.is_some() {
        SystemClock
            .now()
            .ok()
            .and_then(|now| ctx.run.deadline_remaining(now))
            .unwrap_or(Duration::ZERO)
    } else {
        render_timeout
    };
    Budget::start(render_timeout.min(from_deadline))
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
        let tools = Arc::clone(&self.tools);
        let ffmpeg_path = self.ffmpeg_path.clone();
        let ffprobe_path = self.ffprobe_path.clone();
        let artifact_store = Arc::clone(&self.artifact_store);
        let scratch_dir = self.scratch_dir.clone();
        let render_timeout = self.render_timeout;
        Box::pin(async move {
            verify_authority(&ctx)?;
            let tool_name = call.call.tool_name();
            let known = tools
                .iter()
                .any(|spec| spec.id == call.tool_id && spec.model_name.as_ref() == tool_name);
            match tool_name {
                COMPOSE_TOOL_NAME if known => {
                    handle_compose(
                        &ctx,
                        &call,
                        &ffmpeg_path,
                        &ffprobe_path,
                        &artifact_store,
                        &scratch_dir,
                        render_timeout,
                    )
                    .await
                }
                PROBE_TOOL_NAME if known => {
                    handle_probe(
                        &ctx,
                        &call,
                        &ffprobe_path,
                        &artifact_store,
                        &scratch_dir,
                        render_timeout,
                    )
                    .await
                }
                _ => Err(tool_error(
                    VIDEO_COMPOSE_INVALID_ARGUMENTS,
                    ErrorCategory::Validation,
                    "video compose call identity is invalid",
                )),
            }
        })
    }
}

/// Exact scope binding for every artifact operation this Toolset performs.
fn artifact_scope(ctx: &ToolCallContext) -> ArtifactScope {
    ArtifactScope {
        tenant_scope: Arc::clone(&ctx.run.locator.tenant_scope),
        session_id: ctx.run.locator.session_id,
        run_id: Some(ctx.run.locator.run_id),
        sensitivity: Sensitivity::Internal,
    }
}

/// Fetch one stored artifact to a file under `dir` and return its path.
///
/// # Errors
///
/// Returns [`VIDEO_COMPOSE_MEDIA_FAILURE`] when the store read fails, the
/// returned bytes fail independent verification, or the scratch write fails.
async fn fetch_artifact_to_file(
    store: &Arc<dyn ArtifactStore>,
    scope: &ArtifactScope,
    artifact: &ArtifactRef,
    dir: &Path,
    stem: &str,
) -> Result<(PathBuf, Bytes), ToolError> {
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
    Ok((path, content))
}

/// Fetch one stored media artifact to a file under `dir` and pin its
/// container from the staged bytes.
///
/// # Errors
///
/// Propagates [`fetch_artifact_to_file`] failures and returns
/// [`VIDEO_COMPOSE_INVALID_ARGUMENTS`] when the bytes are neither an ISO
/// base media file nor an EBML/Matroska stream.
async fn fetch_media_to_file(
    store: &Arc<dyn ArtifactStore>,
    scope: &ArtifactScope,
    artifact: &ArtifactRef,
    dir: &Path,
    stem: &str,
) -> Result<(PathBuf, InputContainer), ToolError> {
    let (path, content) = fetch_artifact_to_file(store, scope, artifact, dir, stem).await?;
    let container = sniff_container(&content).ok_or_else(|| {
        tool_error(
            VIDEO_COMPOSE_INVALID_ARGUMENTS,
            ErrorCategory::Validation,
            "media artifact is not an mp4 or matroska/webm container",
        )
    })?;
    Ok((path, container))
}

/// Stage a rendered scratch file as a required output artifact.
///
/// The file size is checked against `max_artifact_bytes` before any of it is
/// read: `ffmpeg -fs` stops writing at the ceiling but still exits
/// successfully, so this is the gate that keeps an oversized render out of
/// memory.
///
/// # Errors
///
/// Returns [`VIDEO_COMPOSE_LIMIT_EXCEEDED`] when the rendered file is larger
/// than `max_artifact_bytes` or the store rejects the content as oversized,
/// or [`VIDEO_COMPOSE_MEDIA_FAILURE`] for any other read or staging failure.
async fn stage_output(
    store: &Arc<dyn ArtifactStore>,
    scope: &ArtifactScope,
    path: &Path,
    media_type: &str,
    max_artifact_bytes: u64,
) -> Result<(ArtifactRef, u64), ToolError> {
    let byte_length = tokio::fs::metadata(path)
        .await
        .map_err(|_| {
            tool_error(
                VIDEO_COMPOSE_MEDIA_FAILURE,
                ErrorCategory::Tool,
                "render output read failed",
            )
        })?
        .len();
    if byte_length > max_artifact_bytes {
        return Err(tool_error(
            VIDEO_COMPOSE_LIMIT_EXCEEDED,
            ErrorCategory::Limit,
            "render output exceeds the artifact store limit",
        ));
    }
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
            "artifact store rejected the render output as oversized",
        ),
        _ => tool_error(
            VIDEO_COMPOSE_MEDIA_FAILURE,
            ErrorCategory::Tool,
            "render output staging failed",
        ),
    })?;
    Ok((artifact, byte_length))
}

/// One call's private scratch directory, `<scratch_root>/<render stem>/`.
///
/// Every staged clip, probe target, side input, and rendered output of the
/// call lives under it, and the whole directory is removed when the guard
/// drops, whether the call succeeded or failed early. Removal is best
/// effort: a failure leaves files behind but never fails the call.
struct ScratchGuard {
    dir: PathBuf,
}

impl ScratchGuard {
    /// Create `<root>/<stem>/` and take ownership of it.
    ///
    /// # Errors
    ///
    /// Returns [`VIDEO_COMPOSE_MEDIA_FAILURE`] when the directory cannot be
    /// created.
    async fn create(root: &Path, stem: &str) -> Result<Self, ToolError> {
        let dir = root.join(stem);
        tokio::fs::create_dir_all(&dir).await.map_err(|_| {
            tool_error(
                VIDEO_COMPOSE_MEDIA_FAILURE,
                ErrorCategory::Tool,
                "scratch directory could not be created",
            )
        })?;
        Ok(Self { dir })
    }

    fn dir(&self) -> &Path {
        &self.dir
    }
}

impl Drop for ScratchGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
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
    let budget = call_budget(ctx, render_timeout);
    let scope = artifact_scope(ctx);
    let scratch = ScratchGuard::create(scratch_dir, &render_file_stem(ctx.run.effect_id)).await?;
    let (path, container) = fetch_media_to_file(
        artifact_store,
        &scope,
        &args.artifact,
        scratch.dir(),
        "probe",
    )
    .await?;
    let probe = exec::probe(
        ffprobe_path,
        &path,
        container,
        &budget,
        &ctx.run.cancellation,
    )
    .await?;
    drop(scratch);

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

    let budget = call_budget(ctx, render_timeout);
    let max_artifact_bytes =
        u64::try_from(artifact_store.limits().max_artifact_bytes).unwrap_or(u64::MAX);
    let scope = artifact_scope(ctx);
    let stem = render_file_stem(ctx.run.effect_id);
    let scratch = ScratchGuard::create(scratch_dir, &stem).await?;
    let dir = scratch.dir();

    let mut staged_clips = Vec::with_capacity(spec.clips.len());
    for (idx, clip) in spec.clips.iter().enumerate() {
        staged_clips.push(
            fetch_media_to_file(
                artifact_store,
                &scope,
                &clip.artifact,
                dir,
                &format!("clip-{idx}"),
            )
            .await?,
        );
    }

    let audio_path = match &spec.audio {
        Some(audio) => Some(
            fetch_artifact_to_file(artifact_store, &scope, &audio.artifact, dir, "audio")
                .await?
                .0,
        ),
        None => None,
    };

    let subtitles_path = match &spec.subtitles {
        Some(subtitles) => {
            let (staged, _) =
                fetch_artifact_to_file(artifact_store, &scope, &subtitles.artifact, dir, "subs")
                    .await?;
            let srt_path = staged.with_extension("srt");
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

    let mut clip_inputs = Vec::with_capacity(staged_clips.len());
    for (path, container) in staged_clips {
        let probed = exec::probe(
            ffprobe_path,
            &path,
            container,
            &budget,
            &ctx.run.cancellation,
        )
        .await?;
        clip_inputs.push(ClipInput {
            path,
            container,
            duration_s: probed.duration_s,
            width: probed.width,
            height: probed.height,
            fps: probed.fps,
            has_audio: probed.has_audio,
        });
    }

    let output_path = dir.join(format!("{stem}.{}", spec.output.container.extension()));

    let args = build_ffmpeg_args(
        &spec,
        &clip_inputs,
        audio_path.as_deref(),
        subtitles_path.as_deref(),
        &output_path,
        max_artifact_bytes,
    )
    .map_err(|reason| {
        tool_error(
            VIDEO_COMPOSE_SPEC_INVALID,
            ErrorCategory::Validation,
            reason,
        )
    })?;

    exec::render(ffmpeg_path, &args, &budget, &ctx.run.cancellation).await?;

    // The output container is the spec's, never sniffed: the toolset wrote
    // it, so the demuxer is pinned from what was asked for.
    let output_container = match spec.output.container {
        Container::Mp4 => InputContainer::Mp4,
        Container::Webm => InputContainer::Matroska,
    };
    let probed = exec::probe(
        ffprobe_path,
        &output_path,
        output_container,
        &budget,
        &ctx.run.cancellation,
    )
    .await?;

    let (artifact, byte_length) = stage_output(
        artifact_store,
        &scope,
        &output_path,
        spec.output.container.media_type(),
        max_artifact_bytes,
    )
    .await?;
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

/// Derive the scratch render's file stem from the effect id.
///
/// The stem must be a plain shell- and ffmpeg-safe token: it lands on the
/// `ffmpeg` argument vector and in the staged artifact's name. Formatting the
/// id with `{:?}`/`{:x?}` would embed the `EffectId(...)` wrapper — brackets,
/// quotes and spaces — so the 16 UUID bytes are hex-encoded explicitly here.
fn render_file_stem(effect_id: EffectId) -> String {
    let mut stem = String::with_capacity(39);
    stem.push_str("render-");
    for byte in effect_id.as_bytes() {
        let _ = write!(stem, "{byte:02x}");
    }
    stem
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

pub(crate) fn tool_error(
    code: &'static str,
    category: ErrorCategory,
    message: impl AsRef<str>,
) -> ToolError {
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
        Digest, EffectId, EffectOutputContract, EffectOutputKind, LaneId, Metadata,
        OperationLocator, PrincipalRef, RawJson, RunId, Sensitivity, SessionId, Timestamp,
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

    use finstack_ai_runtime::ids::{Clock as _, SystemClock};

    use super::{
        ArtifactRef, VIDEO_COMPOSE_CONFIG_INVALID, VIDEO_COMPOSE_FFMPEG_FAILED,
        VIDEO_COMPOSE_INVALID_ARGUMENTS, VIDEO_COMPOSE_LIMIT_EXCEEDED, VIDEO_COMPOSE_SPEC_INVALID,
        VIDEO_COMPOSE_TIMEOUT, VideoComposeConfig, VideoComposeError, VideoComposeToolset,
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

    /// A minimal ISO base media file header (`ftyp` at offset 4) followed by
    /// `tag`, so the staged bytes sniff as mp4 while staying distinct.
    fn mp4_bytes(tag: &[u8]) -> Vec<u8> {
        let mut bytes = vec![
            0, 0, 0, 0x18, b'f', b't', b'y', b'p', b'i', b's', b'o', b'm',
        ];
        bytes.extend_from_slice(tag);
        bytes
    }

    /// Lines of the argv the fake `ffmpeg` recorded.
    fn recorded_args(stub_dir: &Path) -> Vec<String> {
        std::fs::read_to_string(stub_dir.join("ffmpeg-args.txt"))
            .expect("args file")
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn compose_args(clips: &[ArtifactRef]) -> serde_json::Value {
        serde_json::json!({
            "version": 1,
            "clips": clips.iter().map(|c| serde_json::json!({"artifact": c})).collect::<Vec<_>>(),
            "output": {"container": "mp4"},
        })
    }

    fn deadline_in(offset_ms: i64) -> Timestamp {
        let now = SystemClock.now().expect("clock").as_unix_ms();
        Timestamp::from_unix_ms(now + offset_ms).expect("deadline")
    }

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
        let clip_a = stage(&store, &mp4_bytes(b"clip-a"), "video", "video/mp4").await;
        let clip_b = stage(&store, &mp4_bytes(b"clip-b"), "video", "video/mp4").await;
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
        // The output name is derived from the effect id as plain hex: no
        // `EffectId(..)` Debug wrapper leaking brackets or spaces into a path
        // that ends up on an argv and in the staged artifact name.
        let expected_stem = format!("render-{}.mp4", "04".repeat(16));
        assert!(args_text.contains(&expected_stem), "{args_text}");
        assert!(
            !args_text.contains('(') && !args_text.contains(')'),
            "{args_text}"
        );
    }

    #[test]
    fn render_file_stem_is_plain_lowercase_hex() {
        let stem = super::render_file_stem(EffectId::from_bytes([
            0, 1, 0xab, 0xff, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15,
        ]));
        assert_eq!(stem, "render-0001abff0405060708090a0b0c0d0e0f");
        assert!(
            stem.chars()
                .all(|c| c == '-' || c.is_ascii_lowercase() || c.is_ascii_digit())
        );
    }

    #[tokio::test]
    async fn burned_in_subtitles_reach_ffmpeg() {
        let stub_dir = tempdir().expect("stub dir");
        let scratch_dir = tempdir().expect("scratch dir");
        write_stub(stub_dir.path(), "ffprobe", FFPROBE_STUB_BODY);
        write_stub(stub_dir.path(), "ffmpeg", FFMPEG_STUB_BODY);
        let store = Arc::new(InProcessArtifactStore::default());
        let clip = stage(&store, &mp4_bytes(b"clip"), "video", "video/mp4").await;
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
        let clip = stage(&store, &mp4_bytes(b"clip"), "video", "video/mp4").await;
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
        write_stub(stub_dir.path(), "ffmpeg", "exec sleep 30");
        let store = Arc::new(InProcessArtifactStore::default());
        let clip = stage(&store, &mp4_bytes(b"clip"), "video", "video/mp4").await;
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
        let clip = stage(&store, &mp4_bytes(b"clip"), "video", "video/mp4").await;
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
        // Structurally valid but unstaged: the spec must be rejected before
        // any store fetch happens.
        let clip = crate::spec::tests::artifact_ref();
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
        // still accepting the 12-byte fixture clip staged below.
        let store: Arc<dyn ArtifactStore> =
            Arc::new(InProcessArtifactStore::default().with_max_artifact_bytes(16));
        let clip = stage_required_artifact(
            store.as_ref(),
            scope(),
            Bytes::from(mp4_bytes(b"")),
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

    #[tokio::test]
    async fn output_over_limit_is_rejected_before_read() {
        let stub_dir = tempdir().expect("stub dir");
        let scratch_dir = tempdir().expect("scratch dir");
        write_stub(stub_dir.path(), "ffprobe", FFPROBE_STUB_BODY);
        write_stub(
            stub_dir.path(),
            "ffmpeg",
            r#"d=$(dirname "$0"); printf '%s\n' "$@" > "$d/ffmpeg-args.txt"; eval last=\"\${$#}\"; head -c 65536 /dev/zero > "$last""#,
        );
        let store: Arc<dyn ArtifactStore> =
            Arc::new(InProcessArtifactStore::default().with_max_artifact_bytes(16));
        let clip = stage_required_artifact(
            store.as_ref(),
            scope(),
            Bytes::from(mp4_bytes(b"")),
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
        let call = validated_call(&spec, &compose_args(&[clip]));
        let Err(error) = tools.call(tool_context(), call).await else {
            panic!("oversized render output must fail closed")
        };
        assert_eq!(error.code(), VIDEO_COMPOSE_LIMIT_EXCEEDED);
        // The metadata gate fires, not the store's post-read rejection.
        assert_eq!(
            error.message(),
            "render output exceeds the artifact store limit"
        );
        let args = recorded_args(stub_dir.path());
        let fs = args.iter().position(|a| a == "-fs").expect("-fs present");
        assert_eq!(args[fs + 1], "16", "ffmpeg is told the store ceiling");
        assert_eq!(
            fs + 2,
            args.len() - 1,
            "-fs is the last option before the output"
        );
    }

    #[tokio::test]
    async fn scratch_dir_is_per_call_and_removed() {
        let stub_dir = tempdir().expect("stub dir");
        let scratch_dir = tempdir().expect("scratch dir");
        write_stub(stub_dir.path(), "ffprobe", FFPROBE_STUB_BODY);
        // Record the directory the output lands in and what sits beside it.
        write_stub(
            stub_dir.path(),
            "ffmpeg",
            r#"d=$(dirname "$0"); eval last=\"\${$#}\"; dirname "$last" > "$d/out-dir.txt"; ls "$(dirname "$last")" > "$d/listing.txt"; printf 'x' > "$last""#,
        );
        let store = Arc::new(InProcessArtifactStore::default());
        let clip_a = stage(&store, &mp4_bytes(b"clip-a"), "video", "video/mp4").await;
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
            "clips": [{"artifact": clip_a}],
            "subtitles": {"artifact": subs, "mode": "burn_in"},
            "output": {"container": "mp4"},
        });
        let call = validated_call(&spec, &args);
        let mut stream = tools
            .call(tool_context(), call)
            .await
            .expect("call started");
        let item = stream.next().await.expect("item").expect("ok");
        assert!(matches!(item, ToolStreamItem::Completed(_)));

        let expected_dir = scratch_dir
            .path()
            .join(format!("render-{}", "04".repeat(16)));
        let out_dir =
            std::fs::read_to_string(stub_dir.path().join("out-dir.txt")).expect("out dir");
        assert_eq!(
            PathBuf::from(out_dir.trim()),
            expected_dir,
            "every staged file lives under the per-call directory"
        );
        let listing =
            std::fs::read_to_string(stub_dir.path().join("listing.txt")).expect("listing");
        assert!(listing.contains("clip-0-"), "{listing}");
        assert!(listing.contains(".srt"), "{listing}");
        assert!(
            !expected_dir.exists(),
            "the per-call directory is removed after the call"
        );
        assert!(
            std::fs::read_dir(scratch_dir.path())
                .expect("scratch root")
                .next()
                .is_none(),
            "nothing is left under the scratch root"
        );
    }

    #[tokio::test]
    async fn scratch_dir_is_removed_when_the_call_fails() {
        let stub_dir = tempdir().expect("stub dir");
        let scratch_dir = tempdir().expect("scratch dir");
        write_stub(stub_dir.path(), "ffprobe", FFPROBE_STUB_BODY);
        write_stub(stub_dir.path(), "ffmpeg", "exit 1");
        let store = Arc::new(InProcessArtifactStore::default());
        let clip = stage(&store, &mp4_bytes(b"clip"), "video", "video/mp4").await;
        let tools = toolset_with_store(
            stub_dir.path().join("ffmpeg"),
            stub_dir.path().join("ffprobe"),
            scratch_dir.path().to_path_buf(),
            Duration::from_secs(5),
            store,
        );
        let spec = spec_for(super::COMPOSE_TOOL_NAME, &tools.tools());
        let call = validated_call(&spec, &compose_args(&[clip]));
        assert!(tools.call(tool_context(), call).await.is_err());
        assert!(
            std::fs::read_dir(scratch_dir.path())
                .expect("scratch root")
                .next()
                .is_none(),
            "the per-call directory is removed on failure too"
        );
    }

    #[test]
    fn scratch_dir_charset_is_validated() {
        let store: Arc<dyn ArtifactStore> = Arc::new(InProcessArtifactStore::default());
        for bad in [
            "/tmp/with space",
            "/tmp/quote'd",
            "/tmp/colon:x",
            "/tmp/back\\slash",
            "",
        ] {
            let error = VideoComposeToolset::try_new(VideoComposeConfig {
                ffmpeg_path: PathBuf::from("/usr/bin/ffmpeg"),
                ffprobe_path: PathBuf::from("/usr/bin/ffprobe"),
                artifact_store: Arc::clone(&store),
                scratch_dir: PathBuf::from(bad),
                render_timeout: Duration::from_secs(5),
            })
            .expect_err(bad);
            let VideoComposeError::ConfigInvalid { reason } = error;
            assert!(reason.contains("scratch_dir"), "{bad}: {reason}");
            assert!(error.to_string().starts_with(VIDEO_COMPOSE_CONFIG_INVALID));
        }
        let ok = tempdir().expect("dir");
        assert!(super::scratch_dir_is_filter_safe(ok.path()));
        assert!(super::scratch_dir_is_filter_safe(Path::new(
            "/var/tmp/finstack_video-compose.v1"
        )));
    }

    #[tokio::test]
    async fn unknown_container_is_rejected() {
        let stub_dir = tempdir().expect("stub dir");
        let scratch_dir = tempdir().expect("scratch dir");
        write_stub(
            stub_dir.path(),
            "ffprobe",
            &format!("touch \"$(dirname \"$0\")/probed\"; {FFPROBE_STUB_BODY}"),
        );
        write_stub(stub_dir.path(), "ffmpeg", FFMPEG_STUB_BODY);
        let store = Arc::new(InProcessArtifactStore::default());
        let not_video = stage(&store, b"RIFF....WAVEfmt ", "video", "video/mp4").await;
        let tools = toolset_with_store(
            stub_dir.path().join("ffmpeg"),
            stub_dir.path().join("ffprobe"),
            scratch_dir.path().to_path_buf(),
            Duration::from_secs(5),
            store,
        );
        let compose = spec_for(super::COMPOSE_TOOL_NAME, &tools.tools());
        let call = validated_call(&compose, &compose_args(std::slice::from_ref(&not_video)));
        let Err(error) = tools.call(tool_context(), call).await else {
            panic!("unknown container must be rejected")
        };
        assert_eq!(error.code(), VIDEO_COMPOSE_INVALID_ARGUMENTS);

        let probe = spec_for(super::PROBE_TOOL_NAME, &tools.tools());
        let call = validated_call(&probe, &serde_json::json!({ "artifact": not_video }));
        let Err(error) = tools.call(tool_context(), call).await else {
            panic!("unknown container must be rejected by probe_media too")
        };
        assert_eq!(error.code(), VIDEO_COMPOSE_INVALID_ARGUMENTS);
        assert!(
            !stub_dir.path().join("probed").exists(),
            "ffprobe never runs on an unpinned container"
        );
    }

    #[tokio::test]
    async fn argv_pins_container_format() {
        let stub_dir = tempdir().expect("stub dir");
        let scratch_dir = tempdir().expect("scratch dir");
        write_stub(
            stub_dir.path(),
            "ffprobe",
            &format!(
                r#"d=$(dirname "$0"); printf '%s\n' "$@" >> "$d/ffprobe-args.txt"; {FFPROBE_STUB_BODY}"#
            ),
        );
        write_stub(stub_dir.path(), "ffmpeg", FFMPEG_STUB_BODY);
        let store = Arc::new(InProcessArtifactStore::default());
        let mp4 = stage(&store, &mp4_bytes(b"a"), "video", "video/mp4").await;
        let webm = stage(
            &store,
            &[
                0x1A, 0x45, 0xDF, 0xA3, 0x9F, 0x42, 0x86, 0x81, b'w', b'e', b'b', b'm',
            ],
            "video",
            "video/webm",
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
            "clips": [{"artifact": mp4}, {"artifact": webm}],
            "output": {"container": "webm"},
        });
        let call = validated_call(&spec, &args);
        let mut stream = tools
            .call(tool_context(), call)
            .await
            .expect("call started");
        let item = stream.next().await.expect("item").expect("ok");
        assert!(matches!(item, ToolStreamItem::Completed(_)));

        let ffmpeg = recorded_args(stub_dir.path()).join(" ");
        assert!(ffmpeg.contains("-f mov -i "), "{ffmpeg}");
        assert!(ffmpeg.contains("-f matroska -i "), "{ffmpeg}");
        let ffprobe =
            std::fs::read_to_string(stub_dir.path().join("ffprobe-args.txt")).expect("probe args");
        let probe_lines: Vec<&str> = ffprobe.lines().collect();
        let pinned: Vec<&str> = probe_lines
            .windows(2)
            .filter(|w| w[0] == "-f")
            .map(|w| w[1])
            .collect();
        // clip 0 (mp4), clip 1 (webm), then the rendered webm output.
        assert_eq!(pinned, ["mov", "matroska", "matroska"]);
        for w in probe_lines.windows(3).filter(|w| w[0] == "-f") {
            assert!(
                !w[2].starts_with('-'),
                "-f immediately precedes the input: {w:?}"
            );
        }
    }

    #[tokio::test]
    async fn trim_beyond_probed_duration_is_rejected() {
        let stub_dir = tempdir().expect("stub dir");
        let scratch_dir = tempdir().expect("scratch dir");
        write_stub(stub_dir.path(), "ffprobe", FFPROBE_STUB_BODY);
        write_stub(stub_dir.path(), "ffmpeg", FFMPEG_STUB_BODY);
        let store = Arc::new(InProcessArtifactStore::default());
        let clip = stage(&store, &mp4_bytes(b"clip"), "video", "video/mp4").await;
        let tools = toolset_with_store(
            stub_dir.path().join("ffmpeg"),
            stub_dir.path().join("ffprobe"),
            scratch_dir.path().to_path_buf(),
            Duration::from_secs(5),
            store,
        );
        let spec = spec_for(super::COMPOSE_TOOL_NAME, &tools.tools());
        // The stub probes every clip at 4.0 s.
        let args = serde_json::json!({
            "version": 1,
            "clips": [{"artifact": clip, "trim": {"start_s": 1.0, "end_s": 4.5}}],
            "output": {"container": "mp4"},
        });
        let call = validated_call(&spec, &args);
        let Err(error) = tools.call(tool_context(), call).await else {
            panic!("trim past the probed end must be rejected")
        };
        assert_eq!(error.code(), VIDEO_COMPOSE_SPEC_INVALID);
        assert_eq!(
            error.message(),
            "clip trim exceeds the probed clip duration"
        );
        assert!(!stub_dir.path().join("ffmpeg-args.txt").exists());
    }

    #[tokio::test]
    async fn transition_longer_than_clip_is_rejected() {
        let stub_dir = tempdir().expect("stub dir");
        let scratch_dir = tempdir().expect("scratch dir");
        write_stub(stub_dir.path(), "ffprobe", FFPROBE_STUB_BODY);
        write_stub(stub_dir.path(), "ffmpeg", FFMPEG_STUB_BODY);
        let store = Arc::new(InProcessArtifactStore::default());
        let clip_a = stage(&store, &mp4_bytes(b"clip-a"), "video", "video/mp4").await;
        let clip_b = stage(&store, &mp4_bytes(b"clip-b"), "video", "video/mp4").await;
        let tools = toolset_with_store(
            stub_dir.path().join("ffmpeg"),
            stub_dir.path().join("ffprobe"),
            scratch_dir.path().to_path_buf(),
            Duration::from_secs(5),
            store,
        );
        let spec = spec_for(super::COMPOSE_TOOL_NAME, &tools.tools());
        // Both clips probe at 4.0 s; a 5 s crossfade cannot fit either.
        let args = serde_json::json!({
            "version": 1,
            "clips": [{"artifact": clip_a}, {"artifact": clip_b}],
            "transitions": [{"type": "crossfade", "duration_s": 5.0}],
            "output": {"container": "mp4"},
        });
        let call = validated_call(&spec, &args);
        let Err(error) = tools.call(tool_context(), call).await else {
            panic!("over-long transition must be rejected")
        };
        assert_eq!(error.code(), VIDEO_COMPOSE_SPEC_INVALID);
        assert_eq!(
            error.message(),
            "transition is longer than an adjacent clip"
        );
        assert!(!stub_dir.path().join("ffmpeg-args.txt").exists());
    }

    #[tokio::test]
    async fn render_budget_is_min_of_timeout_and_deadline() {
        let stub_dir = tempdir().expect("stub dir");
        let scratch_dir = tempdir().expect("scratch dir");
        write_stub(
            stub_dir.path(),
            "ffprobe",
            &format!("touch \"$(dirname \"$0\")/probed\"; {FFPROBE_STUB_BODY}"),
        );
        write_stub(stub_dir.path(), "ffmpeg", "exec sleep 30");
        let store = Arc::new(InProcessArtifactStore::default());
        let clip = stage(&store, &mp4_bytes(b"clip"), "video", "video/mp4").await;
        // A generous render timeout: only the deadline can cut this short.
        let tools = toolset_with_store(
            stub_dir.path().join("ffmpeg"),
            stub_dir.path().join("ffprobe"),
            scratch_dir.path().to_path_buf(),
            Duration::from_secs(30),
            store,
        );
        let spec = spec_for(super::COMPOSE_TOOL_NAME, &tools.tools());

        // Deadline 50 ms in the past: nothing may spawn at all.
        let mut ctx = tool_context();
        ctx.run.deadline = Some(deadline_in(-50));
        let call = validated_call(&spec, &compose_args(std::slice::from_ref(&clip)));
        let Err(error) = tools.call(ctx, call).await else {
            panic!("an elapsed deadline must fail the call")
        };
        assert_eq!(error.code(), VIDEO_COMPOSE_TIMEOUT);
        assert!(
            !stub_dir.path().join("probed").exists(),
            "no child runs on a spent budget"
        );

        // Deadline shortly ahead: the probe fits, the 30 s render does not.
        let mut ctx = tool_context();
        ctx.run.deadline = Some(deadline_in(400));
        let call = validated_call(&spec, &compose_args(&[clip]));
        let started = std::time::Instant::now();
        let Err(error) = tools.call(ctx, call).await else {
            panic!("the deadline must bound the render")
        };
        assert_eq!(error.code(), VIDEO_COMPOSE_TIMEOUT);
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the deadline, not render_timeout, bounded the call: {:?}",
            started.elapsed()
        );
        assert!(stub_dir.path().join("probed").exists());
    }

    #[tokio::test]
    async fn probe_tool_error_omits_stderr() {
        let stub_dir = tempdir().expect("stub dir");
        let scratch_dir = tempdir().expect("scratch dir");
        write_stub(
            stub_dir.path(),
            "ffprobe",
            r#"echo "$1: No such file or directory" >&2; exit 1"#,
        );
        write_stub(stub_dir.path(), "ffmpeg", FFMPEG_STUB_BODY);
        let store = Arc::new(InProcessArtifactStore::default());
        let clip = stage(&store, &mp4_bytes(b"clip"), "video", "video/mp4").await;
        let tools = toolset_with_store(
            stub_dir.path().join("ffmpeg"),
            stub_dir.path().join("ffprobe"),
            scratch_dir.path().to_path_buf(),
            Duration::from_secs(5),
            store,
        );
        let probe = spec_for(super::PROBE_TOOL_NAME, &tools.tools());
        let call = validated_call(&probe, &serde_json::json!({ "artifact": clip }));
        let Err(error) = tools.call(tool_context(), call).await else {
            panic!("probe must fail")
        };
        assert_eq!(error.code(), VIDEO_COMPOSE_FFMPEG_FAILED);
        assert_eq!(error.message(), "probe process failed");
        assert!(!error.message().contains("No such file"));
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
