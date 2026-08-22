use std::sync::{Arc, Mutex};

use finstack_ai_kernel::{
    ActiveCapability, CapabilityActivationSource, CapabilityId, EffectId, LaneId, OperationLocator,
    RunId, SessionId,
};
use finstack_ai_kernel::{
    Digest, EffectOutputContract, EffectOutputKind, Metadata, PrincipalRef, RawJson, ToolBatchId,
    ToolCallBlock, ToolCallId, ToolFailurePolicy, ValidatedToolCall,
};
use finstack_ai_runtime::ports::model::{AuthorizationContext, CancellationSignal};
use finstack_ai_runtime::ports::tool::{ToolResult, ToolStreamItem, Toolset};
use futures_util::StreamExt;

use super::*;

struct RecordingHost {
    active: Vec<ActiveCapability>,
    submitted: Mutex<Option<Vec<ActiveCapability>>>,
    bound: bool,
}

fn alpha() -> CapabilityId {
    CapabilityId::parse("demo.capability.alpha").expect("alpha")
}

fn catalog() -> String {
    "demo.capability.alpha: Always on\ndemo.capability.beta: Research notes".to_owned()
}

fn host(recording: Arc<RecordingHost>) -> SkillsHost {
    let active = recording.active.clone();
    let bound = recording.bound;
    SkillsHost {
        catalog: catalog(),
        active: Arc::new(move |_run| Ok(active.clone())),
        activate: Arc::new(move |_run, complete| {
            if bound {
                return Err(SkillsHostError::Bound);
            }
            *recording.submitted.lock().expect("lock") = Some(complete);
            Ok(())
        }),
    }
}

fn context() -> ToolCallContext {
    ToolCallContext {
        run: finstack_ai_runtime::ports::model::RunCallContext {
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

fn call(toolset: &SkillsToolset, name: &str, arguments: &serde_json::Value) -> ValidatedToolCall {
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

async fn invoke(toolset: &SkillsToolset, name: &str, arguments: serde_json::Value) -> ToolResult {
    let mut stream = toolset
        .call(context(), call(toolset, name, &arguments))
        .await
        .expect("call");
    match stream.next().await.expect("item").expect("stream") {
        ToolStreamItem::Completed(result) => result,
        other => panic!("expected completion, got {other:?}"),
    }
}

#[test]
fn skills_toolset_exposes_list_and_activate() {
    let recording = Arc::new(RecordingHost {
        active: Vec::new(),
        submitted: Mutex::new(None),
        bound: false,
    });
    let toolset = SkillsToolset::try_new(host(recording)).expect("toolset");
    let tools = toolset.tools();
    let names: Vec<_> = tools.iter().map(|tool| tool.model_name.as_ref()).collect();
    assert_eq!(names, ["capability_list", "capability_activate"]);
}

#[tokio::test]
async fn capability_activate_naming_one_id_does_not_drop_already_active() {
    let recording = Arc::new(RecordingHost {
        active: vec![ActiveCapability {
            capability_id: alpha(),
            source: CapabilityActivationSource::Always,
        }],
        submitted: Mutex::new(None),
        bound: false,
    });
    let toolset = SkillsToolset::try_new(host(Arc::clone(&recording))).expect("toolset");
    let result = invoke(
        &toolset,
        "capability_activate",
        serde_json::json!({ "id": "demo.capability.beta" }),
    )
    .await;
    assert!(!result.is_error);
    let submitted = recording
        .submitted
        .lock()
        .expect("lock")
        .clone()
        .expect("submitted");
    let ids: Vec<_> = submitted
        .iter()
        .map(|item| item.capability_id.as_str())
        .collect();
    assert_eq!(ids, ["demo.capability.alpha", "demo.capability.beta"]);
}

#[tokio::test]
async fn activating_a_model_capability_commits_before_tools_appear() {
    let recording = Arc::new(RecordingHost {
        active: vec![ActiveCapability {
            capability_id: alpha(),
            source: CapabilityActivationSource::Always,
        }],
        submitted: Mutex::new(None),
        bound: false,
    });
    let toolset = SkillsToolset::try_new(host(Arc::clone(&recording))).expect("toolset");
    let result = invoke(
        &toolset,
        "capability_activate",
        serde_json::json!({ "id": "demo.capability.beta" }),
    )
    .await;
    assert!(!result.is_error);
    assert!(
        recording.submitted.lock().expect("lock").is_some(),
        "host must receive the complete set before the tool result returns"
    );
}

#[tokio::test]
async fn capability_activate_bound_fails_the_tool_call() {
    let recording = Arc::new(RecordingHost {
        active: Vec::new(),
        submitted: Mutex::new(None),
        bound: true,
    });
    let toolset = SkillsToolset::try_new(host(recording)).expect("toolset");
    let result = invoke(
        &toolset,
        "capability_activate",
        serde_json::json!({ "id": "demo.capability.beta" }),
    )
    .await;
    assert!(result.is_error);
    let text = String::from_utf8_lossy(result.output.as_bytes());
    assert!(text.contains(SKILLS_ACTIVATION_BOUND));
}
