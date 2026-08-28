use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use finstack_ai::{
    AGENT_RUN_CANCELLED, AGENT_RUN_INVALID_CONFIGURATION, Agent, AgentRun, InteractionResolution,
    RunPolicy,
};
use finstack_ai_kernel::{
    AgentId, AuthorizationEvidence, BundleId, ComponentId, ComponentRef, ContentBlock,
    EffectOutputContract, EffectOutputKind, InteractionKind, ProviderIds, Usage, Version,
};
use finstack_ai_kernel::{
    ChildRunLocator, ChildRunPrepared, Digest, RawJson, ToolCallBlock, ToolFailurePolicy,
    ValidatedToolCall,
};
use finstack_ai_runtime::child::{AgentInvokeError, AgentInvoker, AgentRef, ChildRunHandle, ChildRunRequest, ChildRunStarter, ChildRunStatus, child_relation_digest};
use finstack_ai_runtime::ports::{PortFuture};
use finstack_ai_runtime::ports::model::{Model, ModelResponse, ModelStreamItem, ModelToolCall, ToolCallDelta};
use finstack_ai_runtime::ports::tool::{ToolStreamItem};
use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};
use finstack_ai_test::{ScriptedModel, ScriptedModelAction};
use finstack_ai_tools_subagent::SubagentToolset;
use futures_util::StreamExt;

const VERSION: Version = Version {
    major: 0,
    minor: 0,
    patch: 1,
};

struct ParentStartInvoker {
    parent: Mutex<Option<AgentRun>>,
    child: Agent,
    wait_for_result: bool,
    started: Arc<Mutex<Vec<AgentRun>>>,
}

impl AgentInvoker for ParentStartInvoker {
    fn start_or_attach(
        &self,
        context: ChildRunContext,
        request: ChildRunRequest,
    ) -> PortFuture<Result<ChildRunHandle, AgentInvokeError>> {
        let parent = self.parent.lock().expect("parent slot").clone();
        let child = self.child.clone();
        let wait_for_result = self.wait_for_result;
        let started = Arc::clone(&self.started);
        Box::pin(async move {
            let parent = parent.ok_or_else(|| AgentInvokeError::Unavailable {
                message: Arc::from("parent run is not attached"),
            })?;
            let input = request
                .input()
                .iter()
                .find_map(|block| match block {
                    ContentBlock::Text(text) => Some(text.text().to_string()),
                    _ => None,
                })
                .unwrap_or_default();
            let prepared = ChildRunPrepared {
                parent_run_id: context.parent.run_id,
                parent_effect_id: context.parent_effect_id,
                child: request.locator().clone(),
                request_digest: request.request_digest(),
                placement: request.placement(),
                budget_reservation_id: None,
            };
            let run = parent
                .accept_child(&prepared, &child, agent_request(&input))
                .await
                .map_err(|error| AgentInvokeError::InvalidRequest {
                    message: Arc::from(error.to_string()),
                })?;
            started.lock().expect("started").push(run.clone());
            if wait_for_result {
                let _ = Box::pin(run.result()).await;
            }
            let relation_digest = child_relation_digest(&context, &request).map_err(|error| {
                AgentInvokeError::InvalidRequest {
                    message: Arc::from(error.to_string()),
                }
            })?;
            Ok(ChildRunHandle {
                locator: request.locator().clone(),
                relation_digest,
            })
        })
    }

    fn cancel(&self, locator: &ChildRunLocator) -> PortFuture<Result<(), AgentInvokeError>> {
        let started = Arc::clone(&self.started);
        let locator = locator.clone();
        Box::pin(async move {
            let run = started
                .lock()
                .expect("started")
                .iter()
                .find(|run| run.locator() == &locator.operation)
                .cloned()
                .ok_or_else(|| AgentInvokeError::Unavailable {
                    message: Arc::from("started child is not attached"),
                })?;
            run.cancel()
                .await
                .map_err(|error| AgentInvokeError::InvalidRequest {
                    message: Arc::from(error.to_string()),
                })
        })
    }

    fn status(
        &self,
        locator: &ChildRunLocator,
    ) -> PortFuture<Result<ChildRunStatus, AgentInvokeError>> {
        let started = Arc::clone(&self.started);
        let locator = locator.clone();
        Box::pin(async move {
            started
                .lock()
                .expect("started")
                .iter()
                .any(|run| run.locator() == &locator.operation)
                .then_some(ChildRunStatus::Completed)
                .ok_or_else(|| AgentInvokeError::Unavailable {
                    message: Arc::from("started child is not attached"),
                })
        })
    }
}

fn memory_journal() -> Arc<MemoryJournalStore> {
    Arc::new(
        MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions: 8,
            batches_per_session: 64,
            records_per_session: 512,
            snapshot_bytes: 8_192,
        })
        .expect("store"),
    )
}

async fn build_agent(
    id: &str,
    model: Arc<ScriptedModel>,
    store: Arc<dyn JournalStore>,
    child_runs: ChildRunPolicy,
) -> Agent {
    Agent::builder(
        AgentId::parse(id).expect("agent"),
        BundleId::parse("test.bundle.subagent").expect("bundle"),
        (
            ComponentRef::new(
                ComponentId::parse(format!("test.model.{id}")).expect("model"),
                Some(VERSION),
            ),
            model as Arc<dyn Model>,
        ),
        (
            ComponentRef::new(
                ComponentId::parse("test.store.subagent").expect("store"),
                Some(VERSION),
            ),
            store,
        ),
    )
    .policy(RunPolicy {
        child_runs,
        ..RunPolicy::default()
    })
    .build()
    .await
    .expect("agent")
}

fn child_ref() -> AgentRef {
    AgentRef {
        id: AgentId::parse("test.agent.subagent-child").expect("agent"),
        bundle: None,
        spec_digest: Digest::raw_json(br#"{"agent":"subagent-child"}"#),
    }
}

fn tool_context(parent: &AgentRun) -> finstack_ai_runtime::ports::tool::ToolCallContext {
    finstack_ai_runtime::ports::tool::ToolCallContext {
        run: finstack_ai_runtime::ports::model::RunCallContext {
            locator: parent.locator().clone(),
            authorization: finstack_ai_runtime::ports::model::AuthorizationContext {
                principal: security("decision-v1").principal().clone(),
                authentication_method: Arc::from("oidc"),
                assurance_level: Arc::from("high"),
                roles: Arc::from([]),
                permitted_scopes: Arc::from([]),
                safe_claims: Metadata::empty(),
                policy_version: Arc::from("policy-v1"),
                decision_id: Arc::from("decision-v1"),
            },
            effect_id: finstack_ai_kernel::EffectId::from_bytes([7; 16]),
            attempt: 1,
            deadline: None,
            budget_scope_id: None,
            cancellation: finstack_ai_runtime::ports::model::CancellationSignal::new(),
            relation_depth: 0,
        },
        tool_batch_id: finstack_ai_kernel::ToolBatchId::from_bytes([8; 16]),
        tool_call_id: finstack_ai_kernel::ToolCallId::from_bytes([9; 16]),
    }
}

fn tool_call(
    toolset: &SubagentToolset,
    name: &str,
    arguments: &serde_json::Value,
) -> ValidatedToolCall {
    let tools = toolset.tools();
    let spec = tools
        .iter()
        .find(|tool| tool.model_name.as_ref() == name)
        .expect("tool");
    ValidatedToolCall {
        call: ToolCallBlock::try_new(
            finstack_ai_kernel::ToolCallId::from_bytes([9; 16]),
            name,
            RawJson::parse(serde_json::to_vec(&arguments).expect("arguments")).expect("raw"),
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

async fn invoke(
    toolset: &SubagentToolset,
    parent: &AgentRun,
    name: &str,
    arguments: serde_json::Value,
) -> finstack_ai_runtime::ports::tool::ToolResult {
    let mut stream = toolset
        .call(tool_context(parent), tool_call(toolset, name, &arguments))
        .await
        .expect("call");
    match stream.next().await.expect("item").expect("stream") {
        ToolStreamItem::Completed(result) => result,
        other => panic!("expected completion, got {other:?}"),
    }
}

#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "the lane contract keeps the complete parent-child transition in one scenario"
)]
async fn subagent_start_await_incorporates_child_text() {
    let store = memory_journal();
    let store_port: Arc<dyn JournalStore> = store.clone();
    let child = build_agent(
        "test.agent.subagent-child",
        Arc::new(ScriptedModel::from_plans(
            scripted_profile(),
            vec![completed_plan("child done")],
        )),
        Arc::clone(&store_port),
        ChildRunPolicy::Deny,
    )
    .await;
    let invoker = Arc::new(ParentStartInvoker {
        parent: Mutex::new(None),
        child,
        wait_for_result: true,
        started: Arc::new(Mutex::new(Vec::new())),
    });
    let toolset = SubagentToolset::try_new(
        Arc::new(ChildRunStarter::new(
            Arc::clone(&store_port),
            ChildRunPolicy::Allow { max_depth: 1 },
            Arc::clone(&invoker) as Arc<dyn AgentInvoker>,
        )),
        Arc::from([child_ref()]),
    )
    .expect("toolset");
    let gate = Arc::<str>::from("subagent-start-await");
    let mut plan = completed_plan("incorporated: child done");
    plan.actions
        .insert(0, ScriptedModelAction::Block(Arc::clone(&gate)));
    let parent_model = Arc::new(ScriptedModel::from_plans(scripted_profile(), vec![plan]));
    let control = parent_model.control();
    let parent_agent = build_agent(
        "test.agent.subagent-parent",
        parent_model,
        Arc::clone(&store_port),
        ChildRunPolicy::Allow { max_depth: 1 },
    )
    .await;
    let parent = parent_agent
        .start(agent_request("delegate"))
        .expect("parent start");
    tokio::time::timeout(Duration::from_secs(3), async {
        while control.entries(gate.as_ref()) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("parent reached gate");
    *invoker.parent.lock().expect("slot") = Some(parent.clone());
    let started = invoke(
        &toolset,
        &parent,
        "subagent_start",
        serde_json::json!({
            "agent_id": "test.agent.subagent-child",
            "input": "child work",
            "placement": "isolated_child_session"
        }),
    )
    .await;
    assert!(!started.is_error, "{}", started.output.as_str());
    let started_json: serde_json::Value =
        serde_json::from_slice(started.output.as_bytes()).expect("start json");
    let awaited = invoke(
        &toolset,
        &parent,
        "subagent_status",
        serde_json::json!({ "run_id": started_json["run_id"] }),
    )
    .await;
    assert!(!awaited.is_error, "{}", awaited.output.as_str());
    let kinds = journal_kind_names(&store_port, parent.locator().session_id).await;
    assert!(
        kinds.iter().any(|kind| kind == "child_run_prepared"),
        "start must journal the child mapping: {kinds:?}"
    );
    let child_session = finstack_ai_kernel::SessionId::parse(
        started_json["session_id"].as_str().expect("session_id"),
    )
    .expect("child session");
    let child_kinds = journal_kind_names(&store_port, child_session).await;
    assert!(
        child_kinds.iter().any(|kind| kind == "run_accepted"),
        "child journal must accept and complete: {child_kinds:?}"
    );
    let child_journal = store_port
        .load(finstack_ai_runtime::ports::journal::LoadRequest {
            session_id: child_session,
        })
        .await
        .expect("load child journal");
    let has_child_text = child_journal.committed_batches.iter().any(|batch| {
        batch
            .records
            .iter()
            .any(|record| format!("{:?}", record.body()).contains("child done"))
    });
    assert!(
        has_child_text,
        "awaited child journal must incorporate child text"
    );
    let _ = control;
}

#[tokio::test]
async fn subagent_deny_leaves_no_child_records() {
    let store = memory_journal();
    let store_port: Arc<dyn JournalStore> = store.clone();
    let child = build_agent(
        "test.agent.subagent-child",
        Arc::new(ScriptedModel::from_plans(
            scripted_profile(),
            vec![completed_plan("unused")],
        )),
        Arc::clone(&store_port),
        ChildRunPolicy::Deny,
    )
    .await;
    let invoker = Arc::new(ParentStartInvoker {
        parent: Mutex::new(None),
        child,
        wait_for_result: false,
        started: Arc::new(Mutex::new(Vec::new())),
    });
    let toolset = SubagentToolset::try_new(
        Arc::new(ChildRunStarter::new(
            Arc::clone(&store_port),
            ChildRunPolicy::Deny,
            Arc::clone(&invoker) as Arc<dyn AgentInvoker>,
        )),
        Arc::from([child_ref()]),
    )
    .expect("toolset");
    let gate = Arc::<str>::from("subagent-deny");
    let mut plan = completed_plan("denied");
    plan.actions
        .insert(0, ScriptedModelAction::Block(Arc::clone(&gate)));
    let parent_model = Arc::new(ScriptedModel::from_plans(scripted_profile(), vec![plan]));
    let control = parent_model.control();
    let parent_agent = build_agent(
        "test.agent.subagent-parent",
        parent_model,
        Arc::clone(&store_port),
        ChildRunPolicy::Deny,
    )
    .await;
    let parent = parent_agent
        .start(agent_request("delegate"))
        .expect("parent start");
    tokio::time::timeout(Duration::from_secs(3), async {
        while control.entries(gate.as_ref()) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("parent reached gate");
    *invoker.parent.lock().expect("slot") = Some(parent.clone());
    let started = invoke(
        &toolset,
        &parent,
        "subagent_start",
        serde_json::json!({
            "agent_id": "test.agent.subagent-child",
            "input": "child work"
        }),
    )
    .await;
    assert!(started.is_error, "deny must be a tool result");
    let payload: serde_json::Value =
        serde_json::from_slice(started.output.as_bytes()).expect("json");
    assert_eq!(payload["code"], AGENT_INVOKE_INVALID_ACCEPTANCE);
    control.release(gate.as_ref());
    let _ = tokio::time::timeout(Duration::from_secs(8), parent.result()).await;
    let kinds = journal_kind_names(&store_port, parent.locator().session_id).await;
    assert!(
        !kinds.iter().any(|kind| kind == "child_run_prepared"),
        "deny must leave no child journal records: {kinds:?}"
    );
}

#[tokio::test]
async fn subagent_cancel_fans_out_to_isolated() {
    let store = memory_journal();
    let store_port: Arc<dyn JournalStore> = store.clone();
    let child_gate = Arc::<str>::from("subagent-child-gate");
    let mut isolated_plan = completed_plan("isolated");
    isolated_plan
        .actions
        .insert(0, ScriptedModelAction::Block(Arc::clone(&child_gate)));
    let child_model = Arc::new(ScriptedModel::from_plans(
        scripted_profile(),
        vec![isolated_plan],
    ));
    let child_control = child_model.control();
    let child = build_agent(
        "test.agent.subagent-child",
        child_model,
        Arc::clone(&store_port),
        ChildRunPolicy::Deny,
    )
    .await;
    let gate = Arc::<str>::from("subagent-parent-cancel");
    let mut plan = completed_plan("parent unused");
    plan.actions
        .insert(0, ScriptedModelAction::Block(Arc::clone(&gate)));
    let parent_model = Arc::new(ScriptedModel::from_plans(scripted_profile(), vec![plan]));
    let control = parent_model.control();
    let parent_agent = build_agent(
        "test.agent.subagent-parent",
        parent_model,
        Arc::clone(&store_port),
        ChildRunPolicy::Allow { max_depth: 1 },
    )
    .await;
    let parent = parent_agent
        .start(agent_request("hold"))
        .expect("parent start");
    tokio::time::timeout(Duration::from_secs(3), async {
        while control.entries(gate.as_ref()) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("parent reached gate");
    let isolated = tokio::time::timeout(
        Duration::from_secs(3),
        Box::pin(parent.start_child(
            &child,
            agent_request("isolated work"),
            ChildPlacement::IsolatedChildSession,
            None,
        )),
    )
    .await
    .expect("isolated timeout")
    .expect("isolated");
    tokio::time::timeout(Duration::from_secs(3), async {
        while child_control.entries(child_gate.as_ref()) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("isolated child reached gate");
    let compatible_error = tokio::time::timeout(
        Duration::from_secs(3),
        Box::pin(parent.start_child(
            &child,
            agent_request("compatible work"),
            ChildPlacement::CompatibleLaneInParentSession,
            None,
        )),
    )
    .await
    .expect("compatible timeout")
    .err()
    .expect("compatible mid-turn must fail closed");
    assert_eq!(compatible_error.code(), AGENT_RUN_INVALID_CONFIGURATION);
    let parent_kinds = journal_kind_names(&store_port, parent.locator().session_id).await;
    let compatible_prepared = parent_kinds
        .iter()
        .filter(|kind| *kind == "child_run_prepared")
        .count();
    assert_eq!(
        compatible_prepared, 1,
        "only the isolated mapping is journaled mid-turn: {parent_kinds:?}"
    );
    tokio::time::timeout(Duration::from_secs(3), parent.cancel())
        .await
        .expect("cancel timeout")
        .expect("cancel");
    let isolated_error = tokio::time::timeout(Duration::from_secs(3), isolated.result())
        .await
        .expect("isolated result")
        .expect_err("isolated must cancel");
    assert_eq!(isolated_error.code(), AGENT_RUN_CANCELLED);
}

fn subagent_start_plan() -> finstack_ai_test::ScriptedModelPlan {
    let arguments = RawJson::parse(
        br#"{"agent_id":"test.agent.subagent-child","input":"child work","placement":"isolated_child_session"}"#,
    )
    .expect("arguments");
    finstack_ai_test::ScriptedModelPlan {
        actions: vec![
            ScriptedModelAction::Emit(Ok(ModelStreamItem::ToolCallDelta(ToolCallDelta {
                index: 0,
                name: Some(Arc::from("subagent_start")),
                arguments_delta: Arc::from(arguments.as_str()),
                provider_call_id: None,
            }))),
            ScriptedModelAction::Emit(Ok(ModelStreamItem::Completed(ModelResponse {
                assistant_content: Arc::from([]),
                tool_calls: Arc::from([ModelToolCall {
                    name: Arc::from("subagent_start"),
                    arguments,
                    provider_call_id: None,
                }]),
                usage: Usage::empty(),
                provider_ids: ProviderIds::empty(),
                completion_id: Arc::from("subagent-approval"),
                continuation_state: None,
            }))),
        ],
    }
}

struct ApprovalLoopInvoker {
    starts: AtomicUsize,
}

impl AgentInvoker for ApprovalLoopInvoker {
    fn start_or_attach(
        &self,
        context: ChildRunContext,
        request: ChildRunRequest,
    ) -> PortFuture<Result<ChildRunHandle, AgentInvokeError>> {
        self.starts.fetch_add(1, Ordering::SeqCst);
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

async fn wait_for_listed_interactions(run: &AgentRun) -> Vec<finstack_ai::InteractionRequest> {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let listed = run.list_interactions().await.expect("list");
            if !listed.is_empty() {
                return listed;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("approval timeout")
}

#[tokio::test]
async fn subagent_start_approval_loop_resolves_durable_interaction() {
    let store = memory_journal();
    let store_port: Arc<dyn JournalStore> = store.clone();
    let invoker = Arc::new(ApprovalLoopInvoker {
        starts: AtomicUsize::new(0),
    });
    let toolset = SubagentToolset::try_new(
        Arc::new(ChildRunStarter::new(
            Arc::clone(&store_port),
            ChildRunPolicy::Allow { max_depth: 1 },
            Arc::clone(&invoker) as Arc<dyn AgentInvoker>,
        )),
        Arc::from([child_ref()]),
    )
    .expect("toolset");
    let parent_model = Arc::new(ScriptedModel::from_plans(
        scripted_profile(),
        vec![
            subagent_start_plan(),
            completed_plan("incorporated after approval"),
        ],
    ));
    let parent_agent = Agent::builder(
        AgentId::parse("test.agent.subagent-parent").expect("agent"),
        BundleId::parse("test.bundle.subagent").expect("bundle"),
        (
            ComponentRef::new(
                ComponentId::parse("test.model.subagent-parent").expect("model"),
                Some(VERSION),
            ),
            parent_model as Arc<dyn Model>,
        ),
        (
            ComponentRef::new(
                ComponentId::parse("test.store.subagent").expect("store"),
                Some(VERSION),
            ),
            Arc::clone(&store_port),
        ),
    )
    .policy(RunPolicy {
        child_runs: ChildRunPolicy::Allow { max_depth: 1 },
        ..RunPolicy::default()
    })
    .toolset(
        ComponentRef::new(
            ComponentId::parse("test.toolset.subagent").expect("toolset"),
            Some(VERSION),
        ),
        Arc::new(toolset) as Arc<dyn Toolset>,
    )
    .build()
    .await
    .expect("parent");
    let parent = parent_agent
        .start(agent_request("delegate"))
        .expect("parent start");
    assert_eq!(invoker.starts.load(Ordering::SeqCst), 0);
    let listed = wait_for_listed_interactions(&parent).await;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].kind(), &InteractionKind::Approval);
    let security = security("decision-v1");
    let resolution = InteractionResolution::try_new(
        listed[0].interaction_id(),
        "subagent-approval-1",
        security.principal().clone(),
        AuthorizationEvidence::try_new(
            security.authorization_policy_version(),
            security.authorization_decision_id(),
        )
        .expect("auth"),
        RawJson::parse(br#"{"approved":true}"#).expect("approved"),
        None::<&str>,
    )
    .expect("resolution");
    tokio::time::timeout(
        Duration::from_secs(3),
        parent.resolve_interaction(resolution),
    )
    .await
    .expect("resolve timeout")
    .expect("resolve");
    let output = tokio::time::timeout(Duration::from_secs(8), parent.result())
        .await
        .expect("parent timeout")
        .expect("parent result");
    assert!(
        output.text().contains("incorporated after approval"),
        "{}",
        output.text()
    );
    assert_eq!(
        invoker.starts.load(Ordering::SeqCst),
        1,
        "granted approval must dispatch subagent_start"
    );
    let kinds = journal_kind_names(&store_port, parent.locator().session_id).await;
    assert!(
        kinds.iter().any(|kind| kind == "interaction_requested"),
        "approval loop must journal the durable interaction: {kinds:?}"
    );
}
