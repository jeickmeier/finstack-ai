//! Durable media tick ownership, progress, and interruption regressions.

use finstack_ai_kernel::*;
use finstack_ai_runtime::ports::{PortFuture, model::*, tool::*};
use finstack_ai_workflow_media_pipeline::*;
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

struct PausedToolset {
    calls: Arc<AtomicUsize>,
    started: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}
impl Toolset for PausedToolset {
    fn descriptor(&self) -> ToolsetDescriptor {
        ToolsetDescriptor {
            name: Arc::from("test"),
            metadata: Metadata::empty(),
        }
    }
    fn tools(&self) -> Arc<[ToolSpec]> {
        Arc::from([spec_for("openrouter_generate_video")])
    }
    fn call(
        &self,
        _: ToolCallContext,
        _: ValidatedToolCall,
    ) -> PortFuture<Result<ToolEventStream, ToolError>> {
        let started = self.started.clone();
        let release = self.release.clone();
        let num = self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            started.notify_one();
            release.notified().await;
            let result = ToolResult {
                output: RawJson::parse(
                    serde_json::to_vec(&json!({"id": format!("job-{num}")})).unwrap(),
                )
                .unwrap(),
                is_error: false,
            };
            Ok(
                Box::pin(futures_util::stream::iter([Ok(ToolStreamItem::Completed(
                    result,
                ))])) as ToolEventStream,
            )
        })
    }
}

#[tokio::test]
async fn concurrent_and_interrupted_ticks_never_resubmit() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("render.sqlite");
    let tools = Arc::new(PausedToolset {
        calls: Arc::new(AtomicUsize::new(0)),
        started: Arc::new(tokio::sync::Notify::new()),
        release: Arc::new(tokio::sync::Notify::new()),
    });
    let driver = |store: Arc<dyn RenderStateStore>| {
        MediaPipelineDriver::try_new(MediaPipelineConfig {
            media_tools: tools.clone(),
            compose_tools: tools.clone(),
            state: store,
            artifact_store: None,
            limits: PlanLimits {
                max_scenes: 2,
                max_total_video_s: 12,
                max_concurrent_jobs: 2,
            },
        })
        .unwrap()
    };
    let first = driver(Arc::new(SqliteRenderStateStore::open(&path).unwrap()));
    let second = driver(Arc::new(SqliteRenderStateStore::open(&path).unwrap()));
    let ctx = tool_context();
    let initial = first.submit_plan(&ctx, &url_only_plan(2)).await.unwrap();
    // Pause the second submission after the first job ID is durably saved.
    let mut tick = Box::pin(first.advance(&ctx, &initial.render_id));
    tokio::select! { biased; () = tools.started.notified() => {}, _ = &mut tick => panic!("unexpected completion"), }
    let concurrent = second.advance(&ctx, &initial.render_id).await.unwrap();
    assert_eq!(concurrent.status, RenderStatus::Advancing);
    assert_eq!(tools.calls.load(Ordering::SeqCst), 1);
    tools.release.notify_one();
    tokio::select! { biased; () = tools.started.notified() => {}, _ = &mut tick => panic!("unexpected completion"), }
    let progress = second.status("tenant-a", &initial.render_id).unwrap();
    assert_eq!(progress.scenes[0].job_id.as_deref(), Some("job-0"));
    assert_eq!(progress.scenes[1].job_id, None);
    drop(tick);
    drop(first);
    drop(second);
    let restarted = driver(Arc::new(SqliteRenderStateStore::open(&path).unwrap()));
    let uncertain = restarted.advance(&ctx, &initial.render_id).await.unwrap();
    assert_eq!(uncertain.status, RenderStatus::Advancing);
    assert_eq!(tools.calls.load(Ordering::SeqCst), 2);
    assert_eq!(uncertain.scenes[0].job_id.as_deref(), Some("job-0"));
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

fn url_only_plan(scene_count: usize) -> Vec<u8> {
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

struct FailingUpdates {
    inner: MemoryRenderStateStore,
    updates: AtomicUsize,
    fail_at: usize,
}
impl RenderStateStore for FailingUpdates {
    fn insert(&self, state: &RenderState) -> Result<(), StateError> {
        self.inner.insert(state)
    }
    fn load(&self, tenant: &str, render: &str) -> Result<Option<RenderState>, StateError> {
        self.inner.load(tenant, render)
    }
    fn update(&self, state: &RenderState) -> Result<bool, StateError> {
        if self.updates.fetch_add(1, Ordering::SeqCst) + 1 == self.fail_at {
            Err(StateError::Unavailable {
                code: "injected_write_failure",
            })
        } else {
            self.inner.update(state)
        }
    }
}

#[tokio::test]
async fn claim_and_progress_write_failures_do_not_repeat_effects() {
    for fail_at in [1, 2] {
        let tools = Arc::new(PausedToolset {
            calls: Arc::new(AtomicUsize::new(0)),
            started: Arc::new(tokio::sync::Notify::new()),
            release: Arc::new(tokio::sync::Notify::new()),
        });
        tools.release.notify_one();
        let state = Arc::new(FailingUpdates {
            inner: MemoryRenderStateStore::new(),
            updates: AtomicUsize::new(0),
            fail_at,
        });
        let driver = MediaPipelineDriver::try_new(MediaPipelineConfig {
            media_tools: tools.clone(),
            compose_tools: tools.clone(),
            state: state.clone(),
            artifact_store: None,
            limits: PlanLimits {
                max_scenes: 1,
                max_total_video_s: 6,
                max_concurrent_jobs: 1,
            },
        })
        .unwrap();
        let ctx = tool_context();
        let initial = driver.submit_plan(&ctx, &url_only_plan(1)).await.unwrap();
        assert_eq!(
            driver
                .advance(&ctx, &initial.render_id)
                .await
                .unwrap_err()
                .code(),
            MEDIA_PIPELINE_STORE_FAILURE
        );
        let saved = state.load("tenant-a", &initial.render_id).unwrap().unwrap();
        if fail_at == 1 {
            assert_eq!(saved.status, RenderStatus::Running);
            assert_eq!(tools.calls.load(Ordering::SeqCst), 0);
        } else {
            assert_eq!(saved.status, RenderStatus::Advancing);
            assert_eq!(saved.scenes[0].job_id, None);
            assert_eq!(
                driver
                    .advance(&ctx, &initial.render_id)
                    .await
                    .unwrap()
                    .status,
                RenderStatus::Advancing
            );
            assert_eq!(tools.calls.load(Ordering::SeqCst), 1);
        }
    }
}
