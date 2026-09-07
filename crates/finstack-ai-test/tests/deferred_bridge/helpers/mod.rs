//! Shared setup for deferred child-run bridge integration tests.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use finstack_ai::{
    Agent, AgentRun, AgentRunRequest, ChildEventContext, ChildEventSink, ChildPlanContext,
    ChildRunBridgeError, ChildRunResolver, DeferredChildPlanner, DeferredPlanError,
};
use finstack_ai_kernel::{
    AgentId, AuthorizationEvidence, BundleId, ChildPlacement, ChildRunLocator, ComponentId,
    ComponentRef, ContentBlock, Digest, EffectId, ExternalEffectCompletion,
    ExternalEffectCompletionCommand, ExternalEffectOutcome, ExternalHandleRef, OperationLocator,
    PrincipalRef, RawJson, ReconciliationPolicy, RunPhase, RunSecurityContext, TextBlock,
    ToolExecutionMode, ToolId, Version,
};
use finstack_ai_kernel::{BudgetRequest, Metadata, RetrySafety};
use finstack_ai_runtime::child::{
    AgentInvokeError, AgentInvoker, AgentRef, ChildRunContext, ChildRunHandle, ChildRunRequest,
    child_relation_digest,
};
use finstack_ai_runtime::commit::CommitCoordinator;
use finstack_ai_runtime::events::EventBatch;
use finstack_ai_runtime::ports::PortFuture;
use finstack_ai_runtime::ports::journal::JournalStore;
use finstack_ai_runtime::ports::model::{
    ApprovalMetadata, ApprovalRequirement, Model, ModelContextProfile, ModelName, ModelResponse,
    ModelStreamItem, ModelToolCall, SideEffectClass, TokenEstimatorRef, TokenEstimatorSource,
    ToolCallDelta, ToolDeferralSupport, ToolSpec,
};
use finstack_ai_runtime::ports::tool::{ToolDeferral, ToolStreamItem, Toolset};
use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};
use finstack_ai_test::{
    ScriptedModel, ScriptedModelAction, ScriptedModelPlan, ScriptedToolAction, ScriptedToolPlan,
    ScriptedToolset,
};

pub(crate) const VERSION: Version = Version {
    major: 0,
    minor: 0,
    patch: 1,
};

pub(crate) fn profile() -> ModelContextProfile {
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

pub(crate) fn completed(text: &str) -> ScriptedModelPlan {
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

pub(crate) fn echo_tool_call() -> ScriptedModelPlan {
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

pub(crate) fn echo_tool_spec() -> ToolSpec {
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

pub(crate) fn security() -> RunSecurityContext {
    RunSecurityContext::try_new(
        "tenant-preview",
        PrincipalRef::try_new("preview-tests", "developer", Some("tenant-preview"))
            .expect("principal"),
        "local",
        "test",
        "preview-policy-v1",
        "preview-decision-v1",
        None,
    )
    .expect("security")
}

pub(crate) fn agent_request(input: &str) -> AgentRunRequest {
    AgentRunRequest::try_new(
        ModelName::try_new("preview-1").expect("model name"),
        input,
        security(),
    )
    .expect("request")
}

pub(crate) fn memory_store() -> Arc<dyn JournalStore> {
    Arc::new(
        MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions: 8,
            batches_per_session: 256,
            records_per_session: 1024,
            snapshot_bytes: 64 * 1024,
        })
        .expect("store"),
    )
}

pub(crate) struct RecordingInvoker {
    child_agent: Agent,
    children: Mutex<BTreeMap<finstack_ai_kernel::RunId, AgentRun>>,
}

impl RecordingInvoker {
    pub(crate) fn new(child_agent: Agent) -> Self {
        Self {
            child_agent,
            children: Mutex::new(BTreeMap::new()),
        }
    }
}

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
        let child = match self.child_agent.start(agent_request("child work")) {
            Ok(child) => child,
            Err(error) => {
                return Box::pin(async move {
                    Err(AgentInvokeError::Unavailable {
                        message: Arc::from(error.to_string()),
                    })
                });
            }
        };
        if let Ok(mut children) = self.children.lock() {
            children.insert(request.locator().operation.run_id, child);
        }
        let locator = request.locator().clone();
        Box::pin(async move {
            Ok(ChildRunHandle {
                locator,
                relation_digest,
            })
        })
    }
}

impl ChildRunResolver for RecordingInvoker {
    fn resolve(
        &self,
        child: &ChildRunLocator,
    ) -> PortFuture<Result<AgentRun, ChildRunBridgeError>> {
        let found = self
            .children
            .lock()
            .ok()
            .and_then(|children| children.get(&child.operation.run_id).cloned());
        Box::pin(async move {
            found.ok_or_else(|| ChildRunBridgeError::Failed {
                message: "child run was not started".to_owned(),
            })
        })
    }
}

pub(crate) struct ClaimingPlanner {
    request: ChildRunRequest,
}

impl ClaimingPlanner {
    pub(crate) fn new(request: ChildRunRequest) -> Self {
        Self { request }
    }
}

impl DeferredChildPlanner for ClaimingPlanner {
    fn plan(
        &self,
        _context: &ChildPlanContext,
    ) -> PortFuture<Result<Option<ChildRunRequest>, DeferredPlanError>> {
        let request = self.request.clone();
        Box::pin(async move { Ok(Some(request)) })
    }
}

pub(crate) struct PanickingSink;

impl ChildEventSink for PanickingSink {
    fn on_batch(
        &self,
        _context: &ChildEventContext,
        _batch: &EventBatch,
    ) -> PortFuture<Result<(), ()>> {
        Box::pin(async { panic!("child event sink panicked") })
    }
}

pub(crate) struct BlockingSink;

impl ChildEventSink for BlockingSink {
    fn on_batch(
        &self,
        _context: &ChildEventContext,
        _batch: &EventBatch,
    ) -> PortFuture<Result<(), ()>> {
        Box::pin(async {
            std::future::pending::<()>().await;
            Ok(())
        })
    }
}

pub(crate) async fn deferred_tool_parent() -> (Agent, AgentRun, Arc<dyn JournalStore>, EffectId) {
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
    let model: Arc<dyn Model> = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![echo_tool_call(), completed("unused parent retry")],
    ));
    let store = memory_store();
    let agent = Agent::builder(
        AgentId::parse("test.agent.deferred-bridge").expect("agent"),
        BundleId::parse("test.bundle.deferred-bridge").expect("bundle"),
        (
            ComponentRef::new(
                ComponentId::parse("test.model.deferred-bridge").expect("model"),
                Some(VERSION),
            ),
            model,
        ),
        (
            ComponentRef::new(
                ComponentId::parse("test.store.deferred-bridge").expect("store"),
                Some(VERSION),
            ),
            Arc::clone(&store),
        ),
    )
    .policy(finstack_ai::RunPolicy {
        child_runs: finstack_ai::ChildRunPolicy::Allow { max_depth: 1 },
        ..finstack_ai::RunPolicy::default()
    })
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
        .start(agent_request("defer the tool"))
        .expect("parent start");
    let effect_id = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Ok(commit) =
                CommitCoordinator::recover(Arc::clone(&store), parent.locator().session_id).await
                && commit.state().phase() == Some(RunPhase::AwaitingExternal)
                && let Some(batch) = commit.state().active_tool_batch()
                && let Some(call) = batch.calls.first()
                && let finstack_ai_kernel::ActiveToolCallStatus::Requested {
                    deferred: Some(deferred),
                    ..
                } = &call.status
            {
                return deferred.effect_id;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("deferred tool effect");
    (agent, parent, store, effect_id)
}

pub(crate) async fn child_agent(store: Arc<dyn JournalStore>) -> Agent {
    let model: Arc<dyn Model> = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![completed("child done")],
    ));
    Agent::builder(
        AgentId::parse("test.agent.deferred-bridge-child").expect("agent"),
        BundleId::parse("test.bundle.deferred-bridge-child").expect("bundle"),
        (
            ComponentRef::new(
                ComponentId::parse("test.model.deferred-bridge-child").expect("model"),
                Some(VERSION),
            ),
            model,
        ),
        (
            ComponentRef::new(
                ComponentId::parse("test.store.deferred-bridge-child").expect("store"),
                Some(VERSION),
            ),
            store,
        ),
    )
    .build()
    .await
    .expect("child agent")
}

pub(crate) async fn isolated_child_request(
    store: Arc<dyn JournalStore>,
    parent: &AgentRun,
) -> ChildRunRequest {
    let session = finstack_ai::Session::create(store, Arc::clone(&parent.locator().tenant_scope))
        .await
        .expect("child session");
    let lane = session.lane("main").await.expect("child lane");
    let run_id = finstack_ai_kernel::RunId::parse("01234567-89ab-7cde-89ab-0123456789ad")
        .expect("child run");
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
        ChildRunLocator {
            operation: OperationLocator::try_new(
                parent.locator().tenant_scope.as_ref(),
                session.session_id(),
                lane.lane_id(),
                run_id,
            )
            .expect("child locator"),
            remote: None,
        },
        None,
        BudgetRequest::default(),
        None,
        Metadata::empty(),
    )
    .expect("valid child request")
}

pub(crate) fn failed_command(
    parent: &AgentRun,
    effect_id: EffectId,
    completion_id: &str,
    principal: PrincipalRef,
) -> ExternalEffectCompletionCommand {
    ExternalEffectCompletionCommand::try_new(
        parent.locator().clone(),
        principal,
        AuthorizationEvidence::try_new(
            security().authorization_policy_version(),
            security().authorization_decision_id(),
        )
        .expect("auth"),
        ExternalEffectCompletion::try_new(
            effect_id,
            completion_id,
            ExternalEffectOutcome::Failed {
                error: finstack_ai_kernel::ErrorDescriptor::new(
                    "provider_failed",
                    "provider failed",
                    finstack_ai_kernel::ErrorCategory::Model,
                    true,
                )
                .expect("error"),
            },
        )
        .expect("completion"),
    )
    .expect("command")
}
