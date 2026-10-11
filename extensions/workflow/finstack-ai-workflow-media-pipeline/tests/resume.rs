//! Resume golden tests for `MediaPipelineDriver`.
//!
//! These are integration tests: only the crate's public API is used. A crash
//! is simulated by dropping the driver and its toolsets and rebuilding fresh
//! ones over the same sqlite-backed [`SqliteRenderStateStore`] path, so a
//! "resume" here is a genuine cold start, not merely a second driver method
//! call on the same in-memory objects.

use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use finstack_ai_kernel::{
    ArtifactRef, Digest, EffectId, LaneId, Metadata, OperationLocator, PrincipalRef, RawJson,
    RetrySafety, RunId, SessionId, ToolBatchId, ToolCallId, ToolExecutionMode, ToolId,
    ValidatedToolCall,
};
use finstack_ai_runtime::Bytes;
use finstack_ai_runtime::artifact::{
    ArtifactError, ArtifactMetadata, ArtifactScope, ArtifactStore, ArtifactStoreLimits,
    InProcessArtifactStore, stage_required_artifact,
};
use finstack_ai_runtime::ports::PortFuture;
use finstack_ai_runtime::ports::model::{
    ApprovalMetadata, ApprovalRequirement, AuthorizationContext, CancellationSignal,
    RunCallContext, SideEffectClass, ToolDeferralSupport, ToolSpec,
};
use finstack_ai_runtime::ports::tool::{
    ToolCallContext, ToolError, ToolEventStream, ToolResult, ToolStreamItem, Toolset,
    ToolsetDescriptor,
};
use finstack_ai_workflow_media_pipeline::{
    MEDIA_PIPELINE_INVALID_ARGUMENTS, MEDIA_PIPELINE_STAGE_FAILED, MediaPipelineConfig,
    MediaPipelineDriver, PlanLimits, RenderState, RenderStateStore, RenderStatus, SceneStage,
    SceneState, SqliteRenderStateStore,
};
use futures_util::stream;
use serde_json::{Value, json};

// ----- shared test infrastructure ---------------------------------------

/// A minimal `Toolset` double that pops the next queued outcome for whichever
/// tool name is invoked, recording every call it saw.
#[derive(Debug)]
struct QueueToolset {
    tools: Arc<[ToolSpec]>,
    queues: Mutex<BTreeMap<String, VecDeque<Queued>>>,
    calls: Mutex<Vec<(String, Value)>>,
}

#[derive(Debug, Clone)]
enum Queued {
    /// A normal successful result payload.
    Ok(Value),
    /// An adapter error raised by `call` before any stream exists (mirrors a
    /// leaf tool that fails closed with a stable code, e.g. a compose media
    /// failure).
    CallError(ToolError),
}

impl QueueToolset {
    fn new(names: &[&str]) -> Arc<Self> {
        let tools: Vec<ToolSpec> = names.iter().map(|name| spec_for(name)).collect();
        Arc::new(Self {
            tools: Arc::from(tools),
            queues: Mutex::new(BTreeMap::new()),
            calls: Mutex::new(Vec::new()),
        })
    }

    fn push(&self, tool: &str, result: Value) {
        self.enqueue(tool, Queued::Ok(result));
    }

    fn push_call_error(&self, tool: &str, code: &str, message: &str) {
        let error = ToolError::try_new(
            code,
            finstack_ai_kernel::ErrorCategory::Tool,
            false,
            message,
            Metadata::empty(),
        )
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

    fn calls(&self) -> Vec<(String, Value)> {
        self.calls.lock().unwrap().clone()
    }
}

fn spec_for(name: &str) -> ToolSpec {
    ToolSpec {
        id: ToolId::parse(format!("finstack.tools.{name}")).expect("tool id"),
        model_name: Arc::from(name),
        title: Arc::from(name),
        description: Arc::from("resume-test double"),
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
            name: Arc::from("resume-test-queue-toolset"),
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
        match entry {
            Queued::Ok(payload) => {
                let output = RawJson::parse(serde_json::to_vec(&payload).expect("encode"))
                    .expect("canonical json");
                let item = ToolStreamItem::Completed(ToolResult {
                    output,
                    is_error: false,
                });
                Box::pin(
                    async move { Ok(Box::pin(stream::iter(vec![Ok(item)])) as ToolEventStream) },
                )
            }
            Queued::CallError(error) => Box::pin(async move { Err(error) }),
        }
    }
}

const IMAGE_TOOL: &str = "openrouter_generate_image";
const VIDEO_SUBMIT_TOOL: &str = "openrouter_generate_video";
const VIDEO_STATUS_TOOL: &str = "openrouter_get_video";
const VIDEO_DOWNLOAD_TOOL: &str = "openrouter_download_video";
const COMPOSE_TOOL: &str = "compose_video";
const VIDEO_COMPOSE_MEDIA_FAILURE: &str = "video_compose_media_failure";

fn media_toolset() -> Arc<QueueToolset> {
    QueueToolset::new(&[
        IMAGE_TOOL,
        VIDEO_SUBMIT_TOOL,
        VIDEO_STATUS_TOOL,
        VIDEO_DOWNLOAD_TOOL,
    ])
}

fn compose_toolset() -> Arc<QueueToolset> {
    QueueToolset::new(&[COMPOSE_TOOL])
}

fn limits() -> PlanLimits {
    PlanLimits {
        max_scenes: 10,
        max_total_video_s: 240,
        max_concurrent_jobs: 2,
    }
}

/// The same `tool_context()` construction as the E2B toolset and the
/// in-crate driver tests: fixed tenant/session/run identity so plan digests
/// and artifact scopes are deterministic across "crash" boundaries.
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

fn artifact_scope(ctx: &ToolCallContext) -> ArtifactScope {
    ArtifactScope {
        tenant_scope: Arc::clone(&ctx.run.locator.tenant_scope),
        session_id: ctx.run.locator.session_id,
        run_id: Some(ctx.run.locator.run_id),
        sensitivity: finstack_ai_kernel::Sensitivity::Internal,
    }
}

async fn stage(
    store: &Arc<dyn ArtifactStore>,
    ctx: &ToolCallContext,
    body: &str,
) -> finstack_ai_kernel::ArtifactRef {
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

/// A two-scene plan whose scenes are both pinned to a URL start frame, so no
/// image generation happens and every scene begins in `PendingSubmit`.
fn two_scene_plan() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "version": 1,
        "defaults": {
            "image_model": "test/image-model",
            "video_model": "test/video-model",
            "scene_duration_s": 6,
        },
        "scenes": [
            {
                "id": "scene-01",
                "video_prompt": "scene one",
                "start_frame": { "url": "https://example.test/frame-one.png" },
            },
            {
                "id": "scene-02",
                "video_prompt": "scene two",
                "start_frame": { "url": "https://example.test/frame-two.png" },
            },
        ],
        "output": { "container": "mp4" },
    }))
    .expect("plan json")
}

fn build_driver(
    media: &Arc<QueueToolset>,
    compose: &Arc<QueueToolset>,
    state_path: &std::path::Path,
    store: Option<Arc<dyn ArtifactStore>>,
) -> MediaPipelineDriver {
    let state = SqliteRenderStateStore::open(state_path).expect("open sqlite state store");
    MediaPipelineDriver::try_new(MediaPipelineConfig {
        media_tools: Arc::clone(media) as Arc<dyn Toolset>,
        compose_tools: Arc::clone(compose) as Arc<dyn Toolset>,
        state: Arc::new(state),
        artifact_store: store,
        limits: limits(),
    })
    .expect("driver")
}

// ----- test 1: crash after scene-01 downloads, scene-02 still polling ---

#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "the golden ticks through every stage of a two-scene render across \
              a simulated crash; splitting it would scatter one linear narrative"
)]
async fn killed_after_scene_one_download_resumes_at_scene_two() {
    let ctx = tool_context();
    let dir = tempfile::tempdir().expect("tempdir");
    let state_path = dir.path().join("journal.sqlite");
    let store: Arc<dyn ArtifactStore> = Arc::new(InProcessArtifactStore::default());

    // ----- pre-crash driver ---------------------------------------------
    let media = media_toolset();
    let compose = compose_toolset();
    let driver = build_driver(&media, &compose, &state_path, Some(Arc::clone(&store)));

    let plan = two_scene_plan();
    let submitted = driver.submit_plan(&ctx, &plan).await.expect("submit");
    let render_id = submitted.render_id.to_string();
    assert_eq!(submitted.scenes[0].stage, SceneStage::PendingSubmit);
    assert_eq!(submitted.scenes[1].stage, SceneStage::PendingSubmit);

    // Tick 1: both scenes submit their jobs.
    media.push(
        VIDEO_SUBMIT_TOOL,
        json!({ "id": "job-a", "status": "queued" }),
    );
    media.push(
        VIDEO_SUBMIT_TOOL,
        json!({ "id": "job-b", "status": "queued" }),
    );
    let state = driver.advance(&ctx, &render_id).await.expect("tick 1");
    assert_eq!(state.scenes[0].stage, SceneStage::Polling);
    assert_eq!(state.scenes[1].stage, SceneStage::Polling);

    // Tick 2: scene-01's job completes; scene-02's is still in progress.
    media.push(
        VIDEO_STATUS_TOOL,
        json!({ "id": "job-a", "status": "completed", "urls": [] }),
    );
    media.push(
        VIDEO_STATUS_TOOL,
        json!({ "id": "job-b", "status": "in_progress" }),
    );
    let state = driver.advance(&ctx, &render_id).await.expect("tick 2");
    assert_eq!(state.scenes[0].stage, SceneStage::PendingDownload);
    assert_eq!(state.scenes[1].stage, SceneStage::Polling);

    // Tick 3: scene-01 downloads its clip and reaches `Done`; scene-02 polls
    // again and is still not finished. This is the crash point.
    let clip_one = stage(&store, &ctx, "scene-one-clip-bytes").await;
    media.push(
        VIDEO_DOWNLOAD_TOOL,
        json!({ "artifact": clip_one, "media_type": "video/mp4", "byte_length": 21 }),
    );
    media.push(
        VIDEO_STATUS_TOOL,
        json!({ "id": "job-b", "status": "in_progress" }),
    );
    let pre_crash = driver.advance(&ctx, &render_id).await.expect("tick 3");
    assert_eq!(pre_crash.scenes[0].stage, SceneStage::Done);
    assert_eq!(
        pre_crash.scenes[0].clip_artifact.as_ref(),
        Some(&clip_one),
        "scene-01's clip is staged before the crash"
    );
    assert_eq!(pre_crash.scenes[1].stage, SceneStage::Polling);
    assert_eq!(pre_crash.status, RenderStatus::Running);

    // ----- simulated crash: drop the driver and both toolset doubles ----
    drop(driver);
    drop(media);
    drop(compose);

    // ----- fresh driver over the same sqlite file, seeded only with
    // scene-02's remaining calls -------------------------------------------
    let fresh_media = media_toolset();
    let fresh_compose = compose_toolset();
    let fresh_driver = build_driver(
        &fresh_media,
        &fresh_compose,
        &state_path,
        Some(Arc::clone(&store)),
    );

    fresh_media.push(
        VIDEO_STATUS_TOOL,
        json!({ "id": "job-b", "status": "completed", "urls": [] }),
    );
    let state = fresh_driver
        .advance(&ctx, &render_id)
        .await
        .expect("resumed tick 1");
    assert_eq!(
        state.scenes[0].stage,
        SceneStage::Done,
        "scene-01 untouched"
    );
    assert_eq!(state.scenes[1].stage, SceneStage::PendingDownload);

    let clip_two = stage(&store, &ctx, "scene-two-clip-bytes").await;
    fresh_media.push(
        VIDEO_DOWNLOAD_TOOL,
        json!({ "artifact": clip_two, "media_type": "video/mp4", "byte_length": 22 }),
    );
    let final_clip = stage(&store, &ctx, "final-movie-bytes").await;
    fresh_compose.push(
        COMPOSE_TOOL,
        json!({ "artifact": final_clip, "duration_s": 12.0, "byte_length": 30 }),
    );
    let state = fresh_driver
        .advance(&ctx, &render_id)
        .await
        .expect("resumed tick 2");

    assert_eq!(state.status, RenderStatus::Completed, "{state:?}");
    assert_eq!(state.final_artifact.as_ref(), Some(&final_clip));
    assert_eq!(
        state.scenes[0].clip_artifact.as_ref(),
        Some(&clip_one),
        "scene-01's clip survived the crash unchanged"
    );
    assert_eq!(
        state.scenes[0]
            .clip_artifact
            .as_ref()
            .map(finstack_ai_kernel::ArtifactRef::content_digest),
        Some(clip_one.content_digest()),
    );

    // The fresh media double never saw a single scene-01 call: only the
    // status poll and download for scene-02 (2 calls total).
    let calls = fresh_media.calls();
    assert_eq!(calls.len(), 2, "{calls:?}");
    assert!(
        calls
            .iter()
            .all(|(_, args)| args.get("id").and_then(Value::as_str) != Some("job-a")),
        "no scene-01 call was consumed after resume: {calls:?}"
    );
}

// ----- test 2: a corrupted clip fails closed, then the scene re-runs ----

#[tokio::test]
async fn corrupted_clip_fails_closed_then_reruns_the_scene() {
    let ctx = tool_context();
    let dir = tempfile::tempdir().expect("tempdir");
    let state_path = dir.path().join("journal.sqlite");
    let store: Arc<dyn ArtifactStore> = Arc::new(InProcessArtifactStore::default());
    // A second, independent store used only to mint a well-formed
    // `ArtifactRef` that the driver's own store never received: reading it
    // back through `store` fails, which is exactly what "corrupted" means
    // from the driver's point of view (it never inspects bytes itself).
    let other_store: Arc<dyn ArtifactStore> = Arc::new(InProcessArtifactStore::default());

    let media = media_toolset();
    let compose = compose_toolset();
    let driver = build_driver(&media, &compose, &state_path, Some(Arc::clone(&store)));

    let plan = two_scene_plan();
    let submitted = driver.submit_plan(&ctx, &plan).await.expect("submit");
    let render_id = submitted.render_id.to_string();

    // Drive both scenes to `Done`: submit, poll to completion, download.
    media.push(
        VIDEO_SUBMIT_TOOL,
        json!({ "id": "job-a", "status": "queued" }),
    );
    media.push(
        VIDEO_SUBMIT_TOOL,
        json!({ "id": "job-b", "status": "queued" }),
    );
    driver.advance(&ctx, &render_id).await.expect("submit tick");

    media.push(
        VIDEO_STATUS_TOOL,
        json!({ "id": "job-a", "status": "completed", "urls": [] }),
    );
    media.push(
        VIDEO_STATUS_TOOL,
        json!({ "id": "job-b", "status": "completed", "urls": [] }),
    );
    driver.advance(&ctx, &render_id).await.expect("poll tick");

    let clip_one = stage(&store, &ctx, "scene-one-clip-bytes").await;
    // Genuinely unverifiable: minted from a different store instance, so
    // `store.get` on the driver's own store cannot find it.
    let clip_two_corrupt = stage(&other_store, &ctx, "scene-two-clip-bytes").await;
    media.push(
        VIDEO_DOWNLOAD_TOOL,
        json!({ "artifact": clip_one, "media_type": "video/mp4", "byte_length": 21 }),
    );
    media.push(
        VIDEO_DOWNLOAD_TOOL,
        json!({ "artifact": clip_two_corrupt, "media_type": "video/mp4", "byte_length": 22 }),
    );
    // Composition is attempted the moment both downloads land, so queue the
    // media failure now.
    compose.push_call_error(
        COMPOSE_TOOL,
        VIDEO_COMPOSE_MEDIA_FAILURE,
        "composition inputs failed verification",
    );
    let state = driver
        .advance(&ctx, &render_id)
        .await
        .expect("download + compose-failure tick");

    assert_eq!(state.status, RenderStatus::Running, "{state:?}");
    assert_eq!(
        state.scenes[0].stage,
        SceneStage::Done,
        "scene-01's clip verifies and is left alone"
    );
    assert_eq!(state.scenes[0].clip_artifact.as_ref(), Some(&clip_one));
    assert_eq!(
        state.scenes[1].stage,
        SceneStage::PendingDownload,
        "scene-02 is demoted back to download: it still has a job id"
    );
    assert!(
        state.scenes[1].clip_artifact.is_none(),
        "the unverifiable clip is dropped"
    );
    assert_eq!(state.scenes[1].job_id.as_deref(), Some("job-b"));

    // Seed a re-download plus a successful composition; the render completes.
    let clip_two_repaired = stage(&store, &ctx, "scene-two-repaired-bytes").await;
    media.push(
        VIDEO_DOWNLOAD_TOOL,
        json!({ "artifact": clip_two_repaired, "media_type": "video/mp4", "byte_length": 23 }),
    );
    let final_clip = stage(&store, &ctx, "final-movie-bytes").await;
    compose.push(
        COMPOSE_TOOL,
        json!({ "artifact": final_clip, "duration_s": 12.0, "byte_length": 30 }),
    );
    let state = driver
        .advance(&ctx, &render_id)
        .await
        .expect("recovery tick");

    assert_eq!(state.status, RenderStatus::Completed, "{state:?}");
    assert_eq!(state.final_artifact.as_ref(), Some(&final_clip));
    assert_eq!(
        state.scenes[1].clip_artifact.as_ref(),
        Some(&clip_two_repaired)
    );
}

// ----- test 3: resubmitting the same plan after a crash returns progress -

#[tokio::test]
async fn resubmitted_plan_after_crash_returns_the_persisted_render() {
    let ctx = tool_context();
    let dir = tempfile::tempdir().expect("tempdir");
    let state_path = dir.path().join("journal.sqlite");

    let media = media_toolset();
    let compose = compose_toolset();
    let driver = build_driver(&media, &compose, &state_path, None);

    let plan = two_scene_plan();
    let first = driver.submit_plan(&ctx, &plan).await.expect("submit");
    let render_id = first.render_id.to_string();
    assert_eq!(first.revision, 0);

    media.push(
        VIDEO_SUBMIT_TOOL,
        json!({ "id": "job-a", "status": "queued" }),
    );
    media.push(
        VIDEO_SUBMIT_TOOL,
        json!({ "id": "job-b", "status": "queued" }),
    );
    let ticked = driver.advance(&ctx, &render_id).await.expect("tick");
    assert_eq!(ticked.scenes[0].stage, SceneStage::Polling);
    assert_eq!(ticked.scenes[1].stage, SceneStage::Polling);
    assert!(ticked.revision > first.revision);

    // ----- simulated crash -----------------------------------------------
    drop(driver);
    drop(media);
    drop(compose);

    // ----- fresh driver, resubmitting the byte-identical plan ------------
    let fresh_media = media_toolset();
    let fresh_compose = compose_toolset();
    let fresh_driver = build_driver(&fresh_media, &fresh_compose, &state_path, None);

    let resubmitted = fresh_driver
        .submit_plan(&ctx, &plan)
        .await
        .expect("resubmit after crash");

    assert_eq!(resubmitted.render_id, first.render_id);
    assert_eq!(
        resubmitted.revision, ticked.revision,
        "resubmission returns the persisted row rather than starting over"
    );
    assert_eq!(resubmitted.status, RenderStatus::Running);
    assert_eq!(resubmitted.scenes[0].stage, SceneStage::Polling);
    assert_eq!(resubmitted.scenes[0].job_id.as_deref(), Some("job-a"));
    assert_eq!(resubmitted.scenes[1].stage, SceneStage::Polling);
    assert_eq!(resubmitted.scenes[1].job_id.as_deref(), Some("job-b"));

    // Resubmission must not re-issue any tool call.
    assert!(fresh_media.calls().is_empty());
    assert!(fresh_compose.calls().is_empty());
}

// ----- shared infrastructure for the settle-failure goldens ---------------

/// An `ArtifactStore` that fails its first `stage_put` with a transient
/// error and then behaves like the wrapped in-process store.
#[derive(Debug)]
struct FailOnceStore {
    inner: InProcessArtifactStore,
    fail_next_put: AtomicBool,
}

impl FailOnceStore {
    fn new() -> Self {
        Self {
            inner: InProcessArtifactStore::default(),
            fail_next_put: AtomicBool::new(false),
        }
    }

    fn fail_next_put(&self) {
        self.fail_next_put.store(true, Ordering::SeqCst);
    }
}

impl ArtifactStore for FailOnceStore {
    fn stage_put(
        &self,
        scope: ArtifactScope,
        content: Bytes,
        metadata: ArtifactMetadata,
    ) -> PortFuture<Result<ArtifactRef, ArtifactError>> {
        if self.fail_next_put.swap(false, Ordering::SeqCst) {
            return Box::pin(async {
                Err(ArtifactError::Unavailable {
                    message: Arc::from("artifact backend offline"),
                })
            });
        }
        self.inner.stage_put(scope, content, metadata)
    }

    fn get(
        &self,
        scope: ArtifactScope,
        artifact: ArtifactRef,
    ) -> PortFuture<Result<Bytes, ArtifactError>> {
        self.inner.get(scope, artifact)
    }

    fn limits(&self) -> ArtifactStoreLimits {
        self.inner.limits()
    }
}

/// The two-scene plan with caption cues on scene one and burn-in delivery,
/// so composing must stage a transcript pair first.
fn captioned_two_scene_plan() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "version": 1,
        "defaults": {
            "image_model": "test/image-model",
            "video_model": "test/video-model",
            "scene_duration_s": 6,
        },
        "scenes": [
            {
                "id": "scene-01",
                "video_prompt": "scene one",
                "start_frame": { "url": "https://example.test/frame-one.png" },
                "captions": [
                    { "text": "Harbors wake up slowly.", "start_s": 0.0, "end_s": 3.5 }
                ],
            },
            {
                "id": "scene-02",
                "video_prompt": "scene two",
                "start_frame": { "url": "https://example.test/frame-two.png" },
            },
        ],
        "output": { "container": "mp4", "captions": "burn_in" },
    }))
    .expect("plan json")
}

/// A `Done` scene state with (or, for the compose-spec golden, without) a
/// downloaded clip.
fn done_scene(id: &str, job_id: &str, clip: Option<ArtifactRef>) -> SceneState {
    SceneState {
        scene_id: id.to_owned(),
        stage: SceneStage::Done,
        start_frame_artifact: None,
        start_frame_url: Some(format!("https://example.test/{id}.png")),
        end_frame_artifact: None,
        end_frame_url: None,
        job_id: Some(job_id.to_owned()),
        clip_artifact: clip,
        failure: None,
        resubmitted: false,
    }
}

/// Persist a render row exactly as a host would find it after a crash: the
/// public store API only, no driver involved. Returns the render id.
fn seed_render(
    state_path: &std::path::Path,
    ctx: &ToolCallContext,
    plan_json: &[u8],
    status: RenderStatus,
    scenes: Vec<SceneState>,
) -> String {
    let plan_digest = Digest::raw_json(plan_json);
    let render_id: Arc<str> = Arc::from(format!("render-{}", &plan_digest.to_hex()[..24]));
    let state = RenderState {
        tenant_scope: Arc::clone(&ctx.run.locator.tenant_scope),
        render_id: Arc::clone(&render_id),
        plan_json: String::from_utf8(plan_json.to_vec()).expect("utf8"),
        plan_digest,
        status,
        scenes,
        final_artifact: None,
        transcript_srt_artifact: None,
        transcript_vtt_artifact: None,
        failure: None,
        revision: 0,
    };
    SqliteRenderStateStore::open(state_path)
        .expect("open sqlite state store")
        .insert(&state)
        .expect("seed row");
    render_id.to_string()
}

fn load_persisted(state_path: &std::path::Path, render_id: &str) -> RenderState {
    SqliteRenderStateStore::open(state_path)
        .expect("reopen sqlite state store")
        .load("tenant-a", render_id)
        .expect("load")
        .expect("row")
}

// ----- test 4: an uncomposable row fails terminally, never `Advancing` ----

#[tokio::test]
async fn compose_spec_failure_persists_failed_not_advancing() {
    let ctx = tool_context();
    let dir = tempfile::tempdir().expect("tempdir");
    let state_path = dir.path().join("journal.sqlite");
    let store: Arc<dyn ArtifactStore> = Arc::new(InProcessArtifactStore::default());

    // A row a host reconciled by hand: every scene is `Done`, but scene-02
    // lost its clip reference. The compose spec cannot be built from it, and
    // no tool can repair that.
    let plan = two_scene_plan();
    let clip_one = stage(&store, &ctx, "scene-one-clip-bytes").await;
    let render_id = seed_render(
        &state_path,
        &ctx,
        &plan,
        RenderStatus::Running,
        vec![
            done_scene("scene-01", "job-a", Some(clip_one)),
            done_scene("scene-02", "job-b", None),
        ],
    );

    let media = media_toolset();
    let compose = compose_toolset();
    let driver = build_driver(&media, &compose, &state_path, Some(Arc::clone(&store)));
    let error = driver
        .advance(&ctx, &render_id)
        .await
        .expect_err("a finished scene with no clip cannot be composed");
    assert_eq!(error.code(), MEDIA_PIPELINE_STAGE_FAILED);
    assert!(
        error.message().contains("missing its clip artifact"),
        "{}",
        error.message()
    );
    assert!(compose.calls().is_empty(), "nothing was dispatched");
    assert!(media.calls().is_empty());

    // ----- simulated crash, then a cold read of the row -------------------
    drop(driver);
    drop(media);
    drop(compose);
    let persisted = load_persisted(&state_path, &render_id);
    assert_eq!(
        persisted.status,
        RenderStatus::Failed,
        "a pure pre-effect failure is terminal, not an uncertain claim: {persisted:?}"
    );
    let reason = persisted.failure.clone().expect("reason recorded");
    assert!(
        reason.starts_with(MEDIA_PIPELINE_STAGE_FAILED) && reason.contains("missing its clip"),
        "{reason}"
    );
    assert_eq!(
        persisted.revision, 4,
        "claim write, one progress save per scene, then the terminal write"
    );

    // A terminal render is returned as-is; no second attempt is made.
    let fresh_media = media_toolset();
    let fresh_compose = compose_toolset();
    let fresh_driver = build_driver(
        &fresh_media,
        &fresh_compose,
        &state_path,
        Some(Arc::clone(&store)),
    );
    let again = fresh_driver
        .advance(&ctx, &render_id)
        .await
        .expect("a failed render reads back without error");
    assert_eq!(again.status, RenderStatus::Failed);
    assert_eq!(again.failure, persisted.failure);
    assert_eq!(again.revision, persisted.revision, "nothing was rewritten");
    assert!(fresh_compose.calls().is_empty());
    assert!(fresh_media.calls().is_empty());
}

// ----- test 5: a transcript staging failure releases the claim -----------

#[tokio::test]
async fn transcript_staging_failure_restores_previous_status() {
    let ctx = tool_context();
    let dir = tempfile::tempdir().expect("tempdir");
    let state_path = dir.path().join("journal.sqlite");
    let flaky = Arc::new(FailOnceStore::new());
    let store: Arc<dyn ArtifactStore> = flaky.clone();

    let plan = captioned_two_scene_plan();
    let clip_one = stage(&store, &ctx, "scene-one-clip-bytes").await;
    let clip_two = stage(&store, &ctx, "scene-two-clip-bytes").await;
    let render_id = seed_render(
        &state_path,
        &ctx,
        &plan,
        RenderStatus::Running,
        vec![
            done_scene("scene-01", "job-a", Some(clip_one.clone())),
            done_scene("scene-02", "job-b", Some(clip_two.clone())),
        ],
    );

    // The artifact backend drops the first transcript write. This happens
    // before any paid call, so the tick must release its claim.
    let media = media_toolset();
    let compose = compose_toolset();
    let driver = build_driver(&media, &compose, &state_path, Some(Arc::clone(&store)));
    flaky.fail_next_put();
    let error = driver
        .advance(&ctx, &render_id)
        .await
        .expect_err("the transcript could not be staged");
    assert_eq!(error.code(), MEDIA_PIPELINE_STAGE_FAILED);
    assert!(
        error
            .message()
            .contains("transcript artifact could not be staged"),
        "{}",
        error.message()
    );
    assert!(compose.calls().is_empty(), "compose was never dispatched");

    // ----- simulated crash, then a cold read of the row -------------------
    drop(driver);
    drop(media);
    drop(compose);
    let persisted = load_persisted(&state_path, &render_id);
    assert_eq!(
        persisted.status,
        RenderStatus::Running,
        "the pre-claim status is restored, not `Advancing`: {persisted:?}"
    );
    assert_eq!(persisted.failure, None);
    assert!(persisted.transcript_srt_artifact.is_none());
    assert!(persisted.transcript_vtt_artifact.is_none());
    assert_eq!(
        persisted.revision, 4,
        "claim write, one progress save per scene, then the release write"
    );

    // The next tick over the same file retries from scratch and completes.
    let fresh_media = media_toolset();
    let fresh_compose = compose_toolset();
    let fresh_driver = build_driver(
        &fresh_media,
        &fresh_compose,
        &state_path,
        Some(Arc::clone(&store)),
    );
    let final_clip = stage(&store, &ctx, "final-movie-bytes").await;
    fresh_compose.push(
        COMPOSE_TOOL,
        json!({ "artifact": final_clip, "duration_s": 12.0, "byte_length": 30 }),
    );
    let state = fresh_driver
        .advance(&ctx, &render_id)
        .await
        .expect("retry tick");
    assert_eq!(state.status, RenderStatus::Completed, "{state:?}");
    assert_eq!(state.final_artifact.as_ref(), Some(&final_clip));
    assert!(state.transcript_srt_artifact.is_some());
    assert!(state.transcript_vtt_artifact.is_some());
    assert_eq!(state.scenes[0].clip_artifact.as_ref(), Some(&clip_one));
    assert_eq!(state.scenes[1].clip_artifact.as_ref(), Some(&clip_two));
    let compose_calls = fresh_compose.calls();
    assert_eq!(compose_calls.len(), 1);
    assert_eq!(compose_calls[0].1["subtitles"]["mode"], "burn_in");
    assert!(fresh_media.calls().is_empty());
}

// ----- test 6: the host abandons an interrupted claim ----------------------

#[tokio::test]
async fn release_claim_moves_advancing_to_failed() {
    let ctx = tool_context();
    let dir = tempfile::tempdir().expect("tempdir");
    let state_path = dir.path().join("journal.sqlite");

    // A claim left behind by a process that died mid-dispatch.
    let plan = two_scene_plan();
    let polling = |id: &str, job: &str| SceneState {
        stage: SceneStage::Polling,
        clip_artifact: None,
        ..done_scene(id, job, None)
    };
    let render_id = seed_render(
        &state_path,
        &ctx,
        &plan,
        RenderStatus::Advancing,
        vec![polling("scene-01", "job-a"), polling("scene-02", "job-b")],
    );

    let media = media_toolset();
    let compose = compose_toolset();
    let driver = build_driver(&media, &compose, &state_path, None);

    // Ticking an interrupted claim only reads it back.
    let uncertain = driver.advance(&ctx, &render_id).await.expect("read-only");
    assert_eq!(uncertain.status, RenderStatus::Advancing);
    assert!(media.calls().is_empty());

    let reason = "host reconciled: provider jobs job-a and job-b were cancelled";
    let released = driver
        .release_claim("tenant-a", &render_id, reason)
        .await
        .expect("release");
    assert_eq!(released.status, RenderStatus::Failed);
    assert_eq!(released.failure.as_deref(), Some(reason));
    assert_eq!(released.revision, 1);
    assert_eq!(
        released.scenes[0].job_id.as_deref(),
        Some("job-a"),
        "the scene record is kept for the audit trail"
    );

    // ----- simulated crash, then a cold read of the row -------------------
    drop(driver);
    drop(media);
    drop(compose);
    let persisted = load_persisted(&state_path, &render_id);
    assert_eq!(persisted.status, RenderStatus::Failed);
    assert_eq!(persisted.failure.as_deref(), Some(reason));
    assert_eq!(persisted.revision, 1);

    let fresh_media = media_toolset();
    let fresh_compose = compose_toolset();
    let fresh_driver = build_driver(&fresh_media, &fresh_compose, &state_path, None);
    let terminal = fresh_driver
        .advance(&ctx, &render_id)
        .await
        .expect("terminal read");
    assert_eq!(terminal.status, RenderStatus::Failed);
    assert!(
        fresh_media.calls().is_empty(),
        "a failed render is never re-driven"
    );

    // The CAS precondition is `Advancing`: a second release is refused and
    // the row is untouched.
    let refused = fresh_driver
        .release_claim("tenant-a", &render_id, "second attempt")
        .await
        .expect_err("no claim is held");
    assert_eq!(refused.code(), MEDIA_PIPELINE_INVALID_ARGUMENTS);
    assert_eq!(load_persisted(&state_path, &render_id).revision, 1);

    // The reason is bounded and required.
    let empty = fresh_driver
        .release_claim("tenant-a", &render_id, "")
        .await
        .expect_err("an empty reason is rejected");
    assert_eq!(empty.code(), MEDIA_PIPELINE_INVALID_ARGUMENTS);
    let unknown = fresh_driver
        .release_claim("tenant-a", "render-000000000000000000000000", reason)
        .await
        .expect_err("unknown render");
    assert_eq!(
        unknown.code(),
        finstack_ai_workflow_media_pipeline::MEDIA_PIPELINE_NOT_FOUND
    );
}
