// Tool-policy narrowing → provider bypass → denial lane.
//
// Exercises the full public path: register a two-tool `ScriptedToolset` (one
// `ReadOnly`, one `NonIdempotentWrite`) plus `ToolPolicyMiddleware` configured
// with a role allowlist whose `default_allowed` set contains only the
// read-only tool, then assert that (a) the model-visible request only ever
// carries the read tool (the `before_model` `FilterTools` fold narrowed the
// draft) and (b) a scripted provider that calls the write tool anyway never
// executes it — the call settles as a `tool_policy_denied` synthetic closure
// surfaced back to the model, not a dropped call.
//
// The harness's dispatch path (`dispatch_security_context` in
// `finstack-ai-runtime/src/exec/coordinator/dispatch.rs`) always builds
// `AuthorizationContext.roles` as empty — there is no way for this lane test
// to grant a named role through the public `AgentRunRequest`/`security()`
// fixture. The policy is therefore configured with an empty named-role map
// and keys the allow set entirely on `default_allowed` (the tools allowed
// for roles absent from the map, which covers the empty-granted-roles case
// exactly).

use std::collections::BTreeSet;

use finstack_ai_kernel::{RetrySafety, ToolExecutionMode, ToolId};
use finstack_ai_middleware_tool_policy::{ToolPolicyConfig, ToolPolicyMiddleware};
use finstack_ai_runtime::ports::model::{ApprovalMetadata, ApprovalRequirement, ModelName, SideEffectClass, ToolDeferralSupport, ToolSpec};
use finstack_ai_test::ScriptedToolset;

const TOOL_POLICY_COMPONENT_VERSION: Version = Version {
    major: 1,
    minor: 0,
    patch: 0,
};

fn read_tool_id() -> ToolId {
    ToolId::parse("finstack.tools.tool-policy-read").expect("read tool id")
}

fn write_tool_id() -> ToolId {
    ToolId::parse("finstack.tools.tool-policy-write").expect("write tool id")
}

fn tool_policy_tool_spec(id: ToolId, model_name: &str, side_effect: SideEffectClass) -> ToolSpec {
    ToolSpec {
        id,
        model_name: Arc::from(model_name),
        title: Arc::from(model_name),
        description: Arc::from("tool-policy lane fixture"),
        input_schema: RawJson::parse(b"{}").expect("schema"),
        output_schema: None,
        execution: ToolExecutionMode::Parallel,
        side_effect,
        retry_safety: RetrySafety::SafeToRetry,
        approval: ApprovalMetadata {
            requirement: ApprovalRequirement::NotRequired,
            reason: None,
            attributes: Metadata::empty(),
        },
        max_result_bytes: 1_024,
        metadata: Metadata::empty(),
        deferral: ToolDeferralSupport::Never,
    }
}

/// The model's first-turn action: call the write tool even though the
/// `before_model` narrowing already hid it from `request.tools` — modeling a
/// provider that routes around the visible tool set.
fn call_write_tool_plan() -> finstack_ai_test::ScriptedModelPlan {
    let arguments = RawJson::parse(b"{}").expect("arguments");
    finstack_ai_test::ScriptedModelPlan {
        actions: vec![
            ScriptedModelAction::Emit(Ok(ModelStreamItem::ToolCallDelta(ToolCallDelta {
                index: 0,
                name: Some(Arc::from("tool_policy_write")),
                arguments_delta: Arc::from(arguments.as_str()),
                provider_call_id: None,
            }))),
            ScriptedModelAction::Emit(Ok(ModelStreamItem::Completed(ModelResponse {
                assistant_content: Arc::from([]),
                tool_calls: Arc::from([ModelToolCall {
                    name: Arc::from("tool_policy_write"),
                    arguments,
                    provider_call_id: None,
                }]),
                usage: Usage::empty(),
                provider_ids: ProviderIds::empty(),
                completion_id: Arc::from("tool-policy-turn-1"),
                continuation_state: None,
            }))),
        ],
    }
}

async fn tool_policy_agent() -> (Agent, Arc<ScriptedModel>, Arc<ScriptedToolset>) {
    let store: Arc<dyn JournalStore> = Arc::new(
        MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions: 8,
            batches_per_session: 64,
            records_per_session: 512,
            snapshot_bytes: 8_192,
        })
        .expect("journal store"),
    );
    let model = Arc::new(ScriptedModel::from_plans(
        scripted_profile(),
        vec![call_write_tool_plan(), completed_plan("acknowledged")],
    ));
    let toolset = Arc::new(ScriptedToolset::new(
        Arc::from([
            tool_policy_tool_spec(read_tool_id(), "tool_policy_read", SideEffectClass::ReadOnly),
            tool_policy_tool_spec(
                write_tool_id(),
                "tool_policy_write",
                SideEffectClass::NonIdempotentWrite,
            ),
        ]),
        // No plan is queued: the write call must never reach `Toolset::call`.
        vec![],
    ));

    let policy_config = ToolPolicyConfig::new()
        .with_role_allowlist(
            std::collections::BTreeMap::new(),
            BTreeSet::from([read_tool_id()]),
        )
        .expect("role allowlist");
    let middleware = Arc::new(
        ToolPolicyMiddleware::try_new(policy_config).expect("tool-policy middleware"),
    );

    let agent = Agent::builder(
        AgentId::parse("test.agent.tool-policy").expect("agent id"),
        BundleId::parse("test.bundle.tool-policy").expect("bundle id"),
        (
            ComponentRef::new(
                ComponentId::parse("test.model.tool-policy").expect("model id"),
                Some(TOOL_POLICY_COMPONENT_VERSION),
            ),
            Arc::clone(&model) as Arc<dyn Model>,
        ),
        (
            ComponentRef::new(
                ComponentId::parse("test.store.tool-policy").expect("store id"),
                Some(TOOL_POLICY_COMPONENT_VERSION),
            ),
            Arc::clone(&store),
        ),
    )
    .toolset(
        ComponentRef::new(
            ComponentId::parse("test.toolset.tool-policy").expect("toolset id"),
            Some(TOOL_POLICY_COMPONENT_VERSION),
        ),
        Arc::clone(&toolset) as Arc<dyn Toolset>,
    )
    .middleware(middleware,
    )
    .policy(RunPolicy::default())
    .build()
    .await
    .expect("agent");

    (agent, model, toolset)
}

#[tokio::test]
async fn tool_policy_lane_narrows_model_request_and_denies_filtered_batch_call() {
    let (agent, model, toolset) = tool_policy_agent().await;

    let request = AgentRunRequest::try_new(
        ModelName::try_new("lanes-1").expect("model name"),
        "Please write the report",
        security("decision-v1"),
    )
    .expect("request");

    // The run completes: a denied call settles as a synthetic closure that
    // is returned to the model, not a hang or a run-level failure.
    let output = agent.run(request).await.expect("run completes");
    assert_eq!(output.text(), "acknowledged");

    // (a) The captured model request's tools contain only the allowed
    // (read-only) tool — proving the `before_model` `FilterTools` fold
    // narrowed the draft before it ever reached the provider.
    let sent_request = model.last_request().expect("model was called");
    let sent_tool_ids: BTreeSet<ToolId> = sent_request
        .draft
        .tools
        .iter()
        .map(|tool| tool.id.clone())
        .collect();
    assert_eq!(
        sent_tool_ids,
        BTreeSet::from([read_tool_id()]),
        "model-visible tools must be narrowed to the read-only tool: {sent_tool_ids:?}"
    );

    // (b) The scripted provider called the write tool anyway (bypassing the
    // narrowed request). It must never execute...
    assert_eq!(
        toolset.call_count(),
        0,
        "the disallowed write tool must never reach Toolset::call"
    );

    // ...and it must settle as a `tool_policy_denied` synthetic closure
    // surfaced back to the model as a tool-result message, not a dropped
    // call: find the assistant's write-tool call, then the matching
    // tool-result content block in the same captured request's history.
    let write_call_id = sent_request
        .draft
        .messages
        .iter()
        .flat_map(finstack_ai_kernel::Message::content)
        .find_map(|block| match block {
            ContentBlock::ToolCall(call) if call.tool_name() == "tool_policy_write" => {
                Some(*call.tool_call_id())
            }
            _ => None,
        })
        .expect("write tool call must appear in the model-visible history");

    let denial = sent_request
        .draft
        .messages
        .iter()
        .flat_map(finstack_ai_kernel::Message::content)
        .find_map(|block| match block {
            ContentBlock::ToolResult(result) if *result.tool_call_id() == write_call_id => {
                Some(result)
            }
            _ => None,
        })
        .expect("write tool call must settle with a tool-result, not be dropped");
    assert!(
        denial.is_error(),
        "denial must be reported as an error result"
    );
    let denial_json: String = denial
        .content()
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Json(json) => Some(json.value().as_str().to_string()),
            _ => None,
        })
        .collect();
    assert!(
        denial_json.contains("tool_policy_denied"),
        "denial payload must carry the tool_policy_denied code: {denial_json:?}"
    );
}
