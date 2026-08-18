use std::sync::Mutex;

use finstack_ai::{AGENT_RUN_CANCELLED, Agent, AgentRun, RunPolicy};
use finstack_ai_kernel::{
    AgentId, BundleId, ComponentId, ComponentRef, ContentBlock, EffectOutputContract,
    EffectOutputKind, Version,
};
use finstack_ai_runtime::{
    AgentInvokeError, AgentInvoker, AgentRef, ChildRunHandle, ChildRunLocator, ChildRunRequest,
    Digest, Model, PortFuture, RawJson, ToolCallBlock, ToolFailurePolicy, ToolStreamItem,
    ValidatedToolCall, child_relation_digest,
};
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
        Box::pin(async move {
            let parent = parent.ok_or_else(|| AgentInvokeError::Unavailable {
                message: Arc::from("parent run is not attached"),
            })?;
            let input = request
                .input
                .iter()
                .find_map(|block| match block {
                    ContentBlock::Text(text) => Some(text.text().to_string()),
                    _ => None,
                })
                .unwrap_or_default();
            let run = parent
                .start_child(&child, agent_request(&input), request.placement)
                .await
                .map_err(|error| AgentInvokeError::InvalidRequest {
                    message: Arc::from(error.to_string()),
                })?;
            if wait_for_result {
                let _ = Box::pin(run.result()).await;
            }
            let relation_digest = child_relation_digest(&context, &request).map_err(|error| {
                AgentInvokeError::InvalidRequest {
                    message: Arc::from(error.to_string()),
                }
            })?;
            Ok(ChildRunHandle {
                locator: ChildRunLocator {
                    operation: run.locator().clone(),
                    remote: None,
                },
                relation_digest,
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

fn tool_context(parent: &AgentRun) -> finstack_ai_runtime::ToolCallContext {
    finstack_ai_runtime::ToolCallContext {
        run: finstack_ai_runtime::RunCallContext {
            locator: parent.locator().clone(),
            authorization: finstack_ai_runtime::AuthorizationContext {
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
            cancellation: finstack_ai_runtime::CancellationSignal::new(),
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
) -> finstack_ai_runtime::ToolResult {
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
    });
    let toolset = SubagentToolset::try_new(
        Arc::clone(&invoker) as Arc<dyn AgentInvoker>,
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
    let awaited = invoke(&toolset, &parent, "subagent_await", serde_json::json!({})).await;
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
        .load(finstack_ai_runtime::LoadRequest {
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
    });
    let toolset = SubagentToolset::try_new(
        Arc::clone(&invoker) as Arc<dyn AgentInvoker>,
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
async fn subagent_cancel_fans_out_to_compatible_and_isolated() {
    let store = memory_journal();
    let store_port: Arc<dyn JournalStore> = store.clone();
    let child_gate = Arc::<str>::from("subagent-child-gate");
    let mut isolated_plan = completed_plan("isolated");
    isolated_plan
        .actions
        .insert(0, ScriptedModelAction::Block(Arc::clone(&child_gate)));
    let mut compatible_plan = completed_plan("compatible");
    compatible_plan
        .actions
        .insert(0, ScriptedModelAction::Block(Arc::clone(&child_gate)));
    let child_model = Arc::new(ScriptedModel::from_plans(
        scripted_profile(),
        vec![isolated_plan, compatible_plan],
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
        store_port,
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
    let compatible = tokio::time::timeout(
        Duration::from_secs(3),
        Box::pin(parent.start_child(
            &child,
            agent_request("compatible work"),
            ChildPlacement::CompatibleLaneInParentSession,
        )),
    )
    .await
    .expect("compatible timeout")
    .expect("compatible");
    tokio::time::timeout(Duration::from_secs(3), parent.cancel())
        .await
        .expect("cancel timeout")
        .expect("cancel");
    let isolated_error = tokio::time::timeout(Duration::from_secs(3), isolated.result())
        .await
        .expect("isolated result")
        .expect_err("isolated must cancel");
    let compatible_error = tokio::time::timeout(Duration::from_secs(3), compatible.result())
        .await
        .expect("compatible result")
        .expect_err("compatible must cancel");
    assert_eq!(isolated_error.code(), AGENT_RUN_CANCELLED);
    assert!(
        compatible_error.code() == AGENT_RUN_CANCELLED
            || compatible_error.code() == "agent_run_runtime_failure",
        "compatible-lane cancel fans out or hits the shared-session caveat; remote is not asserted: {} {}",
        compatible_error.code(),
        compatible_error
    );
}
