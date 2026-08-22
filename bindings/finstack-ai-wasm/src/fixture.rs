//! Scripted request fixtures used by native tests and the browser drive export.

#![allow(dead_code)]

use std::sync::Arc;

use finstack_ai::runtime::ports::model::{
    AuthorizationContext, CancellationSignal, ModelCallContext, ModelName, ModelRequest,
    ModelRequestDraft, ModelRequestLimits, ModelSettings, RunCallContext,
};
use finstack_ai::runtime::ports::tool::ToolCallContext;
use finstack_ai_kernel::{
    ContentBlock, Digest, EffectId, EffectOutputContract, EffectOutputKind, LaneId, Message,
    MessageId, MessageRole, Metadata, OperationLocator, OutputSpec, PrincipalRef, ProviderIds,
    RawJson, RetrySafety, RunId, SessionId, TextBlock, Timestamp, ToolBatchId, ToolCallBlock,
    ToolCallId, ToolExecutionMode, ToolFailurePolicy, ToolId, ValidatedToolCall,
};

const SESSION: &str = "01234567-89ab-7cde-89ab-0123456789ae";
const LANE: &str = "01234567-89ab-7cde-89ab-0123456789af";
const RUN: &str = "01234567-89ab-7cde-89ab-0123456789b0";
const EFFECT: &str = "01234567-89ab-7cde-89ab-0123456789ad";
const MESSAGE: &str = "01234567-89ab-7cde-89ab-0123456789c1";
const REQUEST: &str = "01234567-89ab-7cde-89ab-0123456789c2";
const TOOL_CALL: &str = "01234567-89ab-7cde-89ab-0123456789c3";
const TOOL_BATCH: &str = "01234567-89ab-7cde-89ab-0123456789c4";

/// Shared run-call context for scripted port drives.
///
/// # Errors
///
/// Returns a static reason when a fixture identifier is invalid.
pub fn run_call_context(cancellation: CancellationSignal) -> Result<RunCallContext, &'static str> {
    Ok(RunCallContext {
        locator: OperationLocator::try_new(
            "tenant-a",
            SessionId::parse(SESSION).map_err(|_| "session")?,
            LaneId::parse(LANE).map_err(|_| "lane")?,
            RunId::parse(RUN).map_err(|_| "run")?,
        )
        .map_err(|_| "locator")?,
        authorization: AuthorizationContext {
            principal: PrincipalRef::try_new("https://issuer.example", "user-1", Some("tenant-a"))
                .map_err(|_| "principal")?,
            authentication_method: Arc::from("test"),
            assurance_level: Arc::from("aal1"),
            roles: Arc::from([]),
            permitted_scopes: Arc::from([]),
            safe_claims: Metadata::empty(),
            policy_version: Arc::from("policy-v1"),
            decision_id: Arc::from("decision-1"),
        },
        effect_id: EffectId::parse(EFFECT).map_err(|_| "effect")?,
        attempt: 1,
        deadline: None,
        budget_scope_id: None,
        cancellation,
        relation_depth: 0,
    })
}

/// One user-message model request for scripted `Model::request` drives.
///
/// # Errors
///
/// Returns a static reason when the fixture draft cannot be constructed.
pub fn model_request(
    model: &ModelName,
    cancellation: CancellationSignal,
) -> Result<ModelRequest, &'static str> {
    let created_at = Timestamp::from_unix_ms(1_704_067_200_000).map_err(|_| "timestamp")?;
    let message = Message::try_new(
        MessageId::parse(MESSAGE).map_err(|_| "message")?,
        MessageRole::User,
        vec![ContentBlock::Text(
            TextBlock::try_new("hello").map_err(|_| "text")?,
        )],
        created_at,
        None,
        ProviderIds::empty(),
        Metadata::empty(),
    )
    .map_err(|_| "message")?;
    Ok(ModelRequest {
        call: ModelCallContext {
            run: run_call_context(cancellation)?,
            request_id: finstack_ai_kernel::ModelRequestId::parse(REQUEST)
                .map_err(|_| "request")?,
        },
        draft: ModelRequestDraft {
            model: model.clone(),
            messages: Arc::from([message]),
            tools: Arc::from([]),
            output: OutputSpec::PlainText,
            settings: ModelSettings {
                values: RawJson::parse(b"{}").map_err(|_| "settings")?,
            },
            limits: ModelRequestLimits {
                max_input_bytes: 1_048_576,
                max_input_tokens: 8_192,
                max_output_tokens: 1_024,
            },
        },
        continuation_state: None,
    })
}

/// One validated echo tool call for scripted `Toolset::call` drives.
///
/// # Errors
///
/// Returns a static reason when the fixture call cannot be constructed.
pub fn tool_call(
    cancellation: CancellationSignal,
) -> Result<(ToolCallContext, ValidatedToolCall), &'static str> {
    let ctx = ToolCallContext {
        run: run_call_context(cancellation)?,
        tool_batch_id: ToolBatchId::parse(TOOL_BATCH).map_err(|_| "batch")?,
        tool_call_id: ToolCallId::parse(TOOL_CALL).map_err(|_| "call")?,
    };
    let call = ValidatedToolCall {
        call: ToolCallBlock::try_new(
            ToolCallId::parse(TOOL_CALL).map_err(|_| "call")?,
            "echo",
            RawJson::parse(br#"{"value":"hi"}"#).map_err(|_| "arguments")?,
        )
        .map_err(|_| "block")?,
        tool_id: ToolId::parse("js.echo").map_err(|_| "tool")?,
        component: None,
        output_contract: EffectOutputContract {
            kind: EffectOutputKind::ToolResult,
            schema_version: 1,
            schema_digest: Digest::raw_json(b"{}"),
        },
        retry_safety: RetrySafety::SafeToRetry,
        deadline: None,
        execution: ToolExecutionMode::Sequential,
        failure_policy: ToolFailurePolicy::ReturnToModel,
    };
    Ok((ctx, call))
}

/// Default echo tool schema used by scripted toolset constructors.
#[must_use]
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub fn echo_tool_json() -> &'static str {
    r#"{
        "id": "js.echo",
        "model_name": "echo",
        "title": "Echo",
        "description": "Echo one validated string value.",
        "input_schema": {
            "additionalProperties": false,
            "properties": {"value": {"type": "string"}},
            "required": ["value"],
            "type": "object"
        },
        "output_schema": {
            "additionalProperties": false,
            "properties": {"value": {"type": "string"}},
            "required": ["value"],
            "type": "object"
        },
        "execution": "sequential",
        "side_effect": "read_only",
        "retry_safety": "safe_to_retry",
        "approval": {
            "requirement": "not_required",
            "reason": null,
            "attributes": {}
        },
        "max_result_bytes": 4096,
        "metadata": {}
    }"#
}
