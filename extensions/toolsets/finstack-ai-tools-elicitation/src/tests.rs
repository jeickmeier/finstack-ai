use super::*;
use finstack_ai_kernel::{
    ContentBlock, Digest, EffectId, EffectOutputContract, EffectOutputKind, InteractionId, LaneId,
    OperationLocator, PrincipalRef, RawJson, RunId, SessionId, ToolBatchId, ToolCallBlock,
    ToolCallId, ToolFailurePolicy,
};
use finstack_ai_runtime::{
    ApprovalRequirement, AuthorizationContext, CancellationSignal, RunCallContext, ToolCallContext,
};
use futures_util::StreamExt;
use std::sync::Arc;

fn context(principal_scope: &str) -> ToolCallContext {
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
                principal: PrincipalRef::try_new("issuer", "subject", Some(principal_scope))
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

fn validated_call(
    toolset: &ElicitationToolset,
    tool_name: &str,
    arguments: &[u8],
) -> ValidatedToolCall {
    let spec = toolset
        .tools()
        .iter()
        .find(|spec| spec.model_name.as_ref() == tool_name)
        .expect("registered tool")
        .clone();
    ValidatedToolCall {
        call: ToolCallBlock::try_new(
            ToolCallId::from_bytes([6; 16]),
            tool_name,
            RawJson::parse(arguments).expect("arguments"),
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

fn free_form_toolset() -> ElicitationToolset {
    ElicitationToolset::builder()
        .with_ask_user()
        .build()
        .expect("toolset")
}

async fn parked_request(
    toolset: &ElicitationToolset,
    tool_name: &str,
    arguments: &[u8],
) -> InteractionRequest {
    let error = toolset
        .call(
            context("tenant-a"),
            validated_call(toolset, tool_name, arguments),
        )
        .await
        .err()
        .expect("elicitation must park");
    assert_eq!(error.code(), TOOL_INTERACTION_REQUIRED);
    interaction_request_from_tool_error(&error).expect("request in metadata")
}

fn prompt_text(request: &InteractionRequest) -> String {
    request
        .prompt()
        .iter()
        .map(|block| match block {
            ContentBlock::Text(text) => text.text().to_owned(),
            _ => String::new(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn builder_exposes_free_form_ask_user_spec() {
    let toolset = free_form_toolset();
    let tools = toolset.tools();
    assert_eq!(tools.len(), 1);
    let spec = &tools[0];
    assert_eq!(spec.id.as_str(), "finstack.tools.elicitation.ask_user");
    assert_eq!(spec.model_name.as_ref(), "ask_user");
    assert_eq!(
        spec.approval.requirement,
        ApprovalRequirement::NotRequired,
        "the interaction itself is the human touchpoint"
    );
    let schema: serde_json::Value =
        serde_json::from_slice(spec.input_schema.as_bytes()).expect("schema json");
    assert_eq!(schema["required"], serde_json::json!(["prompt"]));
}

#[test]
fn tool_specs_are_built_once_and_shared() {
    let toolset = free_form_toolset();
    assert!(Arc::ptr_eq(&toolset.tools(), &toolset.tools()));
}

#[tokio::test]
async fn ask_user_free_text_parks_with_interaction_request() {
    let toolset = free_form_toolset();
    let request = parked_request(
        &toolset,
        "ask_user",
        br#"{"prompt":"What is the position limit?"}"#,
    )
    .await;
    assert_eq!(*request.kind(), InteractionKind::FreeText);
    assert_eq!(prompt_text(&request), "What is the position limit?");
    assert_eq!(
        request.interaction_id(),
        InteractionId::from_bytes(EffectId::from_bytes([4; 16]).to_bytes()),
        "interaction id must be derived from the committed effect id"
    );
    let schema: serde_json::Value =
        serde_json::from_slice(request.response_schema().as_bytes()).expect("schema json");
    assert_eq!(schema["properties"]["answer"]["type"], "string");
}

#[tokio::test]
async fn ask_user_choice_builds_enum_response_schema() {
    let toolset = free_form_toolset();
    let request = parked_request(
        &toolset,
        "ask_user",
        br#"{"prompt":"Which account?","kind":"choice","options":["cash","margin"]}"#,
    )
    .await;
    assert_eq!(*request.kind(), InteractionKind::Choice);
    let schema: serde_json::Value =
        serde_json::from_slice(request.response_schema().as_bytes()).expect("schema json");
    assert_eq!(
        schema["properties"]["answer"]["enum"],
        serde_json::json!(["cash", "margin"])
    );
}

#[tokio::test]
async fn ask_user_form_uses_supplied_schema() {
    let toolset = free_form_toolset();
    let request = parked_request(
        &toolset,
        "ask_user",
        br#"{"prompt":"Fill in the trade.","kind":"form","response_schema":{"type":"object","properties":{"qty":{"type":"number"}},"required":["qty"]}}"#,
    )
    .await;
    assert_eq!(*request.kind(), InteractionKind::Form);
    let schema: serde_json::Value =
        serde_json::from_slice(request.response_schema().as_bytes()).expect("schema json");
    assert_eq!(
        schema["properties"]["answer"]["required"],
        serde_json::json!(["qty"]),
        "form schemas nest under the reserved answer key"
    );
    assert_eq!(schema["required"], serde_json::json!(["answer"]));
}

async fn completed_output(
    toolset: &ElicitationToolset,
    tool_name: &str,
    arguments: &[u8],
) -> serde_json::Value {
    let mut stream = toolset
        .call(
            context("tenant-a"),
            validated_call(toolset, tool_name, arguments),
        )
        .await
        .expect("resumed call must succeed");
    let item = stream
        .next()
        .await
        .expect("one stream item")
        .expect("stream ok");
    match item {
        finstack_ai_runtime::ToolStreamItem::Completed(result) => {
            assert!(!result.is_error);
            serde_json::from_slice(result.output.as_bytes()).expect("output json")
        }
        _ => panic!("expected a completed tool result"),
    }
}

#[tokio::test]
async fn resumed_ask_user_returns_the_answer_as_tool_result() {
    let toolset = free_form_toolset();
    let output = completed_output(
        &toolset,
        "ask_user",
        br#"{"prompt":"What is the position limit?","answer":"250k USD"}"#,
    )
    .await;
    assert_eq!(output, serde_json::json!({"answer": "250k USD"}));
}

#[tokio::test]
async fn resumed_typed_tool_returns_the_structured_answer() {
    let toolset = typed_toolset();
    let output = completed_output(
        &toolset,
        "confirm_trade_params",
        br#"{"context":"Buy 100 AAPL @ market.","answer":{"confirmed":true}}"#,
    )
    .await;
    assert_eq!(output, serde_json::json!({"answer": {"confirmed": true}}));
}

#[tokio::test]
async fn ask_user_choice_without_options_is_rejected() {
    let toolset = free_form_toolset();
    let error = toolset
        .call(
            context("tenant-a"),
            validated_call(
                &toolset,
                "ask_user",
                br#"{"prompt":"Which account?","kind":"choice"}"#,
            ),
        )
        .await
        .err()
        .expect("must fail");
    assert_eq!(error.code(), ELICITATION_INVALID_ARGUMENTS);
    assert_eq!(error.category(), ErrorCategory::Validation);
}

#[tokio::test]
async fn ask_user_form_without_schema_is_rejected() {
    let toolset = free_form_toolset();
    let error = toolset
        .call(
            context("tenant-a"),
            validated_call(&toolset, "ask_user", br#"{"prompt":"Fill.","kind":"form"}"#),
        )
        .await
        .err()
        .expect("must fail");
    assert_eq!(error.code(), ELICITATION_INVALID_ARGUMENTS);
}

#[tokio::test]
async fn call_rejects_mismatched_principal_scope() {
    let toolset = free_form_toolset();
    let error = toolset
        .call(
            context("tenant-b"),
            validated_call(&toolset, "ask_user", br#"{"prompt":"hi"}"#),
        )
        .await
        .err()
        .expect("must fail");
    assert_eq!(error.code(), ELICITATION_INVALID_ARGUMENTS);
}

fn typed_toolset() -> ElicitationToolset {
    ElicitationToolset::builder()
        .tool(ElicitationToolDef {
            name: "confirm_trade_params".into(),
            title: "Confirm trade parameters".into(),
            description: "Ask the operator to confirm trade parameters before execution.".into(),
            prompt: "Please confirm the trade parameters.".into(),
            kind: ElicitationKind::Form,
            response_schema: serde_json::json!({
                "type": "object",
                "properties": {"confirmed": {"type": "boolean"}},
                "required": ["confirmed"]
            }),
        })
        .build()
        .expect("toolset")
}

#[tokio::test]
async fn typed_tool_uses_registered_schema_and_prompt() {
    let toolset = typed_toolset();
    let request = parked_request(
        &toolset,
        "confirm_trade_params",
        br#"{"context":"Buy 100 AAPL @ market."}"#,
    )
    .await;
    assert_eq!(*request.kind(), InteractionKind::Form);
    let text = prompt_text(&request);
    assert!(text.contains("Please confirm the trade parameters."));
    assert!(text.contains("Buy 100 AAPL @ market."));
    let schema: serde_json::Value =
        serde_json::from_slice(request.response_schema().as_bytes()).expect("schema json");
    assert_eq!(
        schema["properties"]["answer"]["required"],
        serde_json::json!(["confirmed"])
    );
}

#[tokio::test]
async fn typed_tool_response_schema_is_fixed_at_registration() {
    let toolset = typed_toolset();
    // The model cannot smuggle a different schema through the arguments.
    let request = parked_request(
        &toolset,
        "confirm_trade_params",
        br#"{"context":"x","response_schema":{"type":"object"}}"#,
    )
    .await;
    let schema: serde_json::Value =
        serde_json::from_slice(request.response_schema().as_bytes()).expect("schema json");
    assert_eq!(
        schema["properties"]["answer"]["required"],
        serde_json::json!(["confirmed"])
    );
}

#[test]
fn typed_tool_registration_rejects_invalid_name() {
    let result = ElicitationToolset::builder()
        .tool(ElicitationToolDef {
            name: "not a valid name!".into(),
            title: "Bad".into(),
            description: "Bad.".into(),
            prompt: "Bad?".into(),
            kind: ElicitationKind::FreeText,
            response_schema: serde_json::json!({"type": "object"}),
        })
        .build();
    assert!(matches!(
        result,
        Err(ElicitationError::Configuration { .. })
    ));
}

#[test]
fn builder_without_tools_is_rejected() {
    assert!(matches!(
        ElicitationToolset::builder().build(),
        Err(ElicitationError::Configuration { .. })
    ));
}

#[tokio::test]
async fn error_metadata_round_trips_the_request() {
    let toolset = free_form_toolset();
    let error = toolset
        .call(
            context("tenant-a"),
            validated_call(&toolset, "ask_user", br#"{"prompt":"Round trip?"}"#),
        )
        .await
        .err()
        .expect("must park");
    let via_helper = interaction_request_from_tool_error(&error).expect("helper");
    let via_runtime = error.interaction_request().expect("runtime recovery");
    assert_eq!(via_helper, via_runtime);
}
