//! The `MoviePlan` render driver: submit once, then advance in bounded ticks.
//!
//! The driver composes the leaf media toolsets in-process. The kernel-journaled
//! effects are the wrapping `render_movie` / `advance_render` tool calls; the
//! per-stage durability of an individual render lives in the adapter-owned
//! [`RenderStateStore`] (the workflow-local cron adapter precedent). Every
//! [`MediaPipelineDriver::advance`] call performs at most one leaf tool call per
//! scene. A durable claim excludes concurrent ticks; progress is saved after
//! each scene. Interrupted claims require host reconciliation before retry;
//! [`MediaPipelineDriver::release_claim`] is the host's way to abandon one.

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
            failure: None,
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
    /// `limits.max_concurrent_jobs`. A durable [`RenderStatus::Advancing`]
    /// claim is saved before dispatch. Concurrent callers only read that
    /// claim; interruption requires host reconciliation, never a timed retry.
    ///
    /// A failure in the composition step is persisted by what it means:
    /// a persisted row that cannot be composed (a finished scene with no
    /// clip) is terminal and is saved as [`RenderStatus::Failed`] with the
    /// reason in [`RenderState::failure`]; a transcript-staging failure
    /// happens before any paid call, so the claim is released by restoring
    /// the pre-tick status and the next tick retries it. Only a failure after
    /// the compose tool was dispatched keeps the claim, because the effect's
    /// outcome is then genuinely unknown.
    ///
    /// # Errors
    ///
    /// Returns [`MEDIA_PIPELINE_NOT_FOUND`] for an unknown render,
    /// [`MEDIA_PIPELINE_PLAN_INVALID`] when the persisted plan no longer
    /// parses, [`MEDIA_PIPELINE_STORE_FAILURE`] on a store failure, and
    /// [`MEDIA_PIPELINE_STAGE_FAILED`] when transcript staging fails, the
    /// persisted row cannot be composed, or the compose result is unusable.
    pub async fn advance(
        &self,
        ctx: &ToolCallContext,
        render_id: &str,
    ) -> Result<RenderState, ToolError> {
        let mut state = self.status(ctx.run.locator.tenant_scope.as_ref(), render_id)?;
        if matches!(
            state.status,
            RenderStatus::Completed | RenderStatus::Failed | RenderStatus::Advancing
        ) {
            return Ok(state);
        }
        let plan: MoviePlan = serde_json::from_str(&state.plan_json).map_err(|_| {
            tool_error(
                MEDIA_PIPELINE_PLAN_INVALID,
                ErrorCategory::Validation,
                "persisted movie plan no longer matches the plan schema",
            )
        })?;

        if ctx.run.cancellation.is_cancelled() {
            return Ok(state);
        }
        let previous_status = state.status;
        state.status = RenderStatus::Advancing;
        if !self.state.update(&state).map_err(|_| store_failure())? {
            return self.status(ctx.run.locator.tenant_scope.as_ref(), render_id);
        }
        state.revision = state.revision.saturating_add(1);
        // Keep the persisted claim until the final write; intermediate
        // progress saves must not reopen the render to a second caller.
        state.status = previous_status;

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
                scene.failure = Some(failure_reason(&error));
            }
            let after = scene.stage;
            if before != SceneStage::Polling && after == SceneStage::Polling {
                in_flight = in_flight.saturating_add(1);
            } else if before == SceneStage::Polling && after != SceneStage::Polling {
                in_flight = in_flight.saturating_sub(1);
            }
            self.persist_progress(&mut state)?;
        }

        if !cancelled {
            match self.settle(ctx, &plan, &mut state).await {
                Ok(()) => {}
                Err(SettleFailure::Retryable(error)) => {
                    // Nothing paid happened: release the claim so the next
                    // tick can retry, keeping the scene progress saved so far.
                    state.status = previous_status;
                    self.persist(&mut state)?;
                    return Err(error);
                }
                Err(SettleFailure::Terminal(error)) => {
                    state.status = RenderStatus::Failed;
                    state.failure = Some(failure_reason(&error));
                    self.persist(&mut state)?;
                    return Err(error);
                }
                // The compose effect was dispatched and its outcome is
                // unknown: the claim stays until the host reconciles it.
                Err(SettleFailure::Uncertain(error)) => return Err(error),
            }
        }
        self.persist(&mut state)
    }

    /// Abandon an interrupted tick: move a render from
    /// [`RenderStatus::Advancing`] to [`RenderStatus::Failed`], recording
    /// `reason` in [`RenderState::failure`].
    ///
    /// This is the host reconciliation API for a claim whose owner was
    /// interrupted after a paid dispatch began. The host decides, outside
    /// this crate, that the render will not be resumed; the driver only
    /// performs the compare-and-set so no concurrent writer is overwritten.
    /// A render that holds no claim is left untouched.
    ///
    /// # Errors
    ///
    /// Returns [`MEDIA_PIPELINE_NOT_FOUND`] for an unknown render,
    /// [`MEDIA_PIPELINE_INVALID_ARGUMENTS`] when the render is not
    /// `Advancing`, when `reason` is empty or longer than
    /// [`MAX_RELEASE_REASON_BYTES`], or when the row changed between the read
    /// and the write (re-read and retry), and
    /// [`MEDIA_PIPELINE_STORE_FAILURE`] when the store cannot be reached.
    #[allow(
        clippy::unused_async,
        reason = "release_claim is async by contract, like submit_plan, so a \
                  store-backed reconciliation step can be awaited here \
                  without a breaking change"
    )]
    pub async fn release_claim(
        &self,
        tenant_scope: &str,
        render_id: &str,
        reason: &str,
    ) -> Result<RenderState, ToolError> {
        if reason.is_empty() || reason.len() > MAX_RELEASE_REASON_BYTES {
            return Err(tool_error(
                MEDIA_PIPELINE_INVALID_ARGUMENTS,
                ErrorCategory::Validation,
                "release reason must be non-empty and at most 1024 bytes",
            ));
        }
        let mut state = self.status(tenant_scope, render_id)?;
        if state.status != RenderStatus::Advancing {
            return Err(tool_error(
                MEDIA_PIPELINE_INVALID_ARGUMENTS,
                ErrorCategory::Validation,
                "the render holds no advancing claim to release",
            ));
        }
        state.status = RenderStatus::Failed;
        state.failure = Some(reason.to_owned());
        if !self.state.update(&state).map_err(|_| store_failure())? {
            return Err(tool_error(
                MEDIA_PIPELINE_INVALID_ARGUMENTS,
                ErrorCategory::Validation,
                "the render changed concurrently; re-read its status and retry",
            ));
        }
        state.revision = state.revision.saturating_add(1);
        Ok(state)
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
                let arguments = {
                    let job_id = state
                        .scenes
                        .get(index)
                        .and_then(|scene| scene.job_id.as_deref())
                        .ok_or_else(|| stage_error("polling scene has no job id"))?;
                    json!({ "id": job_id, "wait_seconds": POLL_WAIT_SECONDS })
                };
                let result =
                    invoke_tool(&self.media_tools, VIDEO_STATUS_TOOL, arguments, ctx).await?;
                let status = result
                    .get("status")
                    .and_then(Value::as_str)
                    .ok_or_else(|| stage_error("video status result carried no status"))?;
                let Some(scene) = state.scenes.get_mut(index) else {
                    return Ok(());
                };
                match status {
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
                    _ => {}
                }
                Ok(())
            }
            SceneStage::PendingDownload => {
                let arguments = {
                    let job_id = state
                        .scenes
                        .get(index)
                        .and_then(|scene| scene.job_id.as_deref())
                        .ok_or_else(|| stage_error("download stage has no job id"))?;
                    json!({ "id": job_id })
                };
                let result =
                    invoke_tool(&self.media_tools, VIDEO_DOWNLOAD_TOOL, arguments, ctx).await?;
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
    ///
    /// Each failure is classified by whether anything paid happened before
    /// it, so [`Self::advance`] can persist the right status; see
    /// [`SettleFailure`].
    async fn settle(
        &self,
        ctx: &ToolCallContext,
        plan: &MoviePlan,
        state: &mut RenderState,
    ) -> Result<(), SettleFailure> {
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
            // `submit_plan` rejects a caption plan without a store, so this
            // row was edited out from under the driver: terminal.
            let Some(store) = self.artifact_store.as_ref() else {
                return Err(SettleFailure::Terminal(stage_error(
                    "captions require an artifact store",
                )));
            };
            let scope = artifact_scope(ctx);
            let srt = stage_text(
                store.as_ref(),
                &scope,
                to_srt(&cues),
                "application/x-subrip",
                "transcript.srt",
            )
            .await
            .map_err(SettleFailure::Retryable)?;
            let vtt = stage_text(
                store.as_ref(),
                &scope,
                to_vtt(&cues),
                "text/vtt",
                "transcript.vtt",
            )
            .await
            .map_err(SettleFailure::Retryable)?;
            state.transcript_srt_artifact = Some(srt);
            state.transcript_vtt_artifact = Some(vtt);
            // A lost or failed write here leaves the claim in place: the
            // staged transcripts are idempotent, but the write's own outcome
            // is unknown and must not be guessed by a retry.
            self.persist_progress(state)
                .map_err(SettleFailure::Uncertain)?;
            transcripts_staged_now = true;
        }

        let spec = compose_spec(plan, state).map_err(SettleFailure::Terminal)?;
        match invoke_tool(&self.compose_tools, COMPOSE_TOOL, spec, ctx).await {
            Ok(result) => {
                let artifact = parse_artifact(&result).ok_or_else(|| {
                    SettleFailure::Uncertain(stage_error("composition returned no artifact"))
                })?;
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

    /// Save completed stages while retaining exclusive tick ownership.
    fn persist_progress(&self, state: &mut RenderState) -> Result<(), ToolError> {
        let mut claimed = state.clone();
        claimed.status = RenderStatus::Advancing;
        if !self.state.update(&claimed).map_err(|_| store_failure())? {
            return Err(store_failure());
        }
        state.revision = state.revision.saturating_add(1);
        Ok(())
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

/// Why [`MediaPipelineDriver::settle`] stopped, by what it means for the
/// durable claim. The variant, not the error code, decides what
/// [`MediaPipelineDriver::advance`] persists.
enum SettleFailure {
    /// No paid call happened and nothing durable changed (transcript
    /// staging I/O): release the claim and let the next tick retry.
    Retryable(ToolError),
    /// The persisted row can never be composed as it stands (pure,
    /// pre-effect): fail the render with the reason.
    Terminal(ToolError),
    /// A durable write or the compose dispatch already began and its
    /// outcome is unknown: keep the claim for host reconciliation.
    Uncertain(ToolError),
}

/// Upper bound on a host-supplied `release_claim` reason, in bytes.
pub const MAX_RELEASE_REASON_BYTES: usize = 1024;

/// The `code: message` form both scenes and renders record as their reason.
fn failure_reason(error: &ToolError) -> String {
    format!("{}: {}", error.code(), error.message())
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
    let stage = if matches!(scene.start_frame, FrameSource::Prompt { .. }) {
        SceneStage::PendingStartFrame
    } else if matches!(scene.end_frame, Some(FrameSource::Prompt { .. })) {
        SceneStage::PendingEndFrame
    } else {
        SceneStage::PendingSubmit
    };
    let (start_frame_artifact, start_frame_url) = pinned_frame(Some(&scene.start_frame));
    let (end_frame_artifact, end_frame_url) = pinned_frame(scene.end_frame.as_ref());
    SceneState {
        scene_id: scene.id.clone(),
        stage,
        start_frame_artifact,
        start_frame_url,
        end_frame_artifact,
        end_frame_url,
        job_id: None,
        clip_artifact: None,
        failure: None,
        resubmitted: false,
    }
}

/// The already-resolved `(artifact, url)` of a frame source; a prompt (or
/// no frame) resolves nothing until it is generated.
fn pinned_frame(source: Option<&FrameSource>) -> (Option<ArtifactRef>, Option<String>) {
    match source {
        Some(FrameSource::Artifact { artifact }) => (Some(artifact.clone()), None),
        Some(FrameSource::Url { url }) => (None, Some(url.clone())),
        Some(FrameSource::Prompt { .. }) | None => (None, None),
    }
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
                    || json!({ "type": TransitionKindName::Cut }),
                    |transition| match transition.duration_s {
                        Some(duration) if transition.kind != TransitionKindName::Cut => {
                            json!({ "type": transition.kind, "duration_s": duration })
                        }
                        _ => json!({ "type": transition.kind }),
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
    if ctx.run.cancellation.is_cancelled() {
        return Err(stage_error("pipeline tool dispatch was cancelled"));
    }
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
        return Err(tool_error(
            MEDIA_PIPELINE_STAGE_FAILED,
            ErrorCategory::Tool,
            &format!("pipeline tool returned an error result: {code}"),
        ));
    }
    Ok(value)
}

fn tool_error(code: &'static str, category: ErrorCategory, message: &str) -> ToolError {
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

#[cfg(test)]
#[path = "driver_tests.rs"]
mod tests;
#[cfg(test)]
pub(crate) use tests::test_support;
