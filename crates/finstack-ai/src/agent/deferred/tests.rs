use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use finstack_ai_kernel::{
    AgentId, BundleId, ChildPlacement, ChildRunLocator, ComponentId, ComponentRef, ContentBlock,
    Digest, EffectDeferred, EffectId, EffectOutputContract, EffectOutputKind, ExternalHandleRef,
    OperationLocator, RawJson, ReconciliationPolicy, RunPhase, RunSecurityContext, TextBlock,
    ToolExecutionMode, ToolId, Version,
};
use finstack_ai_kernel::{BudgetRequest, Metadata, RetrySafety};
use finstack_ai_runtime::child::{
    AgentInvokeError, AgentInvoker, AgentRef, ChildRunContext, ChildRunHandle, ChildRunRequest,
    child_relation_digest,
};
use finstack_ai_runtime::commit::CommitCoordinator;
use finstack_ai_runtime::ports::PortFuture;
use finstack_ai_runtime::ports::journal::JournalStore;
use finstack_ai_runtime::ports::model::{
    ApprovalMetadata, ApprovalRequirement, ModelContextProfile, ModelName, ModelResponse,
    ModelStreamItem, ModelToolCall, SideEffectClass, TokenEstimatorRef, TokenEstimatorSource,
    ToolCallDelta, ToolDeferralSupport, ToolSpec,
};
use finstack_ai_runtime::ports::tool::{ToolDeferral, ToolStreamItem, Toolset};
use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};
use finstack_ai_test::{
    ScriptedModel, ScriptedModelAction, ScriptedModelPlan, ScriptedToolAction, ScriptedToolPlan,
    ScriptedToolset,
};

use super::{
    ChildPlanContext, ChildRunBridge, ChildRunBridgeError, ChildRunResolver, ChildSettleOutcome,
    DeferredChildPlanner, DeferredPlanError, outstanding_deferrals,
};
use crate::{Agent, AgentRun, AgentRunRequest};

struct UnownedPlanner;

impl DeferredChildPlanner for UnownedPlanner {
    fn plan(
        &self,
        _context: &ChildPlanContext,
    ) -> PortFuture<Result<Option<ChildRunRequest>, DeferredPlanError>> {
        Box::pin(async { Ok(None) })
    }
}

fn plan_context() -> ChildPlanContext {
    let effect_id = EffectId::from_bytes([
        0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x70, 0x00, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x2c,
    ]);
    ChildPlanContext {
        parent: OperationLocator::try_new(
            "tenant-preview",
            finstack_ai_kernel::SessionId::from_bytes([
                0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x70, 0x00, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x01,
            ]),
            finstack_ai_kernel::LaneId::from_bytes([
                0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x70, 0x00, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x02,
            ]),
            finstack_ai_kernel::RunId::from_bytes([
                0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x70, 0x00, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x03,
            ]),
        )
        .expect("parent locator"),
        deferred: EffectDeferred {
            effect_id,
            handle: ExternalHandleRef::try_new(
                ComponentId::parse("finstack.tool.scripted").expect("component"),
                "unowned",
                RawJson::parse(b"{}").expect("metadata"),
            )
            .expect("handle"),
            reconciliation: ReconciliationPolicy::CallbackOnly,
            next_poll_at: None,
            expires_at: None,
            output_contract: EffectOutputContract {
                kind: EffectOutputKind::ToolResult,
                schema_version: 1,
                schema_digest: Digest::raw_json(b"tool-result"),
            },
        },
    }
}

#[tokio::test]
async fn planner_may_return_none_for_an_unowned_handle() {
    let planner = UnownedPlanner;
    let claim = planner
        .plan(&plan_context())
        .await
        .expect("unowned planner stays available");
    assert_eq!(claim, None);
    let _resolver: Option<Arc<dyn super::ChildRunResolver>> = None;
}

const VERSION: Version = Version {
    major: 0,
    minor: 0,
    patch: 1,
};

struct CountingPlanner {
    hits: AtomicUsize,
    claim: Option<ChildRunRequest>,
}

impl DeferredChildPlanner for CountingPlanner {
    fn plan(
        &self,
        _context: &ChildPlanContext,
    ) -> PortFuture<Result<Option<ChildRunRequest>, DeferredPlanError>> {
        self.hits.fetch_add(1, Ordering::SeqCst);
        let claim = self.claim.clone();
        Box::pin(async move { Ok(claim) })
    }
}

struct RecordingInvoker;

impl AgentInvoker for RecordingInvoker {
    fn start_or_attach(
        &self,
        context: ChildRunContext,
        request: ChildRunRequest,
    ) -> PortFuture<Result<ChildRunHandle, AgentInvokeError>> {
        let relation_digest = match child_relation_digest(&context, &request) {
            Ok(digest) => digest,
            Err(error) => {
                return Box::pin(async move {
                    Err(AgentInvokeError::InvalidRequest {
                        message: Arc::from(error.to_string()),
                    })
                });
            }
        };
        let locator = request.locator().clone();
        Box::pin(async move {
            Ok(ChildRunHandle {
                locator,
                relation_digest,
            })
        })
    }
}

struct FailingResolver;

impl ChildRunResolver for FailingResolver {
    fn resolve(
        &self,
        _child: &ChildRunLocator,
    ) -> PortFuture<Result<AgentRun, ChildRunBridgeError>> {
        Box::pin(async {
            Err(ChildRunBridgeError::Failed {
                message: "resolver unused in unclaimed path".to_owned(),
            })
        })
    }
}

fn profile() -> ModelContextProfile {
    ModelContextProfile {
        provider: Arc::from("scripted"),
        model: ModelName::try_new("preview-1").expect("model name"),
        hard_input_bytes: 1_048_576,
        context_window_tokens: 1_048_576,
        max_output_tokens: 256,
        reserved_output_tokens: 256,
        provider_overhead_tokens: 32,
        estimator: TokenEstimatorRef {
            id: Arc::from("bytes-upper-bound"),
            version: Arc::from("1"),
            source: TokenEstimatorSource::ConservativeUpperBound,
        },
    }
}

fn completed(text: &str) -> ScriptedModelPlan {
    ScriptedModelPlan {
        actions: vec![
            ScriptedModelAction::Emit(Ok(ModelStreamItem::TextDelta(
                finstack_ai_runtime::ports::model::TextDelta {
                    text: Arc::from(text),
                },
            ))),
            ScriptedModelAction::Emit(Ok(ModelStreamItem::Completed(ModelResponse {
                assistant_content: Arc::from([ContentBlock::Text(
                    TextBlock::try_new(text).expect("assistant text"),
                )]),
                tool_calls: Arc::from([]),
                usage: finstack_ai_kernel::Usage::empty(),
                provider_ids: finstack_ai_kernel::ProviderIds::empty(),
                completion_id: Arc::from("preview-completion"),
                continuation_state: None,
            }))),
        ],
    }
}

fn security() -> RunSecurityContext {
    RunSecurityContext::try_new(
        "tenant-preview",
        finstack_ai_kernel::PrincipalRef::try_new(
            "preview-tests",
            "developer",
            Some("tenant-preview"),
        )
        .expect("principal"),
        "local",
        "test",
        "preview-policy-v1",
        "preview-decision-v1",
        None,
    )
    .expect("security")
}

fn request(input: &str) -> AgentRunRequest {
    AgentRunRequest::try_new(
        ModelName::try_new("preview-1").expect("model name"),
        input,
        security(),
    )
    .expect("request")
}

async fn preview_parent() -> (AgentRun, Arc<dyn JournalStore>) {
    let model: Arc<dyn finstack_ai_runtime::ports::model::Model> = Arc::new(
        ScriptedModel::from_plans(profile(), vec![completed("parent")]),
    );
    let store: Arc<dyn JournalStore> = Arc::new(
        MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions: 4,
            batches_per_session: 64,
            records_per_session: 512,
            snapshot_bytes: 4_096,
        })
        .expect("store"),
    );
    let agent = Agent::builder(
        AgentId::parse("test.agent.deferred").expect("agent"),
        BundleId::parse("test.bundle.deferred").expect("bundle"),
        (
            ComponentRef::new(
                ComponentId::parse("test.model.deferred").expect("model"),
                Some(VERSION),
            ),
            model,
        ),
        (
            ComponentRef::new(
                ComponentId::parse("test.store.deferred").expect("store"),
                Some(VERSION),
            ),
            Arc::clone(&store),
        ),
    )
    .build()
    .await
    .expect("agent");
    let parent = agent.start(request("parent work")).expect("parent start");
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Ok(commit) =
                CommitCoordinator::recover(Arc::clone(&store), parent.locator().session_id).await
                && commit
                    .state()
                    .accepted()
                    .is_some_and(|accepted| accepted.run_id() == parent.locator().run_id)
            {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("parent accepted");
    (parent, store)
}

async fn isolated_child_request(
    store: Arc<dyn JournalStore>,
    parent: &AgentRun,
) -> ChildRunRequest {
    let session = crate::Session::create(store, Arc::clone(&parent.locator().tenant_scope))
        .await
        .expect("child session");
    let lane = session.lane("main").await.expect("child lane");
    let run_id = super::super::prepare::NativeIds::generate::<finstack_ai_kernel::RunTag>()
        .expect("child run");
    let locator = ChildRunLocator {
        operation: OperationLocator::try_new(
            parent.locator().tenant_scope.as_ref(),
            session.session_id(),
            lane.lane_id(),
            run_id,
        )
        .expect("child locator"),
        remote: None,
    };
    ChildRunRequest::try_new(
        AgentRef {
            id: AgentId::parse("finstack.agent.child").expect("agent"),
            bundle: None,
            spec_digest: Digest::raw_json(br#"{"agent":"child"}"#),
        },
        Arc::from([ContentBlock::Text(
            TextBlock::try_new("work").expect("text"),
        )]),
        ChildPlacement::IsolatedChildSession,
        locator,
        None,
        BudgetRequest::default(),
        None,
        Metadata::empty(),
    )
    .expect("valid child request")
}

#[tokio::test]
async fn settle_returns_unclaimed_when_no_planner_owns_the_handle() {
    let (parent, _store) = preview_parent().await;
    let bridge = ChildRunBridge::new(
        vec![Arc::new(UnownedPlanner)],
        Arc::new(RecordingInvoker),
        Arc::new(FailingResolver),
    );
    let outcome = bridge
        .settle(&parent, &plan_context().deferred)
        .await
        .expect("unclaimed is not an error");
    assert_eq!(outcome, ChildSettleOutcome::Unclaimed);
}

#[tokio::test]
async fn settle_uses_the_first_claiming_planner() {
    let (parent, store) = preview_parent().await;
    let request = isolated_child_request(store, &parent).await;
    let skipped = Arc::new(CountingPlanner {
        hits: AtomicUsize::new(0),
        claim: None,
    });
    let claimed = Arc::new(CountingPlanner {
        hits: AtomicUsize::new(0),
        claim: Some(request),
    });
    let later = Arc::new(CountingPlanner {
        hits: AtomicUsize::new(0),
        claim: Some(isolated_child_request(Arc::clone(parent.journal_store()), &parent).await),
    });
    let bridge = ChildRunBridge::new(
        vec![
            Arc::clone(&skipped) as Arc<dyn DeferredChildPlanner>,
            Arc::clone(&claimed) as Arc<dyn DeferredChildPlanner>,
            Arc::clone(&later) as Arc<dyn DeferredChildPlanner>,
        ],
        Arc::new(RecordingInvoker),
        Arc::new(FailingResolver),
    );
    let error = bridge
        .settle(&parent, &plan_context().deferred)
        .await
        .expect_err("resolver fails after the first claim");
    assert_eq!(error.code(), super::CHILD_RUN_BRIDGE_FAILED);
    assert_eq!(skipped.hits.load(Ordering::SeqCst), 1);
    assert_eq!(claimed.hits.load(Ordering::SeqCst), 1);
    assert_eq!(later.hits.load(Ordering::SeqCst), 0);
}

struct FixedResolver {
    child: AgentRun,
}

impl ChildRunResolver for FixedResolver {
    fn resolve(
        &self,
        _child: &ChildRunLocator,
    ) -> PortFuture<Result<AgentRun, ChildRunBridgeError>> {
        let child = self.child.clone();
        Box::pin(async move { Ok(child) })
    }
}

fn echo_tool_spec() -> ToolSpec {
    ToolSpec {
        id: ToolId::parse("finstack.tools.echo").expect("tool id"),
        model_name: Arc::from("echo"),
        title: Arc::from("echo"),
        description: Arc::from("scripted deterministic tool"),
        input_schema: RawJson::parse(
            br#"{"additionalProperties":false,"properties":{"value":{"type":"integer"}},"required":["value"],"type":"object"}"#,
        )
        .expect("input schema"),
        output_schema: Some(
            RawJson::parse(
                br#"{"additionalProperties":false,"properties":{"ok":{"type":"boolean"},"value":{"type":"integer"}},"required":["ok","value"],"type":"object"}"#,
            )
            .expect("output schema"),
        ),
        execution: ToolExecutionMode::Parallel,
        side_effect: SideEffectClass::ReadOnly,
        retry_safety: RetrySafety::SafeToRetry,
        approval: ApprovalMetadata {
            requirement: ApprovalRequirement::NotRequired,
            reason: None,
            attributes: Metadata::empty(),
        },
        max_result_bytes: 4_096,
        metadata: Metadata::empty(),
        deferral: ToolDeferralSupport::Supported,
    }
}

fn echo_tool_call() -> ScriptedModelPlan {
    let arguments = RawJson::parse(br#"{"value":1}"#).expect("arguments");
    ScriptedModelPlan {
        actions: vec![
            ScriptedModelAction::Emit(Ok(ModelStreamItem::ToolCallDelta(ToolCallDelta {
                index: 0,
                name: Some(Arc::from("echo")),
                arguments_delta: Arc::from(arguments.as_str()),
                provider_call_id: Some(Arc::from("call-echo")),
            }))),
            ScriptedModelAction::Emit(Ok(ModelStreamItem::Completed(ModelResponse {
                assistant_content: Arc::from([]),
                tool_calls: Arc::from([ModelToolCall {
                    name: Arc::from("echo"),
                    arguments,
                    provider_call_id: Some(Arc::from("call-echo")),
                }]),
                usage: finstack_ai_kernel::Usage::empty(),
                provider_ids: finstack_ai_kernel::ProviderIds::empty(),
                completion_id: Arc::from("echo-tool-completion"),
                continuation_state: None,
            }))),
        ],
    }
}

async fn deferred_tool_parent() -> (AgentRun, Arc<dyn JournalStore>) {
    let toolset: Arc<dyn Toolset> = Arc::new(ScriptedToolset::new(
        Arc::from([echo_tool_spec()]),
        vec![ScriptedToolPlan {
            panic_on_call: None,
            actions: vec![ScriptedToolAction::Emit(Ok(ToolStreamItem::Deferred(
                ToolDeferral {
                    handle: ExternalHandleRef::try_new(
                        ComponentId::parse("finstack.tool.scripted").expect("component"),
                        "job-1",
                        RawJson::parse(b"{}").expect("metadata"),
                    )
                    .expect("handle"),
                    reconciliation: ReconciliationPolicy::CallbackOrPoll,
                    next_poll_at: None,
                    expires_at: None,
                },
            )))],
        }],
    ));
    let model: Arc<dyn finstack_ai_runtime::ports::model::Model> =
        Arc::new(ScriptedModel::from_plans(
            profile(),
            vec![echo_tool_call(), completed("unused parent retry")],
        ));
    let store: Arc<dyn JournalStore> = Arc::new(
        MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions: 4,
            batches_per_session: 64,
            records_per_session: 512,
            snapshot_bytes: 4_096,
        })
        .expect("store"),
    );
    let agent = Agent::builder(
        AgentId::parse("test.agent.deferred-tool").expect("agent"),
        BundleId::parse("test.bundle.deferred-tool").expect("bundle"),
        (
            ComponentRef::new(
                ComponentId::parse("test.model.deferred-tool").expect("model"),
                Some(VERSION),
            ),
            model,
        ),
        (
            ComponentRef::new(
                ComponentId::parse("test.store.deferred-tool").expect("store"),
                Some(VERSION),
            ),
            Arc::clone(&store),
        ),
    )
    .toolset(
        ComponentRef::new(
            ComponentId::parse("test.tools.echo").expect("toolset"),
            Some(VERSION),
        ),
        toolset,
    )
    .build()
    .await
    .expect("agent");
    let parent = agent
        .start(request("defer the tool"))
        .expect("parent start");
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Ok(commit) =
                CommitCoordinator::recover(Arc::clone(&store), parent.locator().session_id).await
                && commit.state().phase() == Some(RunPhase::AwaitingExternal)
                && !outstanding_deferrals(commit.state()).is_empty()
            {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("deferred tool");
    (parent, store)
}

async fn completing_child(store: Arc<dyn JournalStore>) -> AgentRun {
    let model: Arc<dyn finstack_ai_runtime::ports::model::Model> = Arc::new(
        ScriptedModel::from_plans(profile(), vec![completed("child done")]),
    );
    let agent = Agent::builder(
        AgentId::parse("test.agent.deferred-child").expect("agent"),
        BundleId::parse("test.bundle.deferred-child").expect("bundle"),
        (
            ComponentRef::new(
                ComponentId::parse("test.model.deferred-child").expect("model"),
                Some(VERSION),
            ),
            model,
        ),
        (
            ComponentRef::new(
                ComponentId::parse("test.store.deferred-child").expect("store"),
                Some(VERSION),
            ),
            store,
        ),
    )
    .build()
    .await
    .expect("child agent");
    agent.start(request("child work")).expect("child start")
}

#[tokio::test]
async fn recover_is_empty_when_no_tool_is_deferred() {
    let (parent, _store) = preview_parent().await;
    let bridge = ChildRunBridge::new(
        vec![Arc::new(UnownedPlanner)],
        Arc::new(RecordingInvoker),
        Arc::new(FailingResolver),
    );
    let outcomes = bridge.recover(&parent).await.expect("recover");
    assert!(outcomes.is_empty());
}

#[tokio::test]
async fn recover_settles_outstanding_deferrals_and_is_idempotent() {
    let (parent, store) = deferred_tool_parent().await;
    let commit = CommitCoordinator::recover(Arc::clone(&store), parent.locator().session_id)
        .await
        .expect("recover state");
    let pending = outstanding_deferrals(commit.state());
    assert_eq!(pending.len(), 1);
    let child = completing_child(Arc::clone(&store)).await;
    tokio::time::timeout(Duration::from_secs(3), child.cancel())
        .await
        .expect("child cancel timeout")
        .ok();
    let child_request = isolated_child_request(Arc::clone(&store), &parent).await;
    let bridge = ChildRunBridge::new(
        vec![Arc::new(CountingPlanner {
            hits: AtomicUsize::new(0),
            claim: Some(child_request),
        })],
        Arc::new(RecordingInvoker),
        Arc::new(FixedResolver { child }),
    );
    let first = bridge.recover(&parent).await.expect("first recover");
    assert_eq!(first, vec![ChildSettleOutcome::Failed]);
    let after = CommitCoordinator::recover(Arc::clone(&store), parent.locator().session_id)
        .await
        .expect("recover after settle");
    assert!(outstanding_deferrals(after.state()).is_empty());
    let second = bridge.recover(&parent).await.expect("second recover");
    assert!(second.is_empty());
}
