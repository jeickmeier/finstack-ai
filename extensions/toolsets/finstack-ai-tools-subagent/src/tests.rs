use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use finstack_ai_runtime::{
    AGENT_INVOKE_INVALID_ACCEPTANCE, AgentId, AgentInvokeError, AgentInvoker, AgentRef,
    AuthorizationContext, CancellationSignal, ChildPlacement, ChildRunContext, ChildRunHandle,
    ChildRunLocator, ChildRunRequest, Digest, EffectId, EffectOutputContract, EffectOutputKind,
    LaneId, Metadata, OperationLocator, PortFuture, PrincipalRef, RawJson, RunCallContext, RunId,
    SessionId, SideEffectClass, ToolBatchId, ToolCallBlock, ToolCallId, ToolFailurePolicy,
    ToolStreamItem, Toolset, ValidatedToolCall, child_relation_digest,
};
use futures_util::StreamExt;

use super::*;

struct RecordingInvoker {
    starts: AtomicUsize,
    cancels: AtomicUsize,
    error: Option<AgentInvokeError>,
}

impl AgentInvoker for RecordingInvoker {
    fn start_or_attach(
        &self,
        context: ChildRunContext,
        request: ChildRunRequest,
    ) -> PortFuture<Result<ChildRunHandle, AgentInvokeError>> {
        self.starts.fetch_add(1, Ordering::SeqCst);
        if let Some(error) = self.error.clone() {
            return Box::pin(async move { Err(error) });
        }
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
        let locator = request.locator;
        Box::pin(async move {
            Ok(ChildRunHandle {
                locator,
                relation_digest,
            })
        })
    }

    fn cancel(&self, locator: &ChildRunLocator) -> PortFuture<Result<(), AgentInvokeError>> {
        let _ = locator;
        self.cancels.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(()) })
    }
}

fn allow_list() -> Arc<[AgentRef]> {
    Arc::from([AgentRef {
        id: AgentId::parse("finstack.agent.child").expect("agent"),
        bundle: None,
        spec_digest: Digest::raw_json(br#"{"agent":"child"}"#),
    }])
}

fn context() -> ToolCallContext {
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
        },
        tool_batch_id: ToolBatchId::from_bytes([5; 16]),
        tool_call_id: ToolCallId::from_bytes([6; 16]),
    }
}

fn call(toolset: &SubagentToolset, name: &str, arguments: &serde_json::Value) -> ValidatedToolCall {
    let tools = toolset.tools();
    let spec = tools
        .iter()
        .find(|tool| tool.model_name.as_ref() == name)
        .expect("tool");
    ValidatedToolCall {
        call: ToolCallBlock::try_new(
            context().tool_call_id,
            name,
            RawJson::parse(serde_json::to_vec(arguments).expect("arguments")).expect("raw"),
        )
        .expect("tool call"),
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
    name: &str,
    arguments: serde_json::Value,
) -> Result<ToolResult, ToolError> {
    let mut stream = toolset
        .call(context(), call(toolset, name, &arguments))
        .await?;
    match stream.next().await.expect("item").expect("stream") {
        ToolStreamItem::Completed(result) => Ok(result),
        other => panic!("expected completion, got {other:?}"),
    }
}

fn start_args(agent_id: &str) -> serde_json::Value {
    serde_json::json!({
        "agent_id": agent_id,
        "input": "child work",
        "placement": "isolated_child_session",
    })
}

#[test]
fn subagent_toolset_exposes_exactly_three_tools() {
    let invoker = Arc::new(RecordingInvoker {
        starts: AtomicUsize::new(0),
        cancels: AtomicUsize::new(0),
        error: None,
    });
    let toolset = SubagentToolset::try_new(invoker, allow_list()).expect("toolset");
    let tools = toolset.tools();
    let names: Vec<_> = tools.iter().map(|tool| tool.model_name.as_ref()).collect();
    assert_eq!(
        names,
        ["subagent_start", "subagent_await", "subagent_cancel"]
    );
    for spec in tools.iter() {
        assert_eq!(spec.side_effect, SideEffectClass::NonIdempotentWrite);
        assert_eq!(spec.retry_safety, RetrySafety::AtMostOnce);
        assert_eq!(spec.approval.requirement, ApprovalRequirement::Required);
    }
}

#[test]
fn try_new_rejects_empty_allow_list() {
    let invoker = Arc::new(RecordingInvoker {
        starts: AtomicUsize::new(0),
        cancels: AtomicUsize::new(0),
        error: None,
    });
    let error = SubagentToolset::try_new(invoker, Arc::from([]))
        .err()
        .expect("empty");
    assert!(matches!(
        error,
        SubagentError::Configuration {
            reason: "empty_allow_list"
        }
    ));
}

#[tokio::test]
async fn unknown_agent_id_is_a_tool_result_not_a_run_abort() {
    let invoker = Arc::new(RecordingInvoker {
        starts: AtomicUsize::new(0),
        cancels: AtomicUsize::new(0),
        error: None,
    });
    let toolset =
        SubagentToolset::try_new(Arc::clone(&invoker) as Arc<dyn AgentInvoker>, allow_list())
            .expect("toolset");
    let result = invoke(&toolset, START_NAME, start_args("finstack.agent.other"))
        .await
        .expect("tool result");
    assert!(result.is_error);
    let payload: serde_json::Value =
        serde_json::from_slice(result.output.as_bytes()).expect("json");
    assert_eq!(payload["code"], SUBAGENT_AGENT_NOT_ALLOWED);
    assert_eq!(invoker.starts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn policy_and_budget_failures_surface_as_tool_results() {
    let invoker = Arc::new(RecordingInvoker {
        starts: AtomicUsize::new(0),
        cancels: AtomicUsize::new(0),
        error: Some(AgentInvokeError::InvalidRequest {
            message: Arc::from("child run policy denies child invocation"),
        }),
    });
    let toolset =
        SubagentToolset::try_new(Arc::clone(&invoker) as Arc<dyn AgentInvoker>, allow_list())
            .expect("toolset");
    let result = invoke(&toolset, START_NAME, start_args("finstack.agent.child"))
        .await
        .expect("tool result");
    assert!(result.is_error);
    let payload: serde_json::Value =
        serde_json::from_slice(result.output.as_bytes()).expect("json");
    assert_eq!(payload["code"], AGENT_INVOKE_INVALID_ACCEPTANCE);
    assert_eq!(invoker.starts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn start_await_and_cancel_track_compatible_and_isolated_only() {
    let invoker = Arc::new(RecordingInvoker {
        starts: AtomicUsize::new(0),
        cancels: AtomicUsize::new(0),
        error: None,
    });
    let toolset =
        SubagentToolset::try_new(Arc::clone(&invoker) as Arc<dyn AgentInvoker>, allow_list())
            .expect("toolset");
    let started = invoke(&toolset, START_NAME, start_args("finstack.agent.child"))
        .await
        .expect("start");
    assert!(!started.is_error);
    let payload: serde_json::Value =
        serde_json::from_slice(started.output.as_bytes()).expect("json");
    let run_id = payload["run_id"].as_str().expect("run_id").to_string();
    let awaited = invoke(&toolset, AWAIT_NAME, serde_json::json!({}))
        .await
        .expect("await");
    assert!(!awaited.is_error);
    let _ = run_id;
    let cancelled = invoke(
        &toolset,
        CANCEL_NAME,
        serde_json::json!({ "run_id": run_id }),
    )
    .await
    .expect("cancel");
    assert!(!cancelled.is_error);
    assert_eq!(invoker.cancels.load(Ordering::SeqCst), 1);
    let missing = invoke(
        &toolset,
        CANCEL_NAME,
        serde_json::json!({ "run_id": "00000000-0000-7000-8000-000000000099" }),
    )
    .await
    .expect("missing");
    assert!(missing.is_error);
}

#[tokio::test]
async fn remote_cancel_is_forwarded_to_the_invoker() {
    let child = StartedChild {
        handle: ChildRunHandle {
            locator: ChildRunLocator {
                operation: OperationLocator::try_new(
                    "tenant-a",
                    SessionId::from_bytes([8; 16]),
                    LaneId::from_bytes([9; 16]),
                    RunId::from_bytes([10; 16]),
                )
                .expect("locator"),
                remote: None,
            },
            relation_digest: Digest::raw_json(b"{}"),
        },
        placement: ChildPlacement::RemoteChildSession,
    };
    let mut children = BTreeMap::new();
    children.insert(Arc::from("remote-1"), child);
    let invoker = Arc::new(RecordingInvoker {
        starts: AtomicUsize::new(0),
        cancels: AtomicUsize::new(0),
        error: None,
    });
    let call = {
        let toolset =
            SubagentToolset::try_new(Arc::clone(&invoker) as Arc<dyn AgentInvoker>, allow_list())
                .expect("toolset");
        call(
            &toolset,
            CANCEL_NAME,
            &serde_json::json!({ "run_id": "remote-1" }),
        )
    };
    let outcome = cancel_child(
        &(Arc::clone(&invoker) as Arc<dyn AgentInvoker>),
        &children,
        &call,
    )
    .await
    .expect("cancel");
    assert!(!outcome.is_error);
    assert_eq!(invoker.cancels.load(Ordering::SeqCst), 1);
}
