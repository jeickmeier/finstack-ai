//! Codex toolset: start / status / cancel over the frozen exec invoker.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use finstack_ai_kernel::{
    BudgetRequest, ChildPlacement, ChildRunLocator, ContentBlock, Digest, ErrorCategory, LaneTag,
    Metadata, OperationLocator, RawJson, RemoteRouteRef, RetrySafety, RunTag, SessionTag,
    TextBlock, ToolExecutionMode, ToolId, ValidatedToolCall,
};
use finstack_ai_runtime::{
    AGENT_INVOKE_INVALID_ACCEPTANCE, AgentInvokeError, AgentInvoker, AgentRef, ApprovalMetadata,
    ApprovalRequirement, ChildRunContext, ChildRunHandle, ChildRunRequest, IdGenerationError,
    OsRandomSource, PortFuture, SideEffectClass, SystemClock, ToolCallContext, ToolDeferralSupport,
    ToolError, ToolEventStream, ToolResult, ToolSpec, ToolStreamItem, Toolset, ToolsetDescriptor,
    UuidV7Generator, verify_authority,
};
use futures_util::stream;
use serde::Deserialize;

use crate::identity::{codex_agent_ref, codex_route_ref};
use crate::invoker::CodexChildInvoker;
use crate::{CODEX_CHILD_NOT_FOUND, CODEX_INVALID_ARGUMENTS, CodexChildError};

const START_ID: &str = "finstack.tools.codex.start";
const STATUS_ID: &str = "finstack.tools.codex.status";
const CANCEL_ID: &str = "finstack.tools.codex.cancel";
const START_NAME: &str = "codex_start";
const STATUS_NAME: &str = "codex_status";
const CANCEL_NAME: &str = "codex_cancel";
const MAX_MESSAGE_BYTES: usize = 4_096;
const APPROVAL_REASON: &str = "codex child invocation edits the frozen workspace unless the host already authorized ChildRunPolicy::Allow";

/// Codex toolset over one frozen [`CodexChildInvoker`]. The prompt is the
/// only model-supplied input; binary, workspace, and sandbox are frozen.
pub struct CodexToolset {
    descriptor: ToolsetDescriptor,
    tools: Arc<[ToolSpec]>,
    invoker: Arc<CodexChildInvoker>,
    agent: AgentRef,
    remote: RemoteRouteRef,
    children: Arc<Mutex<BTreeMap<ChildKey, StartedChild>>>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ChildKey {
    tenant_scope: Arc<str>,
    session_id: Arc<str>,
    run_id: Arc<str>,
}

#[derive(Clone)]
struct StartedChild {
    handle: ChildRunHandle,
}

impl CodexToolset {
    /// Construct the toolset over a frozen invoker.
    ///
    /// # Errors
    ///
    /// Returns [`CodexChildError::Configuration`] when a checked-in tool
    /// specification or the peer identity cannot be built.
    pub fn try_new(invoker: Arc<CodexChildInvoker>) -> Result<Self, CodexChildError> {
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
            children: Arc::new(Mutex::new(BTreeMap::new())),
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
        let agent = self.agent.clone();
        let remote = self.remote.clone();
        let table = Arc::clone(&self.children);
        Box::pin(async move {
            verify_authority(&ctx)?;
            let name = call.call.tool_name();
            if name != START_NAME && name != STATUS_NAME && name != CANCEL_NAME {
                return Err(tool_error(
                    CODEX_INVALID_ARGUMENTS,
                    ErrorCategory::Validation,
                    "codex call identity is invalid",
                ));
            }
            let result = if name == START_NAME {
                start_child(&invoker, &agent, remote, &ctx, &call).await
            } else {
                // Only status/cancel read the table; start never does.
                let snapshot = table.lock().map(|guard| guard.clone()).unwrap_or_default();
                if name == STATUS_NAME {
                    status_child(&invoker, &snapshot, &ctx, &call)
                } else {
                    cancel_child(&invoker, &snapshot, &ctx, &call).await
                }
            }?;
            if let Some(started) = result.started
                && let Ok(mut children) = table.lock()
            {
                children.insert(started.key, started.child);
            }
            if let Some(key) = result.forget
                && let Ok(mut children) = table.lock()
            {
                children.remove(&key);
            }
            Ok(completed(result.output, result.is_error))
        })
    }
}

struct CallOutcome {
    output: RawJson,
    is_error: bool,
    started: Option<StartedRecord>,
    // A child the table should forget: its run was evicted from the
    // invoker (status `unknown`), so later lookups would never improve.
    forget: Option<ChildKey>,
}

struct StartedRecord {
    key: ChildKey,
    child: StartedChild,
}

async fn start_child(
    invoker: &Arc<CodexChildInvoker>,
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
    let locator = child_locator(&ctx.run.locator, remote).map_err(|_| {
        tool_error(
            CODEX_INVALID_ARGUMENTS,
            ErrorCategory::Internal,
            "codex locator allocation failed",
        )
    })?;
    let input: Arc<[ContentBlock]> = Arc::from([ContentBlock::Text(
        TextBlock::try_new(arguments.prompt.as_ref()).map_err(|_| {
            tool_error(
                CODEX_INVALID_ARGUMENTS,
                ErrorCategory::Validation,
                "codex prompt is invalid",
            )
        })?,
    )]);
    let request_digest = request_digest(agent, &input, &locator)?;
    let request = ChildRunRequest {
        agent: agent.clone(),
        input,
        placement: ChildPlacement::RemoteChildSession,
        locator,
        requested_deadline: ctx.run.deadline,
        requested_budget: BudgetRequest::default(),
        delegation_id: None,
        metadata: Metadata::empty(),
        request_digest,
    };
    if let Err(error) = request.validate() {
        return Ok(invoke_error_result(&error));
    }
    let context = ChildRunContext {
        parent: ctx.run.locator.clone(),
        parent_effect_id: ctx.run.effect_id,
        authorization: ctx.run.authorization.clone(),
    };
    match AgentInvoker::start_or_attach(invoker.as_ref(), context, request).await {
        Ok(handle) => {
            let run_id = Arc::<str>::from(handle.locator.operation.run_id.to_string());
            let key = ChildKey {
                tenant_scope: Arc::clone(&handle.locator.operation.tenant_scope),
                session_id: Arc::<str>::from(handle.locator.operation.session_id.to_string()),
                run_id: Arc::clone(&run_id),
            };
            let output = result_json(&serde_json::json!({
                "run_id": run_id.as_ref(),
                "session_id": key.session_id.as_ref(),
                "status": "accepted",
            }))?;
            Ok(CallOutcome {
                output,
                is_error: false,
                started: Some(StartedRecord {
                    key,
                    child: StartedChild { handle },
                }),
                forget: None,
            })
        }
        Err(error) => Ok(invoke_error_result(&error)),
    }
}

fn status_child(
    invoker: &Arc<CodexChildInvoker>,
    children: &BTreeMap<ChildKey, StartedChild>,
    ctx: &ToolCallContext,
    call: &ValidatedToolCall,
) -> Result<CallOutcome, ToolError> {
    let arguments: RunIdArguments = parse_args(call)?;
    let Some(run_id) = arguments.run_id.as_ref() else {
        return Ok(error_result(
            CODEX_INVALID_ARGUMENTS,
            "status requires run_id",
        ));
    };
    let Some((key, child)) = lookup_child(children, ctx.run.locator.tenant_scope.as_ref(), run_id)
    else {
        return Ok(error_result(
            CODEX_CHILD_NOT_FOUND,
            "no started child matches run_id",
        ));
    };
    let Some(report) = invoker.run_status(&child.handle.locator.operation.run_id) else {
        let output = result_json(&serde_json::json!({
            "run_id": key.run_id.as_ref(),
            "status": "unknown",
        }))?;
        // The invoker no longer knows this run (host restart or eviction
        // of a settled run); later lookups can never improve, so the
        // table forgets it instead of growing without bound.
        return Ok(CallOutcome {
            output,
            is_error: false,
            started: None,
            forget: Some(key.clone()),
        });
    };
    let last_message = report
        .last_message
        .as_deref()
        .map(|text| truncate_message(text, MAX_MESSAGE_BYTES));
    // Already bounded to STDERR_TAIL_BYTES at write time in `append_stderr`.
    let stderr_tail = report.stderr_tail.as_deref();
    let output = result_json(&serde_json::json!({
        "run_id": key.run_id.as_ref(),
        "status": status_name(report.status),
        "thread_id": report.thread_id,
        "last_message": last_message,
        "usage": report.usage,
        "exit_code": report.exit_code,
        "stderr_tail": stderr_tail,
    }))?;
    Ok(CallOutcome {
        output,
        is_error: false,
        started: None,
        forget: None,
    })
}

fn lookup_child<'a>(
    children: &'a BTreeMap<ChildKey, StartedChild>,
    tenant_scope: &str,
    run_id: &str,
) -> Option<(&'a ChildKey, &'a StartedChild)> {
    children
        .iter()
        .find(|(key, _)| key.tenant_scope.as_ref() == tenant_scope && key.run_id.as_ref() == run_id)
}

async fn cancel_child(
    invoker: &Arc<CodexChildInvoker>,
    children: &BTreeMap<ChildKey, StartedChild>,
    ctx: &ToolCallContext,
    call: &ValidatedToolCall,
) -> Result<CallOutcome, ToolError> {
    let arguments: RunIdArguments = parse_args(call)?;
    let Some(run_id) = arguments.run_id.as_ref() else {
        return Ok(error_result(
            CODEX_INVALID_ARGUMENTS,
            "cancel requires run_id",
        ));
    };
    let Some((key, child)) = lookup_child(children, ctx.run.locator.tenant_scope.as_ref(), run_id)
    else {
        return Ok(error_result(
            CODEX_CHILD_NOT_FOUND,
            "no started child matches run_id",
        ));
    };
    let run_id = &key.run_id;
    if let Err(error) = AgentInvoker::cancel(invoker.as_ref(), &child.handle.locator).await {
        return Ok(invoke_error_result(&error));
    }
    let output = result_json(&serde_json::json!({
        "run_id": run_id.as_ref(),
        "cancelled": true,
        "placement": "remote_child_session",
    }))?;
    Ok(CallOutcome {
        output,
        is_error: false,
        started: None,
        forget: None,
    })
}

fn child_locator(
    parent: &OperationLocator,
    remote: RemoteRouteRef,
) -> Result<ChildRunLocator, IdGenerationError> {
    let generator = UuidV7Generator::new(SystemClock, OsRandomSource);
    let run_id = generator.generate::<RunTag>()?;
    let lane_id = generator.generate::<LaneTag>()?;
    let session_id = generator.generate::<SessionTag>()?;
    Ok(ChildRunLocator {
        operation: OperationLocator::try_new(
            parent.tenant_scope.as_ref(),
            session_id,
            lane_id,
            run_id,
        )
        .map_err(|_| IdGenerationError::Source("codex child locator is invalid".into()))?,
        remote: Some(remote),
    })
}

fn request_digest(
    agent: &AgentRef,
    input: &[ContentBlock],
    locator: &ChildRunLocator,
) -> Result<Digest, ToolError> {
    let canonical = serde_json::to_vec(&serde_json::json!({
        "agent_id": agent.id.to_string(),
        "input": input.iter().map(content_text).collect::<Vec<_>>(),
        "placement": "remote_child_session",
        "run_id": locator.operation.run_id.to_string(),
    }))
    .map_err(|_| {
        tool_error(
            CODEX_INVALID_ARGUMENTS,
            ErrorCategory::Internal,
            "codex request digest serialization failed",
        )
    })?;
    Digest::domain_separated("child-run-request", 1, &canonical).map_err(|_| {
        tool_error(
            CODEX_INVALID_ARGUMENTS,
            ErrorCategory::Internal,
            "codex request digest failed",
        )
    })
}

fn content_text(block: &ContentBlock) -> String {
    match block {
        ContentBlock::Text(text) => text.text().to_string(),
        _ => String::new(),
    }
}

fn status_name(status: crate::state::CodexRunStatus) -> &'static str {
    match status {
        crate::state::CodexRunStatus::Running => "running",
        crate::state::CodexRunStatus::Completed => "completed",
        crate::state::CodexRunStatus::Failed => "failed",
        crate::state::CodexRunStatus::Cancelled => "cancelled",
    }
}

fn truncate_message(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text.get(..end).unwrap_or(text)
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

fn error_result(code: &'static str, message: &'static str) -> CallOutcome {
    CallOutcome {
        output: result_json(&serde_json::json!({ "code": code, "message": message }))
            .unwrap_or_else(|_| Metadata::empty().as_raw_json().clone()),
        is_error: true,
        started: None,
        forget: None,
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
