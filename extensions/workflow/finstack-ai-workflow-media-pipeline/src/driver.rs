//! The `MoviePlan` render driver: submit once, then advance in bounded ticks.
//!
//! The driver composes the leaf media toolsets in-process. The kernel-journaled
//! effects are the wrapping `render_movie` / `advance_render` tool calls; the
//! per-stage durability of an individual render lives in the adapter-owned
//! [`RenderStateStore`] (the workflow-local cron adapter precedent). Every
//! [`MediaPipelineDriver::advance`] call performs at most one leaf tool call per
//! scene, so a tick is bounded and a crash resumes from the persisted stage.

use std::sync::Arc;

use finstack_ai_kernel::{
    ArtifactRef, Digest, EffectOutputContract, EffectOutputKind, ErrorCategory, Metadata, RawJson,
    Sensitivity, ToolCallBlock, ToolFailurePolicy, ValidatedToolCall,
};
use finstack_ai_runtime::Bytes;
use finstack_ai_runtime::artifact::{
    ArtifactMetadata, ArtifactScope, ArtifactStore, stage_required_artifact,
    validate_retrieved_artifact,
};
use finstack_ai_runtime::ports::tool::{ToolCallContext, ToolError, ToolStreamItem, Toolset};
use futures_util::StreamExt;
use serde_json::{Value, json};

use crate::plan::{
    CaptionsMode, FrameSource, MoviePlan, PlanLimits, SceneSpec, TransitionKindName, validate_plan,
};
use crate::state::{
    RenderState, RenderStateStore, RenderStatus, SceneStage, SceneState, StateError,
};
use crate::subtitles::{cue_timeline, to_srt, to_vtt};

/// Stable code: the driver was configured with limits it cannot honor.
pub const MEDIA_PIPELINE_CONFIG_INVALID: &str = "media_pipeline_config_invalid";
/// Stable code: the caller supplied arguments the driver cannot act on.
pub const MEDIA_PIPELINE_INVALID_ARGUMENTS: &str = "media_pipeline_invalid_arguments";
/// Stable code: the submitted movie plan is malformed or unsupported.
pub const MEDIA_PIPELINE_PLAN_INVALID: &str = "media_pipeline_plan_invalid";
/// Stable code: the plan exceeds a host-imposed resource ceiling.
pub const MEDIA_PIPELINE_BUDGET_EXCEEDED: &str = "media_pipeline_budget_exceeded";
/// Stable code: the render state store could not be read or written.
pub const MEDIA_PIPELINE_STORE_FAILURE: &str = "media_pipeline_store_failure";
/// Stable code: one pipeline stage (a leaf tool call) failed.
pub const MEDIA_PIPELINE_STAGE_FAILED: &str = "media_pipeline_stage_failed";
/// Stable code: no render exists for the requested `(tenant, render_id)`.
pub const MEDIA_PIPELINE_NOT_FOUND: &str = "media_pipeline_not_found";

/// Compose error code that means the composed inputs no longer verify.
const VIDEO_COMPOSE_MEDIA_FAILURE: &str = "video_compose_media_failure";

/// Seconds each poll call waits inside the provider before returning, so one
/// tick makes progress without blocking the whole batch on one scene.
const POLL_WAIT_SECONDS: u32 = 15;

const IMAGE_TOOL: &str = "openrouter_generate_image";
const VIDEO_SUBMIT_TOOL: &str = "openrouter_generate_video";
const VIDEO_STATUS_TOOL: &str = "openrouter_get_video";
const VIDEO_DOWNLOAD_TOOL: &str = "openrouter_download_video";
const COMPOSE_TOOL: &str = "compose_video";

/// The two `validate_plan` messages that are budget ceilings, not shape errors.
const BUDGET_MESSAGES: [&str; 2] = [
    "movie plan exceeds the scene ceiling",
    "movie plan exceeds the total video seconds ceiling",
];

/// Host wiring for one [`MediaPipelineDriver`].
pub struct MediaPipelineConfig {
    /// Media generation toolset (`finstack-ai-tools-openrouter-media`).
    pub media_tools: Arc<dyn Toolset>,
    /// Composition toolset (`finstack-ai-tools-video-compose`).
    pub compose_tools: Arc<dyn Toolset>,
    /// Adapter-owned progress store.
    pub state: Arc<dyn RenderStateStore>,
    /// Artifact store. Required by plans that carry caption cues.
    pub artifact_store: Option<Arc<dyn ArtifactStore>>,
    /// Host-imposed plan ceilings.
    pub limits: PlanLimits,
}

/// Drives one `MoviePlan` from submission to a composed final artifact.
pub struct MediaPipelineDriver {
    media_tools: Arc<dyn Toolset>,
    compose_tools: Arc<dyn Toolset>,
    state: Arc<dyn RenderStateStore>,
    artifact_store: Option<Arc<dyn ArtifactStore>>,
    limits: PlanLimits,
}

impl std::fmt::Debug for MediaPipelineDriver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MediaPipelineDriver")
            .field("limits", &self.limits)
            .field("artifact_store", &self.artifact_store.is_some())
            .finish_non_exhaustive()
    }
}

/// Driver construction failures.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PipelineError {
    /// The supplied configuration cannot be honored.
    #[error("{} : {reason}", MEDIA_PIPELINE_CONFIG_INVALID)]
    ConfigInvalid {
        /// Stable, non-secret reason.
        reason: &'static str,
    },
}

impl MediaPipelineDriver {
    /// Construct the driver after checking the host-supplied limits.
    ///
    /// # Errors
    ///
    /// Returns [`PipelineError::ConfigInvalid`] when any limit is zero: a
    /// driver that can never admit a scene, a second of video, or a
    /// concurrent job cannot make progress and must fail closed at wiring.
    pub fn try_new(config: MediaPipelineConfig) -> Result<Self, PipelineError> {
        if config.limits.max_scenes == 0 {
            return Err(PipelineError::ConfigInvalid {
                reason: "max_scenes must be greater than zero",
            });
        }
        if config.limits.max_total_video_s == 0 {
            return Err(PipelineError::ConfigInvalid {
                reason: "max_total_video_s must be greater than zero",
            });
        }
        if config.limits.max_concurrent_jobs == 0 {
            return Err(PipelineError::ConfigInvalid {
                reason: "max_concurrent_jobs must be greater than zero",
            });
        }
        Ok(Self {
            media_tools: config.media_tools,
            compose_tools: config.compose_tools,
            state: config.state,
            artifact_store: config.artifact_store,
            limits: config.limits,
        })
    }

    /// Read one render's persisted progress.
    ///
    /// # Errors
    ///
    /// Returns [`MEDIA_PIPELINE_STORE_FAILURE`] when the store cannot be read
    /// and [`MEDIA_PIPELINE_NOT_FOUND`] when no such render exists.
    pub fn status(&self, tenant_scope: &str, render_id: &str) -> Result<RenderState, ToolError> {
        self.state
            .load(tenant_scope, render_id)
            .map_err(|_| store_failure())?
            .ok_or_else(|| {
                tool_error(
                    MEDIA_PIPELINE_NOT_FOUND,
                    ErrorCategory::Validation,
                    "no render exists for this tenant and render id",
                )
            })
    }

    /// Validate and register a plan, returning the render's initial state.
    ///
    /// Resubmitting a byte-identical plan is idempotent: the render id is
    /// derived from the plan digest, so a duplicate insert resumes the
    /// persisted render instead of starting a second one.
    ///
    /// # Errors
    ///
    /// Returns [`MEDIA_PIPELINE_PLAN_INVALID`] for a malformed plan (including
    /// a caption plan with no artifact store configured),
    /// [`MEDIA_PIPELINE_BUDGET_EXCEEDED`] when the plan breaches a ceiling, and
    /// [`MEDIA_PIPELINE_STORE_FAILURE`] when the state store cannot be written.
    #[allow(
        clippy::unused_async,
        reason = "submit_plan is async by contract so a store-backed plan \
                  validator can be awaited here without a breaking change"
    )]
    pub async fn submit_plan(
        &self,
        ctx: &ToolCallContext,
        plan_json: &[u8],
    ) -> Result<RenderState, ToolError> {
        let plan: MoviePlan = serde_json::from_slice(plan_json).map_err(|_| {
            tool_error(
                MEDIA_PIPELINE_PLAN_INVALID,
                ErrorCategory::Validation,
                "movie plan does not match the plan schema",
            )
        })?;
        if plan_requires_store(&plan) && self.artifact_store.is_none() {
            return Err(tool_error(
                MEDIA_PIPELINE_PLAN_INVALID,
                ErrorCategory::Validation,
                "captions require an artifact store",
            ));
        }
        validate_plan(&plan, &self.limits).map_err(|reason| {
            if BUDGET_MESSAGES.contains(&reason) {
                tool_error(MEDIA_PIPELINE_BUDGET_EXCEEDED, ErrorCategory::Limit, reason)
            } else {
                tool_error(
                    MEDIA_PIPELINE_PLAN_INVALID,
                    ErrorCategory::Validation,
                    reason,
                )
            }
        })?;

        let plan_text = std::str::from_utf8(plan_json)
            .map_err(|_| {
                tool_error(
                    MEDIA_PIPELINE_PLAN_INVALID,
                    ErrorCategory::Validation,
                    "movie plan bytes are not valid UTF-8",
                )
            })?
            .to_owned();
        let plan_digest = Digest::raw_json(plan_json);
        let hex = plan_digest.to_hex();
        let short = hex.get(..24).ok_or_else(|| {
            tool_error(
                MEDIA_PIPELINE_PLAN_INVALID,
                ErrorCategory::Validation,
                "plan digest is too short to derive a render identity",
            )
        })?;
        let render_id: Arc<str> = Arc::from(format!("render-{short}"));
        let tenant_scope = Arc::clone(&ctx.run.locator.tenant_scope);

        let state = RenderState {
            tenant_scope: Arc::clone(&tenant_scope),
            render_id: Arc::clone(&render_id),
            plan_json: plan_text,
            plan_digest,
            status: RenderStatus::Running,
            scenes: plan.scenes.iter().map(initial_scene_state).collect(),
            final_artifact: None,
            transcript_srt_artifact: None,
            transcript_vtt_artifact: None,
            revision: 0,
        };
        match self.state.insert(&state) {
            Ok(()) => Ok(state),
            // A true duplicate is a resubmission of the same plan, which the
            // digest-derived identity makes a resume rather than a failure.
            Err(StateError::Integrity {
                code: "render_exists",
            }) => self.status(tenant_scope.as_ref(), render_id.as_ref()),
            Err(_) => Err(store_failure()),
        }
    }

    /// Run one bounded tick over a render's scenes and persist the result.
    ///
    /// At most one leaf tool call is issued per scene, and the number of
    /// scenes held in [`SceneStage::Polling`] never exceeds
    /// `limits.max_concurrent_jobs`.
    ///
    /// # Errors
    ///
    /// Returns [`MEDIA_PIPELINE_NOT_FOUND`] for an unknown render,
    /// [`MEDIA_PIPELINE_PLAN_INVALID`] when the persisted plan no longer
    /// parses, [`MEDIA_PIPELINE_STORE_FAILURE`] on a store failure, and
    /// [`MEDIA_PIPELINE_STAGE_FAILED`] when transcript staging fails.
    pub async fn advance(
        &self,
        ctx: &ToolCallContext,
        render_id: &str,
    ) -> Result<RenderState, ToolError> {
        let mut state = self.status(ctx.run.locator.tenant_scope.as_ref(), render_id)?;
        if matches!(state.status, RenderStatus::Completed | RenderStatus::Failed) {
            return Ok(state);
        }
        let plan: MoviePlan = serde_json::from_str(&state.plan_json).map_err(|_| {
            tool_error(
                MEDIA_PIPELINE_PLAN_INVALID,
                ErrorCategory::Validation,
                "persisted movie plan no longer matches the plan schema",
            )
        })?;

        let mut in_flight = state
            .scenes
            .iter()
            .filter(|scene| scene.stage == SceneStage::Polling)
            .count();
        let mut cancelled = false;
        for index in 0..state.scenes.len() {
            if ctx.run.cancellation.is_cancelled() {
                cancelled = true;
                break;
            }
            let Some(spec) = plan.scenes.get(index) else {
                continue;
            };
            let before = state
                .scenes
                .get(index)
                .map_or(SceneStage::Failed, |scene| scene.stage);
            if before == SceneStage::PendingSubmit && in_flight >= self.limits.max_concurrent_jobs {
                continue;
            }
            let outcome = self.step_scene(ctx, &plan, spec, index, &mut state).await;
            let Some(scene) = state.scenes.get_mut(index) else {
                continue;
            };
            if let Err(error) = outcome {
                scene.stage = SceneStage::Failed;
                scene.failure = Some(format!("{}: {}", error.code(), error.message()));
            }
            let after = scene.stage;
            if before != SceneStage::Polling && after == SceneStage::Polling {
                in_flight = in_flight.saturating_add(1);
            } else if before == SceneStage::Polling && after != SceneStage::Polling {
                in_flight = in_flight.saturating_sub(1);
            }
        }

        if !cancelled {
            self.settle(ctx, &plan, &mut state).await?;
        }
        self.persist(&mut state)
    }

    /// Perform at most one leaf tool call for one scene.
    #[allow(
        clippy::too_many_lines,
        reason = "the per-stage match is the pipeline's state machine; splitting \
                  each arm into its own method would scatter one readable table"
    )]
    async fn step_scene(
        &self,
        ctx: &ToolCallContext,
        plan: &MoviePlan,
        spec: &SceneSpec,
        index: usize,
        state: &mut RenderState,
    ) -> Result<(), ToolError> {
        let Some(current) = state.scenes.get(index).map(|scene| scene.stage) else {
            return Ok(());
        };
        match current {
            SceneStage::PendingStartFrame | SceneStage::PendingEndFrame => {
                let start = current == SceneStage::PendingStartFrame;
                let source = if start {
                    Some(&spec.start_frame)
                } else {
                    spec.end_frame.as_ref()
                };
                let Some(FrameSource::Prompt { prompt }) = source else {
                    // Nothing to generate; fall through to the next stage.
                    advance_frame_stage(spec, index, state, start);
                    return Ok(());
                };
                let result = invoke_tool(
                    &self.media_tools,
                    IMAGE_TOOL,
                    image_arguments(plan, spec, prompt),
                    ctx,
                )
                .await?;
                let artifact = parse_artifact(&result).ok_or_else(|| {
                    stage_error(
                        "generated frame has no artifact; the media toolset needs an artifact store",
                    )
                })?;
                if let Some(scene) = state.scenes.get_mut(index) {
                    if start {
                        scene.start_frame_artifact = Some(artifact);
                    } else {
                        scene.end_frame_artifact = Some(artifact);
                    }
                }
                advance_frame_stage(spec, index, state, start);
                Ok(())
            }
            SceneStage::PendingSubmit => {
                let arguments = {
                    let Some(scene) = state.scenes.get(index) else {
                        return Ok(());
                    };
                    submit_arguments(plan, spec, scene)
                };
                let result =
                    invoke_tool(&self.media_tools, VIDEO_SUBMIT_TOOL, arguments, ctx).await?;
                let job_id = result
                    .get("id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| stage_error("video submission returned no job id"))?
                    .to_owned();
                if let Some(scene) = state.scenes.get_mut(index) {
                    scene.job_id = Some(job_id);
                    scene.stage = SceneStage::Polling;
                }
                Ok(())
            }
            SceneStage::Polling => {
                let Some(job_id) = state
                    .scenes
                    .get(index)
                    .and_then(|scene| scene.job_id.clone())
                else {
                    return Err(stage_error("polling scene has no job id"));
                };
                let result = invoke_tool(
                    &self.media_tools,
                    VIDEO_STATUS_TOOL,
                    json!({ "id": job_id, "wait_seconds": POLL_WAIT_SECONDS }),
                    ctx,
                )
                .await?;
                let status = result
                    .get("status")
                    .and_then(Value::as_str)
                    .ok_or_else(|| stage_error("video status result carried no status"))?
                    .to_owned();
                let Some(scene) = state.scenes.get_mut(index) else {
                    return Ok(());
                };
                match status.as_str() {
                    "completed" => scene.stage = SceneStage::PendingDownload,
                    "failed" => {
                        if scene.resubmitted {
                            scene.stage = SceneStage::Failed;
                            scene.failure = Some("video job failed".to_owned());
                        } else {
                            scene.resubmitted = true;
                            scene.job_id = None;
                            scene.stage = SceneStage::PendingSubmit;
                        }
                    }
                    // `pending` / `in_progress` simply need another tick.
                    _ => {}
                }
                Ok(())
            }
            SceneStage::PendingDownload => {
                let Some(job_id) = state
                    .scenes
                    .get(index)
                    .and_then(|scene| scene.job_id.clone())
                else {
                    return Err(stage_error("download stage has no job id"));
                };
                let result = invoke_tool(
                    &self.media_tools,
                    VIDEO_DOWNLOAD_TOOL,
                    json!({ "id": job_id }),
                    ctx,
                )
                .await?;
                let artifact = parse_artifact(&result)
                    .ok_or_else(|| stage_error("video download returned no artifact"))?;
                if let Some(scene) = state.scenes.get_mut(index) {
                    scene.clip_artifact = Some(artifact);
                    scene.stage = SceneStage::Done;
                }
                Ok(())
            }
            SceneStage::Done | SceneStage::Failed => Ok(()),
        }
    }

    /// Decide the render-level status after one pass, composing when ready.
    async fn settle(
        &self,
        ctx: &ToolCallContext,
        plan: &MoviePlan,
        state: &mut RenderState,
    ) -> Result<(), ToolError> {
        let any_failed = state
            .scenes
            .iter()
            .any(|scene| scene.stage == SceneStage::Failed);
        let in_flight = state
            .scenes
            .iter()
            .any(|scene| !matches!(scene.stage, SceneStage::Done | SceneStage::Failed));
        if any_failed {
            if !in_flight {
                state.status = RenderStatus::Failed;
            }
            return Ok(());
        }
        if in_flight {
            state.status = RenderStatus::Running;
            return Ok(());
        }

        state.status = RenderStatus::Composing;
        // Transcripts are staged and persisted BEFORE composing so a crash
        // between the two resumes without regenerating them.
        let mut transcripts_staged_now = false;
        if plan_has_cues(plan) && state.transcript_srt_artifact.is_none() {
            let cues = cue_timeline(plan);
            let Some(store) = self.artifact_store.as_ref() else {
                return Err(stage_error("captions require an artifact store"));
            };
            let scope = artifact_scope(ctx);
            let srt = stage_text(
                store.as_ref(),
                &scope,
                to_srt(&cues),
                "application/x-subrip",
                "transcript.srt",
            )
            .await?;
            let vtt = stage_text(
                store.as_ref(),
                &scope,
                to_vtt(&cues),
                "text/vtt",
                "transcript.vtt",
            )
            .await?;
            state.transcript_srt_artifact = Some(srt);
            state.transcript_vtt_artifact = Some(vtt);
            let won = self.state.update(state).map_err(|_| store_failure())?;
            if !won {
                // Lost the race; the caller reloads the newer row.
                return Ok(());
            }
            state.revision = state.revision.saturating_add(1);
            transcripts_staged_now = true;
        }

        let spec = compose_spec(plan, state)?;
        match invoke_tool(&self.compose_tools, COMPOSE_TOOL, spec, ctx).await {
            Ok(result) => {
                let artifact = parse_artifact(&result)
                    .ok_or_else(|| stage_error("composition returned no artifact"))?;
                state.final_artifact = Some(artifact);
                state.status = RenderStatus::Completed;
                Ok(())
            }
            Err(error) => {
                if error.code() == VIDEO_COMPOSE_MEDIA_FAILURE
                    || error.message().contains(VIDEO_COMPOSE_MEDIA_FAILURE)
                {
                    self.rerun_or_fail(ctx, state, transcripts_staged_now).await;
                } else {
                    state.status = RenderStatus::Failed;
                }
                Ok(())
            }
        }
    }

    /// Decide the at-least-once re-run after a compose media failure.
    ///
    /// Composition reads two kinds of input: the per-scene clips and the
    /// transcript pair. Demoting the clips that no longer verify is the normal
    /// repair. When *nothing* was demoted the clips are all fine, so the
    /// suspect input is the transcript pair — which a crash-resume can carry
    /// over from a previous run's scope. Clearing it makes the next tick
    /// re-stage it under this run's scope and compose again.
    ///
    /// That second chance is only real if the transcripts predate this tick.
    /// Transcripts staged moments ago in this same tick would be re-staged
    /// byte-identically under the same scope, so clearing them would buy a
    /// no-progress loop rather than progress: with nothing left to redo, the
    /// render fails instead.
    async fn rerun_or_fail(
        &self,
        ctx: &ToolCallContext,
        state: &mut RenderState,
        transcripts_staged_now: bool,
    ) {
        if self.demote_unverified_clips(ctx, state).await > 0 {
            state.status = RenderStatus::Running;
            return;
        }
        let carried_over_transcripts = !transcripts_staged_now
            && (state.transcript_srt_artifact.is_some() || state.transcript_vtt_artifact.is_some());
        if carried_over_transcripts {
            state.transcript_srt_artifact = None;
            state.transcript_vtt_artifact = None;
            state.status = RenderStatus::Running;
        } else {
            state.status = RenderStatus::Failed;
        }
    }

    /// Demote every clip that no longer verifies to the stage that can produce
    /// it again, returning how many scenes were demoted.
    async fn demote_unverified_clips(
        &self,
        ctx: &ToolCallContext,
        state: &mut RenderState,
    ) -> usize {
        let scope = artifact_scope(ctx);
        let mut demoted = 0_usize;
        for index in 0..state.scenes.len() {
            let Some(artifact) = state
                .scenes
                .get(index)
                .and_then(|scene| scene.clip_artifact.clone())
            else {
                continue;
            };
            if self.clip_verifies(&scope, &artifact).await {
                continue;
            }
            if let Some(scene) = state.scenes.get_mut(index) {
                scene.clip_artifact = None;
                scene.stage = if scene.job_id.is_some() {
                    SceneStage::PendingDownload
                } else {
                    SceneStage::PendingSubmit
                };
                demoted = demoted.saturating_add(1);
            }
        }
        demoted
    }

    async fn clip_verifies(&self, scope: &ArtifactScope, artifact: &ArtifactRef) -> bool {
        // No store means nothing can be verified, so every clip is re-run.
        let Some(store) = self.artifact_store.as_ref() else {
            return false;
        };
        match store.get(scope.clone(), artifact.clone()).await {
            Ok(bytes) => validate_retrieved_artifact(scope, artifact, &bytes).is_ok(),
            Err(_) => false,
        }
    }

    /// Persist the tick. A lost CAS returns the newer persisted row.
    fn persist(&self, state: &mut RenderState) -> Result<RenderState, ToolError> {
        let won = self.state.update(state).map_err(|_| store_failure())?;
        if won {
            state.revision = state.revision.saturating_add(1);
            return Ok(state.clone());
        }
        self.status(state.tenant_scope.as_ref(), state.render_id.as_ref())
    }
}

/// Move a scene past a frame stage once that frame is resolved.
fn advance_frame_stage(spec: &SceneSpec, index: usize, state: &mut RenderState, was_start: bool) {
    let Some(scene) = state.scenes.get_mut(index) else {
        return;
    };
    let end_is_prompt = matches!(spec.end_frame, Some(FrameSource::Prompt { .. }));
    scene.stage = if was_start && end_is_prompt && scene.end_frame_artifact.is_none() {
        SceneStage::PendingEndFrame
    } else {
        SceneStage::PendingSubmit
    };
}

/// Whether this plan carries any caption cue to flatten onto the timeline.
fn plan_has_cues(plan: &MoviePlan) -> bool {
    plan.scenes
        .iter()
        .any(|scene| scene.captions.as_ref().is_some_and(|cues| !cues.is_empty()))
}

/// Whether this plan can only be rendered with an artifact store attached.
fn plan_requires_store(plan: &MoviePlan) -> bool {
    plan_has_cues(plan)
        || matches!(
            plan.output.captions,
            Some(CaptionsMode::Sidecar | CaptionsMode::BurnIn)
        )
}

fn initial_scene_state(scene: &SceneSpec) -> SceneState {
    let mut state = SceneState {
        scene_id: scene.id.clone(),
        stage: SceneStage::PendingSubmit,
        start_frame_artifact: None,
        start_frame_url: None,
        end_frame_artifact: None,
        end_frame_url: None,
        job_id: None,
        clip_artifact: None,
        failure: None,
        resubmitted: false,
    };
    match &scene.start_frame {
        FrameSource::Prompt { .. } => {}
        FrameSource::Artifact { artifact } => {
            state.start_frame_artifact = Some(artifact.clone());
        }
        FrameSource::Url { url } => state.start_frame_url = Some(url.clone()),
    }
    match &scene.end_frame {
        Some(FrameSource::Artifact { artifact }) => {
            state.end_frame_artifact = Some(artifact.clone());
        }
        Some(FrameSource::Url { url }) => state.end_frame_url = Some(url.clone()),
        Some(FrameSource::Prompt { .. }) | None => {}
    }
    state.stage = if matches!(scene.start_frame, FrameSource::Prompt { .. }) {
        SceneStage::PendingStartFrame
    } else if matches!(scene.end_frame, Some(FrameSource::Prompt { .. })) {
        SceneStage::PendingEndFrame
    } else {
        SceneStage::PendingSubmit
    };
    state
}

/// Arguments for `openrouter_generate_image` (required-with-null schema).
///
/// `resolution` and `output_format` stay null: the plan's `resolution` label
/// (`"720p"`) is a *video* token, and image models each define their own
/// resolution vocabulary, so forwarding it would be rejected by the provider.
fn image_arguments(plan: &MoviePlan, spec: &SceneSpec, prompt: &str) -> Value {
    let model = spec
        .overrides
        .as_ref()
        .and_then(|overrides| overrides.image_model.clone())
        .unwrap_or_else(|| plan.defaults.image_model.clone());
    json!({
        "model": model,
        "prompt": prompt,
        "resolution": Value::Null,
        "aspect_ratio": plan.defaults.aspect_ratio.clone(),
        "output_format": Value::Null,
    })
}

/// Arguments for `openrouter_generate_video` (required-with-null schema: every
/// key must be present, with `null` for anything unset).
fn submit_arguments(plan: &MoviePlan, spec: &SceneSpec, scene: &SceneState) -> Value {
    let overrides = spec.overrides.as_ref();
    let model = overrides
        .and_then(|value| value.video_model.clone())
        .unwrap_or_else(|| plan.defaults.video_model.clone());
    let resolution = overrides
        .and_then(|value| value.resolution.clone())
        .or_else(|| plan.defaults.resolution.clone());
    let duration = spec.duration_s.unwrap_or(plan.defaults.scene_duration_s);
    // `validate_plan` rejects prompt-sourced reference images, so every entry
    // here resolves; `filter_map` is the total-function spelling of that.
    let references: Vec<Value> = spec
        .reference_images
        .iter()
        .flatten()
        .filter_map(frame_source_json)
        .collect();
    json!({
        "model": model,
        "prompt": spec.video_prompt.clone(),
        "duration": duration,
        "resolution": resolution,
        "aspect_ratio": plan.defaults.aspect_ratio.clone(),
        "size": Value::Null,
        "seed": spec.seed,
        "generate_audio": Value::Null,
        "first_frame": frame_json(
            scene.start_frame_artifact.as_ref(),
            scene.start_frame_url.as_deref(),
        ),
        "last_frame": frame_json(
            scene.end_frame_artifact.as_ref(),
            scene.end_frame_url.as_deref(),
        ),
        "reference_images": if references.is_empty() {
            Value::Null
        } else {
            Value::Array(references)
        },
    })
}

/// One `{"url": …, "artifact": …}` frame object, or null when unresolved.
fn frame_json(artifact: Option<&ArtifactRef>, url: Option<&str>) -> Value {
    match (artifact, url) {
        (Some(artifact), _) => json!({ "url": Value::Null, "artifact": artifact }),
        (None, Some(url)) => json!({ "url": url, "artifact": Value::Null }),
        (None, None) => Value::Null,
    }
}

fn frame_source_json(source: &FrameSource) -> Option<Value> {
    match source {
        FrameSource::Prompt { .. } => None,
        FrameSource::Artifact { artifact } => Some(frame_json(Some(artifact), None)),
        FrameSource::Url { url } => Some(frame_json(None, Some(url))),
    }
}

/// Build the compose toolset's `CompositionSpec` for a fully rendered plan.
///
/// The compose crate uses the plain-optional convention: absent keys are
/// omitted rather than sent as null.
fn compose_spec(plan: &MoviePlan, state: &RenderState) -> Result<Value, ToolError> {
    let mut clips = Vec::with_capacity(state.scenes.len());
    for scene in &state.scenes {
        let artifact = scene
            .clip_artifact
            .as_ref()
            .ok_or_else(|| stage_error("a finished scene is missing its clip artifact"))?;
        clips.push(json!({ "artifact": artifact }));
    }
    let mut spec = serde_json::Map::new();
    spec.insert("version".into(), json!(1));
    spec.insert("clips".into(), Value::Array(clips));

    // Plan transitions are named by the scene they follow; the compose spec
    // wants one entry per boundary, so index k is the transition after scene k
    // and every unnamed boundary is a hard cut.
    if state.scenes.len() > 1 {
        let boundaries = state.scenes.len().saturating_sub(1);
        let mut transitions = Vec::with_capacity(boundaries);
        for index in 0..boundaries {
            let named = plan
                .scenes
                .get(index)
                .and_then(|scene| {
                    plan.transitions
                        .iter()
                        .flatten()
                        .find(|transition| transition.after == scene.id)
                })
                .map_or_else(
                    || json!({ "type": "cut" }),
                    |transition| {
                        let kind = match transition.kind {
                            TransitionKindName::Cut => "cut",
                            TransitionKindName::Crossfade => "crossfade",
                            TransitionKindName::FadeToBlack => "fade_to_black",
                        };
                        match transition.duration_s {
                            Some(duration) if transition.kind != TransitionKindName::Cut => {
                                json!({ "type": kind, "duration_s": duration })
                            }
                            _ => json!({ "type": kind }),
                        }
                    },
                );
            transitions.push(named);
        }
        spec.insert("transitions".into(), Value::Array(transitions));
    }

    if let Some(audio) = &plan.audio {
        spec.insert(
            "audio".into(),
            json!({ "artifact": audio.artifact, "mode": audio.mode }),
        );
    }

    if let Some(srt) = &state.transcript_srt_artifact {
        let mode = match plan.output.captions {
            Some(CaptionsMode::BurnIn) => Some("burn_in"),
            // A sidecar in an mp4 is muxed as a selectable track; webm has no
            // portable equivalent, so the transcript refs are the deliverable.
            Some(CaptionsMode::Sidecar) if plan.output.container == "mp4" => Some("mux"),
            _ => None,
        };
        if let Some(mode) = mode {
            spec.insert("subtitles".into(), json!({ "artifact": srt, "mode": mode }));
        }
    }

    let mut output = serde_json::Map::new();
    output.insert("container".into(), json!(plan.output.container));
    if let Some(fps) = plan.output.fps {
        output.insert("fps".into(), json!(fps));
    }
    spec.insert("output".into(), Value::Object(output));
    Ok(Value::Object(spec))
}

async fn stage_text(
    store: &dyn ArtifactStore,
    scope: &ArtifactScope,
    text: String,
    media_type: &str,
    name: &str,
) -> Result<ArtifactRef, ToolError> {
    stage_required_artifact(
        store,
        scope.clone(),
        Bytes::from(text.into_bytes()),
        ArtifactMetadata {
            kind: Arc::from("tool-output"),
            media_type: Arc::from(media_type),
            name: Some(Arc::from(name)),
            attributes: Metadata::empty(),
        },
    )
    .await
    .map_err(|_| stage_error("transcript artifact could not be staged"))
}

/// The Global Constraints artifact scope shared by artifact-bearing tools.
fn artifact_scope(ctx: &ToolCallContext) -> ArtifactScope {
    ArtifactScope {
        tenant_scope: Arc::clone(&ctx.run.locator.tenant_scope),
        session_id: ctx.run.locator.session_id,
        run_id: Some(ctx.run.locator.run_id),
        sensitivity: Sensitivity::Internal,
    }
}

fn parse_artifact(result: &Value) -> Option<ArtifactRef> {
    serde_json::from_value(result.get("artifact")?.clone()).ok()
}

/// Invoke one leaf tool on `toolset` and return its parsed JSON result.
async fn invoke_tool(
    toolset: &Arc<dyn Toolset>,
    tool_name: &str,
    arguments: Value,
    ctx: &ToolCallContext,
) -> Result<Value, ToolError> {
    let specs = toolset.tools();
    let spec = specs
        .iter()
        .find(|spec| spec.model_name.as_ref() == tool_name)
        .ok_or_else(|| stage_error("the configured toolset does not offer this pipeline tool"))?;
    let encoded = serde_json::to_vec(&arguments)
        .map_err(|_| stage_error("pipeline tool arguments could not be encoded"))?;
    let raw = RawJson::parse(&encoded)
        .map_err(|_| stage_error("pipeline tool arguments are not canonical JSON"))?;
    let call = ValidatedToolCall {
        call: ToolCallBlock::try_new(ctx.tool_call_id, tool_name, raw)
            .map_err(|_| stage_error("pipeline tool call could not be built"))?,
        tool_id: spec.id.clone(),
        component: None,
        output_contract: EffectOutputContract {
            kind: EffectOutputKind::ToolResult,
            schema_version: 1,
            schema_digest: Digest::raw_json(b"{}"),
        },
        retry_safety: spec.retry_safety,
        deadline: ctx.run.deadline,
        execution: spec.execution,
        failure_policy: ToolFailurePolicy::ReturnToModel,
    };
    let mut stream = toolset.call(ctx.clone(), call).await?;
    let mut completed = None;
    while let Some(item) = stream.next().await {
        if let ToolStreamItem::Completed(result) = item? {
            completed = Some(result);
            break;
        }
    }
    let result =
        completed.ok_or_else(|| stage_error("pipeline tool stream ended with no result"))?;
    let value: Value = serde_json::from_slice(result.output.as_bytes())
        .map_err(|_| stage_error("pipeline tool result is not JSON"))?;
    if result.is_error {
        let code = value
            .get("code")
            .and_then(Value::as_str)
            .unwrap_or("unspecified");
        return Err(dyn_tool_error(
            MEDIA_PIPELINE_STAGE_FAILED,
            ErrorCategory::Tool,
            &format!("pipeline tool returned an error result: {code}"),
        ));
    }
    Ok(value)
}

fn tool_error(code: &'static str, category: ErrorCategory, message: &'static str) -> ToolError {
    ToolError::try_new(code, category, false, message, Metadata::empty()).unwrap_or_else(Into::into)
}

/// [`tool_error`] for a message assembled at runtime.
fn dyn_tool_error(code: &'static str, category: ErrorCategory, message: &str) -> ToolError {
    ToolError::try_new(code, category, false, message, Metadata::empty()).unwrap_or_else(Into::into)
}

fn stage_error(message: &'static str) -> ToolError {
    tool_error(MEDIA_PIPELINE_STAGE_FAILED, ErrorCategory::Tool, message)
}

fn store_failure() -> ToolError {
    tool_error(
        MEDIA_PIPELINE_STORE_FAILURE,
        ErrorCategory::Store,
        "the render state store could not be read or written",
    )
}

/// Shared test infrastructure reused by both `driver`'s own tests and
/// `tools`'s tests: a queued-result `Toolset` double, fixture plans, and the
/// harness that wires a [`MediaPipelineDriver`] to two of those doubles.
#[cfg(test)]
pub(crate) mod test_support {
    use std::collections::{BTreeMap, VecDeque};
    use std::sync::{Arc, Mutex};

    use finstack_ai_kernel::{
        Digest, EffectId, LaneId, Metadata, OperationLocator, PrincipalRef, RawJson, RetrySafety,
        RunId, SessionId, ToolBatchId, ToolCallId, ToolExecutionMode, ToolId, ValidatedToolCall,
    };
    use finstack_ai_runtime::artifact::{ArtifactMetadata, InProcessArtifactStore};
    use finstack_ai_runtime::ports::PortFuture;
    use finstack_ai_runtime::ports::model::{
        ApprovalMetadata, ApprovalRequirement, AuthorizationContext, CancellationSignal,
        RunCallContext, SideEffectClass, ToolDeferralSupport, ToolSpec,
    };
    use finstack_ai_runtime::ports::tool::{
        ToolCallContext, ToolEventStream, ToolResult, ToolStreamItem, Toolset, ToolsetDescriptor,
    };
    use futures_util::stream;
    use serde_json::{Value, json};

    use super::{
        ArtifactRef, ArtifactStore, Bytes, ErrorCategory, MediaPipelineConfig, MediaPipelineDriver,
        PlanLimits, RenderStatus, SceneStage, SceneState, ToolError, artifact_scope,
        stage_required_artifact,
    };
    use crate::state::{MemoryRenderStateStore, RenderState, RenderStateStore};

    // ----- test double ---------------------------------------------------

    /// A `Toolset` whose `call` pops the next queued JSON result for the tool
    /// being invoked, recording every `(tool_name, arguments)` pair.
    ///
    /// `ScriptedToolset` scripts one plan per *call* in arrival order; these
    /// tests need results keyed by tool name (the driver interleaves scenes),
    /// plus the recorded arguments to assert the compose spec, so a local
    /// double is the simpler fit.
    #[derive(Debug)]
    pub(crate) struct QueueToolset {
        tools: Arc<[ToolSpec]>,
        queues: Mutex<BTreeMap<String, VecDeque<Queued>>>,
        calls: Mutex<Vec<(String, Value)>>,
    }

    /// One queued outcome for a single call.
    #[derive(Debug, Clone)]
    enum Queued {
        /// A normal successful result payload.
        Ok(Value),
        /// A terminal result the tool itself flags as an error.
        ErrorResult(Value),
        /// An adapter error raised by `call` before any stream exists.
        CallError(ToolError),
    }

    impl QueueToolset {
        pub(crate) fn new(names: &[&str]) -> Arc<Self> {
            let tools: Vec<ToolSpec> = names.iter().map(|name| spec_for(name)).collect();
            Arc::new(Self {
                tools: Arc::from(tools),
                queues: Mutex::new(BTreeMap::new()),
                calls: Mutex::new(Vec::new()),
            })
        }

        pub(crate) fn push(&self, tool: &str, result: Value) {
            self.enqueue(tool, Queued::Ok(result));
        }

        /// Queue a terminal `ToolResult` carrying `is_error: true`.
        pub(crate) fn push_error_result(&self, tool: &str, payload: Value) {
            self.enqueue(tool, Queued::ErrorResult(payload));
        }

        /// Queue an adapter error raised straight out of `Toolset::call`.
        pub(crate) fn push_call_error(&self, tool: &str, code: &str, message: &str) {
            let error =
                ToolError::try_new(code, ErrorCategory::Tool, false, message, Metadata::empty())
                    .expect("tool error");
            self.enqueue(tool, Queued::CallError(error));
        }

        fn enqueue(&self, tool: &str, entry: Queued) {
            self.queues
                .lock()
                .unwrap()
                .entry(tool.to_owned())
                .or_default()
                .push_back(entry);
        }

        pub(crate) fn calls(&self) -> Vec<(String, Value)> {
            self.calls.lock().unwrap().clone()
        }

        pub(crate) fn queued(&self) -> usize {
            self.queues
                .lock()
                .unwrap()
                .values()
                .map(VecDeque::len)
                .sum()
        }
    }

    fn spec_for(name: &str) -> ToolSpec {
        ToolSpec {
            id: ToolId::parse(format!("finstack.tools.{name}")).expect("tool id"),
            model_name: Arc::from(name),
            title: Arc::from(name),
            description: Arc::from("queued test double"),
            input_schema: RawJson::parse(br#"{"type":"object"}"#).expect("schema"),
            output_schema: None,
            execution: ToolExecutionMode::Sequential,
            side_effect: SideEffectClass::NonIdempotentWrite,
            retry_safety: RetrySafety::AtMostOnce,
            approval: ApprovalMetadata {
                requirement: ApprovalRequirement::NotRequired,
                reason: None,
                attributes: Metadata::empty(),
            },
            max_result_bytes: 1_048_576,
            metadata: Metadata::empty(),
            deferral: ToolDeferralSupport::Never,
        }
    }

    impl Toolset for QueueToolset {
        fn descriptor(&self) -> ToolsetDescriptor {
            ToolsetDescriptor {
                name: Arc::from("queue-toolset"),
                metadata: Metadata::empty(),
            }
        }

        fn tools(&self) -> Arc<[ToolSpec]> {
            Arc::clone(&self.tools)
        }

        fn call(
            &self,
            _ctx: ToolCallContext,
            call: ValidatedToolCall,
        ) -> PortFuture<Result<ToolEventStream, ToolError>> {
            let name = call.call.tool_name().to_owned();
            let arguments: Value =
                serde_json::from_slice(call.call.arguments().as_bytes()).expect("arguments json");
            self.calls.lock().unwrap().push((name.clone(), arguments));
            let entry = self
                .queues
                .lock()
                .unwrap()
                .get_mut(&name)
                .and_then(VecDeque::pop_front)
                .unwrap_or_else(|| panic!("no queued result for {name}"));
            let (payload, is_error) = match entry {
                Queued::Ok(payload) => (payload, false),
                Queued::ErrorResult(payload) => (payload, true),
                Queued::CallError(error) => {
                    return Box::pin(async move { Err(error) });
                }
            };
            let output = RawJson::parse(serde_json::to_vec(&payload).expect("encode"))
                .expect("canonical json");
            let item = ToolStreamItem::Completed(ToolResult { output, is_error });
            Box::pin(async move { Ok(Box::pin(stream::iter(vec![Ok(item)])) as ToolEventStream) })
        }
    }

    // ----- fixtures ------------------------------------------------------

    pub(crate) fn tool_context() -> ToolCallContext {
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

    pub(crate) fn limits() -> PlanLimits {
        PlanLimits {
            max_scenes: 10,
            max_total_video_s: 240,
            max_concurrent_jobs: 2,
        }
    }

    pub(crate) fn fixture_plan() -> Vec<u8> {
        std::fs::read(
            finstack_ai_test::repo_root()
                .join("fixtures/compatibility/movie-plan/valid-two-scene.json"),
        )
        .expect("fixture")
    }

    /// A plan whose scenes all start pinned to a URL, so every scene begins in
    /// `PendingSubmit` and nothing is generated before submission.
    pub(crate) fn url_only_plan(scene_count: usize) -> Vec<u8> {
        let scenes: Vec<Value> = (1..=scene_count)
            .map(|index| {
                json!({
                    "id": format!("scene-{index:02}"),
                    "video_prompt": format!("scene {index}"),
                    "start_frame": { "url": "https://example.test/frame.png" },
                })
            })
            .collect();
        serde_json::to_vec(&json!({
            "version": 1,
            "defaults": {
                "image_model": "test/image-model",
                "video_model": "test/video-model",
                "scene_duration_s": 6,
            },
            "scenes": scenes,
            "output": { "container": "mp4" },
        }))
        .expect("plan json")
    }

    pub(crate) async fn stage(
        store: &Arc<dyn ArtifactStore>,
        ctx: &ToolCallContext,
        body: &str,
    ) -> ArtifactRef {
        stage_required_artifact(
            store.as_ref(),
            artifact_scope(ctx),
            Bytes::from(body.as_bytes().to_vec()),
            ArtifactMetadata {
                kind: Arc::from("tool-output"),
                media_type: Arc::from("video/mp4"),
                name: Some(Arc::from("clip.mp4")),
                attributes: Metadata::empty(),
            },
        )
        .await
        .expect("stage")
    }

    pub(crate) async fn stage_subtitle(
        store: &Arc<dyn ArtifactStore>,
        ctx: &ToolCallContext,
        body: &str,
    ) -> ArtifactRef {
        stage_required_artifact(
            store.as_ref(),
            artifact_scope(ctx),
            Bytes::from(body.as_bytes().to_vec()),
            ArtifactMetadata {
                kind: Arc::from("tool-output"),
                media_type: Arc::from("application/x-subrip"),
                name: Some(Arc::from("transcript.srt")),
                attributes: Metadata::empty(),
            },
        )
        .await
        .expect("stage")
    }

    pub(crate) struct Harness {
        pub(crate) driver: Arc<MediaPipelineDriver>,
        pub(crate) media: Arc<QueueToolset>,
        pub(crate) compose: Arc<QueueToolset>,
        pub(crate) store: Option<Arc<dyn ArtifactStore>>,
        pub(crate) state: Arc<MemoryRenderStateStore>,
    }

    pub(crate) fn harness(limits: PlanLimits, with_store: bool) -> Harness {
        let media = QueueToolset::new(&[
            super::IMAGE_TOOL,
            super::VIDEO_SUBMIT_TOOL,
            super::VIDEO_STATUS_TOOL,
            super::VIDEO_DOWNLOAD_TOOL,
        ]);
        let compose = QueueToolset::new(&[super::COMPOSE_TOOL]);
        let store: Option<Arc<dyn ArtifactStore>> = with_store
            .then(|| Arc::new(InProcessArtifactStore::default()) as Arc<dyn ArtifactStore>);
        let state = Arc::new(MemoryRenderStateStore::new());
        let driver = MediaPipelineDriver::try_new(MediaPipelineConfig {
            media_tools: Arc::clone(&media) as Arc<dyn Toolset>,
            compose_tools: Arc::clone(&compose) as Arc<dyn Toolset>,
            state: Arc::clone(&state) as Arc<dyn RenderStateStore>,
            artifact_store: store.clone(),
            limits,
        })
        .expect("driver");
        Harness {
            driver: Arc::new(driver),
            media,
            compose,
            store,
            state,
        }
    }

    /// Queue the whole two-scene happy path: two frame images, two job
    /// submissions, one `pending` poll before the two `completed` polls, two
    /// downloads, and the final composition.
    pub(crate) fn prime_happy_path(
        harness: &Harness,
        image: &ArtifactRef,
        clips: [&ArtifactRef; 2],
        final_clip: &ArtifactRef,
    ) {
        for _ in 0..2 {
            harness.media.push(
                super::IMAGE_TOOL,
                json!({ "artifact": image, "media_type": "image/png", "byte_length": 11 }),
            );
        }
        for id in ["job-a", "job-b"] {
            harness.media.push(
                super::VIDEO_SUBMIT_TOOL,
                json!({ "id": id, "status": "queued" }),
            );
        }
        harness.media.push(
            super::VIDEO_STATUS_TOOL,
            json!({ "id": "job-a", "status": "pending" }),
        );
        for id in ["job-a", "job-b"] {
            harness.media.push(
                super::VIDEO_STATUS_TOOL,
                json!({ "id": id, "status": "completed", "urls": [] }),
            );
        }
        for clip in clips {
            harness.media.push(
                super::VIDEO_DOWNLOAD_TOOL,
                json!({ "artifact": clip, "media_type": "video/mp4", "byte_length": 14 }),
            );
        }
        harness.compose.push(
            super::COMPOSE_TOOL,
            json!({ "artifact": final_clip, "duration_s": 13.5, "byte_length": 17 }),
        );
    }

    /// Seed the state store with a render whose scenes are already `Done`, as
    /// a crashed-then-resumed process would find it. Returns the render id.
    pub(crate) fn seed_done_render(
        harness: &Harness,
        ctx: &ToolCallContext,
        plan_json: &[u8],
        clips: &[ArtifactRef],
        transcripts: Option<(ArtifactRef, ArtifactRef)>,
    ) -> String {
        let plan: crate::plan::MoviePlan = serde_json::from_slice(plan_json).expect("plan");
        let plan_digest = Digest::raw_json(plan_json);
        let render_id: Arc<str> = Arc::from(format!("render-{}", &plan_digest.to_hex()[..24]));
        let scenes = plan
            .scenes
            .iter()
            .zip(clips)
            .map(|(spec, clip)| SceneState {
                scene_id: spec.id.clone(),
                stage: SceneStage::Done,
                start_frame_artifact: None,
                start_frame_url: None,
                end_frame_artifact: None,
                end_frame_url: None,
                job_id: Some("job-done".to_owned()),
                clip_artifact: Some(clip.clone()),
                failure: None,
                resubmitted: false,
            })
            .collect();
        let (srt, vtt) = transcripts.unzip();
        let state = RenderState {
            tenant_scope: Arc::clone(&ctx.run.locator.tenant_scope),
            render_id: Arc::clone(&render_id),
            plan_json: String::from_utf8(plan_json.to_vec()).expect("utf8"),
            plan_digest,
            status: RenderStatus::Running,
            scenes,
            final_artifact: None,
            transcript_srt_artifact: srt,
            transcript_vtt_artifact: vtt,
            revision: 0,
        };
        harness.state.insert(&state).expect("seed");
        render_id.to_string()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::test_support::{
        fixture_plan, harness, limits, prime_happy_path, seed_done_render, stage, stage_subtitle,
        tool_context, url_only_plan,
    };
    use super::{PlanLimits, RenderStatus, SceneStage, artifact_scope};

    // ----- tests ---------------------------------------------------------

    #[tokio::test]
    async fn happy_path_two_scene_render_reaches_completed() {
        let ctx = tool_context();
        let harness = harness(limits(), true);
        let store = harness.store.clone().expect("store");
        let image = stage(&store, &ctx, "frame-bytes").await;
        let clip_one = stage(&store, &ctx, "clip-one-bytes").await;
        let clip_two = stage(&store, &ctx, "clip-two-bytes").await;
        let final_clip = stage(&store, &ctx, "final-movie-bytes").await;
        prime_happy_path(&harness, &image, [&clip_one, &clip_two], &final_clip);

        let plan = fixture_plan();
        let submitted = harness
            .driver
            .submit_plan(&ctx, &plan)
            .await
            .expect("submit");
        assert_eq!(submitted.status, RenderStatus::Running);
        assert_eq!(submitted.scenes[0].stage, SceneStage::PendingStartFrame);
        // A pinned URL frame needs no generation pass.
        assert_eq!(submitted.scenes[1].stage, SceneStage::PendingSubmit);
        assert_eq!(
            submitted.scenes[1].start_frame_url.as_deref(),
            Some("https://example.test/pier.png")
        );

        let render_id = submitted.render_id.to_string();
        let mut state = submitted;
        for _ in 0..12 {
            if state.status == RenderStatus::Completed {
                break;
            }
            state = harness
                .driver
                .advance(&ctx, &render_id)
                .await
                .expect("tick");
        }
        assert_eq!(state.status, RenderStatus::Completed, "{state:?}");
        assert!(
            state
                .scenes
                .iter()
                .all(|scene| scene.stage == SceneStage::Done)
        );
        assert_eq!(state.final_artifact.as_ref(), Some(&final_clip));

        // Transcripts are staged from the plan's cues before composing.
        let srt = state.transcript_srt_artifact.clone().expect("srt");
        let vtt = state.transcript_vtt_artifact.clone().expect("vtt");
        let bytes = store
            .get(artifact_scope(&ctx), srt)
            .await
            .expect("read srt");
        assert_eq!(
            std::str::from_utf8(&bytes).expect("utf8"),
            "1\n00:00:00,000 --> 00:00:03,500\nHarbors wake up slowly.\n\n2\n00:00:03,500 --> 00:00:07,500\nThen all at once.\n\n"
        );
        let vtt_bytes = store
            .get(artifact_scope(&ctx), vtt)
            .await
            .expect("read vtt");
        assert_eq!(
            std::str::from_utf8(&vtt_bytes).expect("utf8"),
            "WEBVTT\n\n00:00:00.000 --> 00:00:03.500\nHarbors wake up slowly.\n\n00:00:03.500 --> 00:00:07.500\nThen all at once.\n\n"
        );

        let compose_calls = harness.compose.calls();
        assert_eq!(compose_calls.len(), 1);
        let spec = &compose_calls[0].1;
        assert_eq!(spec["clips"].as_array().expect("clips").len(), 2);
        assert_eq!(
            spec["transitions"].as_array().expect("transitions").len(),
            1
        );
        assert_eq!(spec["transitions"][0]["type"], "crossfade");
        assert_eq!(spec["subtitles"]["mode"], "burn_in");
        assert_eq!(spec["output"]["container"], "mp4");
        assert_eq!(spec["version"], 1);
    }

    #[tokio::test]
    async fn concurrency_cap_holds_back_submissions() {
        let ctx = tool_context();
        let harness = harness(
            PlanLimits {
                max_concurrent_jobs: 1,
                ..limits()
            },
            true,
        );
        harness.media.push(
            super::VIDEO_SUBMIT_TOOL,
            json!({ "id": "job-a", "status": "queued" }),
        );
        harness.media.push(
            super::VIDEO_STATUS_TOOL,
            json!({ "id": "job-a", "status": "in_progress" }),
        );

        let plan = url_only_plan(3);
        let state = harness
            .driver
            .submit_plan(&ctx, &plan)
            .await
            .expect("submit");
        let render_id = state.render_id.to_string();
        harness
            .driver
            .advance(&ctx, &render_id)
            .await
            .expect("tick 1");
        let state = harness
            .driver
            .advance(&ctx, &render_id)
            .await
            .expect("tick 2");

        let with_jobs = state
            .scenes
            .iter()
            .filter(|scene| scene.job_id.is_some())
            .count();
        assert_eq!(with_jobs, 1, "the cap admits exactly one job: {state:?}");
        assert_eq!(state.scenes[1].stage, SceneStage::PendingSubmit);
        assert_eq!(state.scenes[2].stage, SceneStage::PendingSubmit);
    }

    #[tokio::test]
    async fn failed_job_is_resubmitted_once_then_fails_the_scene() {
        let ctx = tool_context();
        let harness = harness(limits(), true);
        for id in ["job-a", "job-b"] {
            harness.media.push(
                super::VIDEO_SUBMIT_TOOL,
                json!({ "id": id, "status": "queued" }),
            );
            harness.media.push(
                super::VIDEO_STATUS_TOOL,
                json!({ "id": id, "status": "failed" }),
            );
        }

        let plan = url_only_plan(1);
        let state = harness
            .driver
            .submit_plan(&ctx, &plan)
            .await
            .expect("submit");
        let render_id = state.render_id.to_string();

        let state = harness
            .driver
            .advance(&ctx, &render_id)
            .await
            .expect("submit tick");
        assert_eq!(state.scenes[0].job_id.as_deref(), Some("job-a"));
        let state = harness
            .driver
            .advance(&ctx, &render_id)
            .await
            .expect("first failure");
        assert_eq!(state.scenes[0].stage, SceneStage::PendingSubmit);
        assert!(state.scenes[0].resubmitted);
        assert!(state.scenes[0].job_id.is_none());
        let state = harness
            .driver
            .advance(&ctx, &render_id)
            .await
            .expect("resubmit");
        assert_eq!(state.scenes[0].job_id.as_deref(), Some("job-b"));
        let state = harness
            .driver
            .advance(&ctx, &render_id)
            .await
            .expect("second failure");
        assert_eq!(state.scenes[0].stage, SceneStage::Failed);
        assert_eq!(state.scenes[0].failure.as_deref(), Some("video job failed"));
        assert_eq!(state.status, RenderStatus::Failed);
        assert_eq!(harness.media.queued(), 0, "both attempts were consumed");
    }

    #[tokio::test]
    async fn budget_violation_rejects_before_any_tool_call() {
        let ctx = tool_context();
        let harness = harness(
            PlanLimits {
                max_scenes: 1,
                ..limits()
            },
            true,
        );
        let error = harness
            .driver
            .submit_plan(&ctx, &fixture_plan())
            .await
            .expect_err("two scenes exceed the ceiling");
        assert_eq!(error.code(), super::MEDIA_PIPELINE_BUDGET_EXCEEDED);
        assert!(harness.media.calls().is_empty());
        assert!(harness.compose.calls().is_empty());
    }

    #[tokio::test]
    async fn schema_enum_violation_rejects_before_any_tool_call() {
        let ctx = tool_context();
        let harness = harness(limits(), true);
        harness.media.push(
            super::VIDEO_SUBMIT_TOOL,
            json!({ "id": "job-a", "status": "queued" }),
        );

        let mut plan: serde_json::Value =
            serde_json::from_slice(&url_only_plan(1)).expect("plan parses as json");
        plan["output"]["container"] = json!("mkv");
        let bytes = serde_json::to_vec(&plan).expect("plan json");

        let error = harness
            .driver
            .submit_plan(&ctx, &bytes)
            .await
            .expect_err("mkv is not a schema container");
        assert_eq!(error.code(), super::MEDIA_PIPELINE_PLAN_INVALID);
        assert!(harness.media.calls().is_empty());
        assert!(harness.compose.calls().is_empty());
        assert_eq!(
            harness.media.queued(),
            1,
            "the queued response was untouched"
        );
    }

    #[tokio::test]
    async fn resubmitting_the_same_plan_resumes_instead_of_duplicating() {
        let ctx = tool_context();
        let harness = harness(limits(), true);
        harness.media.push(
            super::VIDEO_SUBMIT_TOOL,
            json!({ "id": "job-a", "status": "queued" }),
        );

        let plan = url_only_plan(1);
        let first = harness
            .driver
            .submit_plan(&ctx, &plan)
            .await
            .expect("submit");
        let render_id = first.render_id.to_string();
        let ticked = harness
            .driver
            .advance(&ctx, &render_id)
            .await
            .expect("tick");
        assert_eq!(ticked.scenes[0].stage, SceneStage::Polling);

        let second = harness
            .driver
            .submit_plan(&ctx, &plan)
            .await
            .expect("resubmit");
        assert_eq!(second.render_id, first.render_id);
        assert_eq!(second.revision, ticked.revision, "the resubmit resumed");
        assert_eq!(second.scenes[0].stage, SceneStage::Polling);
        assert_eq!(second.scenes[0].job_id.as_deref(), Some("job-a"));
        assert_eq!(
            harness.media.calls().len(),
            1,
            "resubmission must not re-issue the job"
        );
    }

    #[tokio::test]
    async fn caption_plan_without_a_store_is_rejected_at_submit() {
        let ctx = tool_context();
        let harness = harness(limits(), false);
        let error = harness
            .driver
            .submit_plan(&ctx, &fixture_plan())
            .await
            .expect_err("captions need a store");
        assert_eq!(error.code(), super::MEDIA_PIPELINE_PLAN_INVALID);
        assert!(
            error
                .message()
                .contains("captions require an artifact store")
        );
        assert!(harness.media.calls().is_empty());
        assert!(harness.compose.calls().is_empty());
    }
    #[tokio::test]
    async fn compose_media_failure_reclears_carried_over_transcripts_then_completes() {
        let ctx = tool_context();
        let harness = harness(limits(), true);
        let store = harness.store.clone().expect("store");
        let clip_one = stage(&store, &ctx, "clip-one-bytes").await;
        let clip_two = stage(&store, &ctx, "clip-two-bytes").await;
        let final_clip = stage(&store, &ctx, "final-movie-bytes").await;
        let stale_srt = stage_subtitle(&store, &ctx, "stale srt").await;
        let stale_vtt = stage_subtitle(&store, &ctx, "stale vtt").await;

        let plan = fixture_plan();
        let render_id = seed_done_render(
            &harness,
            &ctx,
            &plan,
            &[clip_one.clone(), clip_two.clone()],
            Some((stale_srt.clone(), stale_vtt)),
        );

        // The clips all verify, so nothing is demotable: the transcript pair
        // carried over from the crashed run is the only suspect input.
        harness.compose.push_call_error(
            super::COMPOSE_TOOL,
            super::VIDEO_COMPOSE_MEDIA_FAILURE,
            "composition inputs failed verification",
        );
        let state = harness
            .driver
            .advance(&ctx, &render_id)
            .await
            .expect("media failure tick");
        assert_eq!(state.status, RenderStatus::Running);
        assert!(state.transcript_srt_artifact.is_none(), "srt was cleared");
        assert!(state.transcript_vtt_artifact.is_none(), "vtt was cleared");
        assert!(
            state
                .scenes
                .iter()
                .all(|scene| scene.stage == SceneStage::Done && scene.clip_artifact.is_some()),
            "verified clips are left alone: {state:?}"
        );

        // The next tick re-stages the transcripts under this run's scope and
        // composes again.
        harness.compose.push(
            super::COMPOSE_TOOL,
            json!({ "artifact": final_clip, "duration_s": 13.5, "byte_length": 17 }),
        );
        let state = harness
            .driver
            .advance(&ctx, &render_id)
            .await
            .expect("recovery tick");
        assert_eq!(state.status, RenderStatus::Completed);
        assert_eq!(state.final_artifact.as_ref(), Some(&final_clip));
        let srt = state.transcript_srt_artifact.clone().expect("restaged srt");
        assert_ne!(srt, stale_srt, "the stale transcript was replaced");
        let bytes = store.get(artifact_scope(&ctx), srt).await.expect("read");
        assert_eq!(
            std::str::from_utf8(&bytes).expect("utf8"),
            "1\n00:00:00,000 --> 00:00:03,500\nHarbors wake up slowly.\n\n2\n00:00:03,500 --> 00:00:07,500\nThen all at once.\n\n"
        );
    }

    #[tokio::test]
    async fn compose_media_failure_with_nothing_to_redo_fails_the_render() {
        let ctx = tool_context();
        let harness = harness(limits(), true);
        let store = harness.store.clone().expect("store");
        let clip = stage(&store, &ctx, "clip-bytes").await;

        // No captions, so no transcripts exist; every clip verifies, so the
        // re-run has nothing left to change and must fail rather than spin.
        let plan = url_only_plan(1);
        let render_id = seed_done_render(&harness, &ctx, &plan, &[clip], None);
        harness.compose.push_call_error(
            super::COMPOSE_TOOL,
            super::VIDEO_COMPOSE_MEDIA_FAILURE,
            "composition inputs failed verification",
        );

        let state = harness
            .driver
            .advance(&ctx, &render_id)
            .await
            .expect("tick");
        assert_eq!(state.status, RenderStatus::Failed);
        assert!(state.final_artifact.is_none());

        // A terminal render is never re-driven, so the loop is closed.
        let again = harness
            .driver
            .advance(&ctx, &render_id)
            .await
            .expect("terminal tick");
        assert_eq!(again.status, RenderStatus::Failed);
        assert_eq!(
            harness.compose.calls().len(),
            1,
            "the failed render is not re-composed"
        );
    }

    #[tokio::test]
    async fn a_scene_tool_error_fails_only_that_scene_and_the_tick_continues() {
        let ctx = tool_context();
        let harness = harness(limits(), true);
        harness.media.push_call_error(
            super::VIDEO_SUBMIT_TOOL,
            "openrouter_media_transport_failed",
            "the provider connection dropped",
        );
        harness.media.push(
            super::VIDEO_SUBMIT_TOOL,
            json!({ "id": "job-b", "status": "queued" }),
        );

        let plan = url_only_plan(2);
        let state = harness
            .driver
            .submit_plan(&ctx, &plan)
            .await
            .expect("submit");
        let render_id = state.render_id.to_string();
        let state = harness
            .driver
            .advance(&ctx, &render_id)
            .await
            .expect("tick");

        assert_eq!(state.scenes[0].stage, SceneStage::Failed);
        let failure = state.scenes[0].failure.clone().expect("failure");
        assert!(
            failure.contains("openrouter_media_transport_failed"),
            "the scene keeps the inner code: {failure}"
        );
        // The pass did not abort: the second scene still made its own call.
        assert_eq!(state.scenes[1].stage, SceneStage::Polling);
        assert_eq!(state.scenes[1].job_id.as_deref(), Some("job-b"));
        assert_eq!(state.status, RenderStatus::Running);
        assert_eq!(harness.media.calls().len(), 2);
    }

    #[tokio::test]
    async fn an_is_error_result_becomes_a_stage_failure_carrying_the_inner_code() {
        let ctx = tool_context();
        let harness = harness(limits(), true);
        harness.media.push_error_result(
            super::VIDEO_SUBMIT_TOOL,
            json!({ "code": "provider_rejected", "message": "unsupported duration" }),
        );

        let plan = url_only_plan(1);
        let state = harness
            .driver
            .submit_plan(&ctx, &plan)
            .await
            .expect("submit");
        let render_id = state.render_id.to_string();
        let state = harness
            .driver
            .advance(&ctx, &render_id)
            .await
            .expect("tick");

        let failure = state.scenes[0].failure.clone().expect("failure");
        assert!(
            failure.starts_with(super::MEDIA_PIPELINE_STAGE_FAILED),
            "mapped to the pipeline code: {failure}"
        );
        assert!(
            failure.contains("provider_rejected"),
            "the inner code survives: {failure}"
        );
        assert_eq!(state.scenes[0].stage, SceneStage::Failed);
        assert_eq!(state.status, RenderStatus::Failed);
    }

    #[tokio::test]
    async fn a_caption_mode_with_no_cues_anywhere_is_rejected_at_submit() {
        let ctx = tool_context();
        let harness = harness(limits(), true);
        let plan = serde_json::to_vec(&json!({
            "version": 1,
            "defaults": {
                "image_model": "test/image-model",
                "video_model": "test/video-model",
                "scene_duration_s": 6,
            },
            "scenes": [{
                "id": "scene-01",
                "video_prompt": "a quiet street",
                "start_frame": { "url": "https://example.test/frame.png" },
            }],
            "output": { "container": "mp4", "captions": "burn_in" },
        }))
        .expect("plan json");

        let error = harness
            .driver
            .submit_plan(&ctx, &plan)
            .await
            .expect_err("captions with nothing to caption");
        assert_eq!(error.code(), super::MEDIA_PIPELINE_PLAN_INVALID);
        assert!(
            error
                .message()
                .contains("captions delivery requested but no scene has cues")
        );
        assert!(harness.media.calls().is_empty());
    }

    #[tokio::test]
    async fn a_prompt_reference_image_is_rejected_at_submit() {
        let ctx = tool_context();
        let harness = harness(limits(), true);
        let plan = serde_json::to_vec(&json!({
            "version": 1,
            "defaults": {
                "image_model": "test/image-model",
                "video_model": "test/video-model",
                "scene_duration_s": 6,
            },
            "scenes": [{
                "id": "scene-01",
                "video_prompt": "a quiet street",
                "start_frame": { "url": "https://example.test/frame.png" },
                "reference_images": [{ "prompt": "moody teal grade" }],
            }],
            "output": { "container": "mp4" },
        }))
        .expect("plan json");

        let error = harness
            .driver
            .submit_plan(&ctx, &plan)
            .await
            .expect_err("an unpinned reference image");
        assert_eq!(error.code(), super::MEDIA_PIPELINE_PLAN_INVALID);
        assert!(
            error
                .message()
                .contains("reference images must be pinned artifacts or urls"),
            "the drop is reported, not silent: {}",
            error.message()
        );
        assert!(harness.media.calls().is_empty());
    }
}
