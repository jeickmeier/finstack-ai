//! Codex toolset: start / status / cancel over the frozen exec invoker.

use std::sync::Arc;

use finstack_ai_kernel::{
    BudgetRequest, ChildPlacement, ChildRunLocator, ContentBlock, ErrorCategory, Metadata, RawJson,
    RemoteRouteRef, RetrySafety, RunId, TextBlock, ToolExecutionMode, ToolId, ValidatedToolCall,
};
use finstack_ai_runtime::child::{
    AGENT_INVOKE_INVALID_ACCEPTANCE, AgentInvokeError, AgentRef, ChildRunStartRequest,
    ChildRunStarter,
};
use finstack_ai_runtime::ports::PortFuture;
use finstack_ai_runtime::ports::model::{
    ApprovalMetadata, ApprovalRequirement, SideEffectClass, ToolDeferralSupport, ToolSpec,
};
use finstack_ai_runtime::ports::tool::{
    ToolCallContext, ToolError, ToolEventStream, ToolResult, ToolStreamItem, Toolset,
    ToolsetDescriptor, verify_authority,
};
use futures_util::stream;
use serde::Deserialize;

use crate::identity::{codex_agent_ref, codex_route_ref};
use crate::invoker::CodexChildInvoker;
use crate::state::CodexRunStatus;
use crate::{CODEX_CHILD_NOT_FOUND, CODEX_INVALID_ARGUMENTS, CodexChildError};

const START_ID: &str = "finstack.tools.codex.start";
const STATUS_ID: &str = "finstack.tools.codex.status";
const CANCEL_ID: &str = "finstack.tools.codex.cancel";
const START_NAME: &str = "codex_start";
const STATUS_NAME: &str = "codex_status";
const CANCEL_NAME: &str = "codex_cancel";
const APPROVAL_REASON: &str = "codex child invocation edits the frozen workspace unless the host already authorized ChildRunPolicy::Allow";

/// Codex toolset over one frozen [`CodexChildInvoker`]. The prompt is the
/// only model-supplied input; binary, workspace, and sandbox are frozen.
pub struct CodexToolset {
    descriptor: ToolsetDescriptor,
    tools: Arc<[ToolSpec]>,
    invoker: Arc<CodexChildInvoker>,
    agent: AgentRef,
    remote: RemoteRouteRef,
    starter: Arc<ChildRunStarter>,
}

impl CodexToolset {
    /// Construct the toolset over one host-bound starter and frozen invoker.
    ///
    /// # Errors
    ///
    /// Returns [`CodexChildError::Configuration`] when a checked-in tool
    /// specification or the peer identity cannot be built.
    pub fn try_new(
        starter: Arc<ChildRunStarter>,
        invoker: Arc<CodexChildInvoker>,
    ) -> Result<Self, CodexChildError> {
        let agent = codex_agent_ref()?;
        let remote = codex_route_ref()?;
        let tools = Arc::from([
            tool_spec(
                START_ID,
                START_NAME,
                "Delegate one coding task to the Codex child agent in the frozen workspace.",
                br#"{"additionalProperties":false,"properties":{"prompt":{"type":"string"}},"required":["prompt"],"type":"object"}"#,
            )?,
            tool_spec(
                STATUS_ID,
                STATUS_NAME,
                "Report status for one previously started Codex run without waiting.",
                br#"{"additionalProperties":false,"properties":{"run_id":{"type":"string"}},"required":["run_id"],"type":"object"}"#,
            )?,
            tool_spec(
                CANCEL_ID,
                CANCEL_NAME,
                "Cancel a Codex child run started by this toolset.",
                br#"{"additionalProperties":false,"properties":{"run_id":{"type":"string"}},"required":["run_id"],"type":"object"}"#,
            )?,
        ]);
        Ok(Self {
            descriptor: ToolsetDescriptor {
                name: Arc::from("finstack-codex"),
                metadata: Metadata::empty(),
            },
            tools,
            invoker,
            agent,
            remote,
            starter,
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StartArguments {
    prompt: Arc<str>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RunIdArguments {
    #[serde(default)]
    run_id: Option<Arc<str>>,
}

impl Toolset for CodexToolset {
    fn descriptor(&self) -> ToolsetDescriptor {
        self.descriptor.clone()
    }

    fn tools(&self) -> Arc<[ToolSpec]> {
        Arc::clone(&self.tools)
    }

    fn call(
        &self,
        ctx: ToolCallContext,
        call: ValidatedToolCall,
    ) -> PortFuture<Result<ToolEventStream, ToolError>> {
        let invoker = Arc::clone(&self.invoker);
        let starter = Arc::clone(&self.starter);
        let agent = self.agent.clone();
        let remote = self.remote.clone();
        Box::pin(async move {
            verify_authority(&ctx)?;
            let result = match call.call.tool_name() {
                START_NAME => Box::pin(start_child(&starter, &agent, remote, &ctx, &call)).await,
                STATUS_NAME => status_child(&invoker, &ctx, &call),
                CANCEL_NAME => cancel_child(&starter, &invoker, &ctx, &call).await,
                _ => {
                    return Err(tool_error(
                        CODEX_INVALID_ARGUMENTS,
                        ErrorCategory::Validation,
                        "codex call identity is invalid",
                    ));
                }
            }?;
            Ok(completed(result.output, result.is_error))
        })
    }
}

struct CallOutcome {
    output: RawJson,
    is_error: bool,
}

async fn start_child(
    starter: &Arc<ChildRunStarter>,
    agent: &AgentRef,
    remote: RemoteRouteRef,
    ctx: &ToolCallContext,
    call: &ValidatedToolCall,
) -> Result<CallOutcome, ToolError> {
    let arguments: StartArguments = parse_args(call)?;
    if arguments.prompt.trim().is_empty() {
        return Ok(error_result(
            CODEX_INVALID_ARGUMENTS,
            "prompt must not be blank",
        ));
    }
    let input: Arc<[ContentBlock]> = Arc::from([ContentBlock::Text(
        TextBlock::try_new(arguments.prompt.as_ref()).map_err(|_| {
            tool_error(
                CODEX_INVALID_ARGUMENTS,
                ErrorCategory::Validation,
                "codex prompt is invalid",
            )
        })?,
    )]);
    let request = ChildRunStartRequest {
        agent: agent.clone(),
        input,
        placement: ChildPlacement::RemoteChildSession,
        remote: Some(remote),
        requested_deadline: ctx.run.deadline,
        requested_budget: BudgetRequest::default(),
        delegation_id: None,
        metadata: Metadata::empty(),
    };
    match Box::pin(starter.start_or_attach(ctx, request)).await {
        Ok(handle) => ok_result(&serde_json::json!({
            "run_id": handle.locator.operation.run_id.to_string(),
            "session_id": handle.locator.operation.session_id.to_string(),
            "status": "accepted",
        })),
        Err(error) => Ok(invoke_error_result(&error)),
    }
}

fn status_child(
    invoker: &Arc<CodexChildInvoker>,
    ctx: &ToolCallContext,
    call: &ValidatedToolCall,
) -> Result<CallOutcome, ToolError> {
    let (_, locator) = match accepted_child(invoker, ctx, call, "status requires run_id")? {
        Ok(found) => found,
        Err(outcome) => return Ok(outcome),
    };
    let run_id = locator.operation.run_id.to_string();
    let Some(report) = invoker.run_status(&locator) else {
        return ok_result(&serde_json::json!({
            "run_id": run_id,
            "status": "unknown",
        }));
    };
    // `last_message` and `stderr_tail` are already bounded by the run-state
    // reducer at write time.
    ok_result(&serde_json::json!({
        "run_id": run_id,
        "status": status_name(report.status),
        "thread_id": report.thread_id,
        "last_message": report.last_message,
        "failure_message": report.failure_message,
        "usage": report.usage,
        "exit_code": report.exit_code,
        "stderr_tail": report.stderr_tail,
    }))
}

async fn cancel_child(
    starter: &Arc<ChildRunStarter>,
    invoker: &Arc<CodexChildInvoker>,
    ctx: &ToolCallContext,
    call: &ValidatedToolCall,
) -> Result<CallOutcome, ToolError> {
    let (run_id, locator) = match accepted_child(invoker, ctx, call, "cancel requires run_id")? {
        Ok(found) => found,
        Err(outcome) => return Ok(outcome),
    };
    if let Err(error) = starter.cancel(&locator).await {
        return Ok(invoke_error_result(&error));
    }
    ok_result(&serde_json::json!({
        "run_id": run_id.as_ref(),
        "cancelled": true,
        "placement": "remote_child_session",
    }))
}

/// Resolve the `run_id` argument (returned verbatim) to the locator of a
/// child this parent operation started.
///
/// The outer `Err` is a malformed call; the inner `Err` is the tool-visible
/// error result for a missing argument or an unknown child.
fn accepted_child(
    invoker: &CodexChildInvoker,
    ctx: &ToolCallContext,
    call: &ValidatedToolCall,
    missing: &'static str,
) -> Result<Result<(Arc<str>, ChildRunLocator), CallOutcome>, ToolError> {
    let arguments: RunIdArguments = parse_args(call)?;
    let Some(run_id) = arguments.run_id else {
        return Ok(Err(error_result(CODEX_INVALID_ARGUMENTS, missing)));
    };
    let found = RunId::parse(run_id.as_ref())
        .ok()
        .and_then(|parsed| invoker.accepted_locator(&ctx.run.locator, &parsed))
        .map(|locator| (run_id, locator))
        .ok_or_else(|| error_result(CODEX_CHILD_NOT_FOUND, "no started child matches run_id"));
    Ok(found)
}

fn status_name(status: CodexRunStatus) -> &'static str {
    match status {
        CodexRunStatus::Running => "running",
        CodexRunStatus::Completed => "completed",
        CodexRunStatus::Failed => "failed",
        CodexRunStatus::Cancelled => "cancelled",
    }
}

fn parse_args<T: for<'de> Deserialize<'de>>(call: &ValidatedToolCall) -> Result<T, ToolError> {
    serde_json::from_slice(call.call.arguments().as_bytes()).map_err(|_| {
        tool_error(
            CODEX_INVALID_ARGUMENTS,
            ErrorCategory::Validation,
            "codex arguments are invalid",
        )
    })
}

fn invoke_error_result(error: &AgentInvokeError) -> CallOutcome {
    let code = error.code();
    error_result(
        if code.is_empty() {
            AGENT_INVOKE_INVALID_ACCEPTANCE
        } else {
            code
        },
        "child invocation was rejected",
    )
}

fn ok_result(value: &serde_json::Value) -> Result<CallOutcome, ToolError> {
    Ok(CallOutcome {
        output: result_json(value)?,
        is_error: false,
    })
}

fn error_result(code: &'static str, message: &'static str) -> CallOutcome {
    CallOutcome {
        output: result_json(&serde_json::json!({ "code": code, "message": message }))
            .unwrap_or_else(|_| Metadata::empty().as_raw_json().clone()),
        is_error: true,
    }
}

fn result_json(value: &serde_json::Value) -> Result<RawJson, ToolError> {
    let bytes = serde_json::to_vec(value).map_err(|_| {
        tool_error(
            CODEX_INVALID_ARGUMENTS,
            ErrorCategory::Internal,
            "codex result serialization failed",
        )
    })?;
    RawJson::parse(bytes).map_err(|_| {
        tool_error(
            CODEX_INVALID_ARGUMENTS,
            ErrorCategory::Internal,
            "codex result normalization failed",
        )
    })
}

fn completed(output: RawJson, is_error: bool) -> ToolEventStream {
    Box::pin(stream::once(async move {
        Ok(ToolStreamItem::Completed(ToolResult { output, is_error }))
    }))
}

fn tool_spec(
    id: &'static str,
    name: &'static str,
    description: &'static str,
    schema: &'static [u8],
) -> Result<ToolSpec, CodexChildError> {
    let spec = ToolSpec {
        id: ToolId::parse(id).map_err(|_| CodexChildError::Configuration {
            reason: "invalid_tool_id",
        })?,
        model_name: Arc::from(name),
        title: Arc::from(name),
        description: Arc::from(description),
        input_schema: RawJson::parse(schema).map_err(|_| CodexChildError::Configuration {
            reason: "invalid_input_schema",
        })?,
        output_schema: None,
        execution: ToolExecutionMode::Sequential,
        side_effect: SideEffectClass::NonIdempotentWrite,
        retry_safety: RetrySafety::AtMostOnce,
        approval: ApprovalMetadata {
            requirement: ApprovalRequirement::Required,
            reason: Some(Arc::from(APPROVAL_REASON)),
            attributes: Metadata::empty(),
        },
        max_result_bytes: 8_192,
        metadata: Metadata::empty(),
        deferral: ToolDeferralSupport::Never,
    };
    spec.validate()
        .map_err(|_| CodexChildError::Configuration {
            reason: "invalid_tool_spec",
        })?;
    Ok(spec)
}

fn tool_error(code: &'static str, category: ErrorCategory, message: &'static str) -> ToolError {
    ToolError::try_new(code, category, false, message, Metadata::empty()).unwrap_or_else(Into::into)
}
