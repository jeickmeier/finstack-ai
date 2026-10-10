//! Unit tests for the render driver, plus the shared `test_support` doubles
//! that `tools`'s tests reuse. Split out of `driver.rs` so the production
//! file stays readable; `super` here is the `driver` module.

/// Shared test infrastructure reused by both `driver`'s own tests and
/// `tools`'s tests: a queued-result `Toolset` double, fixture plans, and the
/// harness that wires a [`MediaPipelineDriver`] to two of those doubles.
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

    use crate::driver::{
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

    /// Stage `body` as an `mp4` clip under the run's artifact scope.
    pub(crate) async fn stage(
        store: &Arc<dyn ArtifactStore>,
        ctx: &ToolCallContext,
        body: &str,
    ) -> ArtifactRef {
        stage_as(store, ctx, body, "video/mp4", "clip.mp4").await
    }

    /// Stage `body` as an SRT transcript under the run's artifact scope.
    pub(crate) async fn stage_subtitle(
        store: &Arc<dyn ArtifactStore>,
        ctx: &ToolCallContext,
        body: &str,
    ) -> ArtifactRef {
        stage_as(store, ctx, body, "application/x-subrip", "transcript.srt").await
    }

    async fn stage_as(
        store: &Arc<dyn ArtifactStore>,
        ctx: &ToolCallContext,
        body: &str,
        media_type: &str,
        name: &str,
    ) -> ArtifactRef {
        stage_required_artifact(
            store.as_ref(),
            artifact_scope(ctx),
            Bytes::from(body.as_bytes().to_vec()),
            ArtifactMetadata {
                kind: Arc::from("tool-output"),
                media_type: Arc::from(media_type),
                name: Some(Arc::from(name)),
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
            crate::driver::IMAGE_TOOL,
            crate::driver::VIDEO_SUBMIT_TOOL,
            crate::driver::VIDEO_STATUS_TOOL,
            crate::driver::VIDEO_DOWNLOAD_TOOL,
        ]);
        let compose = QueueToolset::new(&[crate::driver::COMPOSE_TOOL]);
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
                crate::driver::IMAGE_TOOL,
                json!({ "artifact": image, "media_type": "image/png", "byte_length": 11 }),
            );
        }
        for id in ["job-a", "job-b"] {
            harness.media.push(
                crate::driver::VIDEO_SUBMIT_TOOL,
                json!({ "id": id, "status": "queued" }),
            );
        }
        harness.media.push(
            crate::driver::VIDEO_STATUS_TOOL,
            json!({ "id": "job-a", "status": "pending" }),
        );
        for id in ["job-a", "job-b"] {
            harness.media.push(
                crate::driver::VIDEO_STATUS_TOOL,
                json!({ "id": id, "status": "completed", "urls": [] }),
            );
        }
        for clip in clips {
            harness.media.push(
                crate::driver::VIDEO_DOWNLOAD_TOOL,
                json!({ "artifact": clip, "media_type": "video/mp4", "byte_length": 14 }),
            );
        }
        harness.compose.push(
            crate::driver::COMPOSE_TOOL,
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
            failure: None,
            revision: 0,
        };
        harness.state.insert(&state).expect("seed");
        render_id.to_string()
    }
}

use serde_json::json;

use super::{PlanLimits, RenderStatus, SceneStage, artifact_scope};
use test_support::{
    fixture_plan, harness, limits, prime_happy_path, seed_done_render, stage, stage_subtitle,
    tool_context, url_only_plan,
};

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
