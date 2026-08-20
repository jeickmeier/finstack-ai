//! Subagent Toolset over the host [`AgentInvoker`].
//!
//! The crate holds no invocation authority. Child-run policy stays a runtime
//! concern; policy, depth, and budget failures surface as tool results.

#![warn(missing_docs)]
#![forbid(unsafe_code)]
#![warn(clippy::float_cmp)]
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
#![deny(clippy::panic)]
#![deny(clippy::unreachable)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::indexing_slicing,
        clippy::float_cmp,
    )
)]
// Allow expect() in doc tests (they are test code)
#![doc(test(attr(allow(clippy::expect_used))))]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use finstack_ai_kernel::{
    AgentId, BudgetRequest, ChildPlacement, ChildRunLocator, ContentBlock, Digest, ErrorCategory,
    Metadata, OperationLocator, RawJson, RetrySafety, TextBlock, ToolExecutionMode, ToolId,
    ValidatedToolCall,
};
use finstack_ai_kernel::{LaneTag, RunTag, SessionTag};
use finstack_ai_runtime::{
    AGENT_INVOKE_INVALID_ACCEPTANCE, AgentInvokeError, AgentInvoker, AgentRef, ApprovalMetadata,
    ApprovalRequirement, ChildRunContext, ChildRunHandle, ChildRunRequest, IdGenerationError,
    OsRandomSource, PortFuture, SideEffectClass, SystemClock, ToolCallContext, ToolDeferralSupport,
    ToolError, ToolEventStream, ToolResult, ToolSpec, ToolStreamItem, Toolset, ToolsetDescriptor,
    UuidV7Generator, verify_authority,
};
use futures_util::stream;
use serde::Deserialize;
use thiserror::Error;

#[cfg(test)]
mod tests;

const START_ID: &str = "finstack.tools.subagent.start";
const STATUS_ID: &str = "finstack.tools.subagent.status";
const CANCEL_ID: &str = "finstack.tools.subagent.cancel";
const START_NAME: &str = "subagent_start";
const STATUS_NAME: &str = "subagent_status";
const CANCEL_NAME: &str = "subagent_cancel";

/// Stable constructor failure when the allow-list is empty or a spec is invalid.
pub const SUBAGENT_CONFIGURATION_INVALID: &str = "subagent_configuration_invalid";
/// Model-supplied agent id is not on the frozen allow-list.
pub const SUBAGENT_AGENT_NOT_ALLOWED: &str = "subagent_agent_not_allowed";
/// Tool arguments failed validation.
pub const SUBAGENT_INVALID_ARGUMENTS: &str = "subagent_invalid_arguments";
/// Named child is not in the in-process start table.
pub const SUBAGENT_CHILD_NOT_FOUND: &str = "subagent_child_not_found";
/// Construction failure for [`SubagentToolset`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SubagentError {
    /// Empty allow-list or invalid public specification.
    #[error("{SUBAGENT_CONFIGURATION_INVALID}: {reason}")]
    Configuration {
        /// Stable non-secret reason.
        reason: &'static str,
    },
}

/// Host-invoker subagent toolset with a frozen agent allow-list.
pub struct SubagentToolset {
    descriptor: ToolsetDescriptor,
    tools: Arc<[ToolSpec]>,
    invoker: Arc<dyn AgentInvoker>,
    allow_list: Arc<[AgentRef]>,
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
    placement: ChildPlacement,
}

impl SubagentToolset {
    /// Construct the toolset over a host invoker and a non-empty allow-list.
    ///
    /// # Errors
    ///
    /// Returns [`SubagentError::Configuration`] when the allow-list is empty or
    /// a checked-in tool specification cannot be built.
    pub fn try_new(
        invoker: Arc<dyn AgentInvoker>,
        allow_list: impl Into<Arc<[AgentRef]>>,
    ) -> Result<Self, SubagentError> {
        let allow_list = allow_list.into();
        if allow_list.is_empty() {
            return Err(SubagentError::Configuration {
                reason: "empty_allow_list",
            });
        }
        let tools = Arc::from([
            tool_spec(
                START_ID,
                START_NAME,
                "Start a child agent from the frozen allow-list.",
                br#"{"additionalProperties":false,"properties":{"agent_id":{"type":"string"},"input":{"type":"string"},"placement":{"enum":["compatible_lane_in_parent_session","isolated_child_session",null],"type":["string","null"]}},"required":["agent_id","input","placement"],"type":"object"}"#,
            )?,
            tool_spec(
                STATUS_ID,
                STATUS_NAME,
                "Report status for one previously started child run without waiting.",
                br#"{"additionalProperties":false,"properties":{"run_id":{"type":"string"}},"required":["run_id"],"type":"object"}"#,
            )?,
            tool_spec(
                CANCEL_ID,
                CANCEL_NAME,
                "Cancel a compatible-lane or isolated child started by this toolset.",
                br#"{"additionalProperties":false,"properties":{"run_id":{"type":"string"}},"required":["run_id"],"type":"object"}"#,
            )?,
        ]);
        Ok(Self {
            descriptor: ToolsetDescriptor {
                name: Arc::from("finstack-subagent"),
                metadata: Metadata::empty(),
            },
            tools,
            invoker,
            allow_list,
            children: Arc::new(Mutex::new(BTreeMap::new())),
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StartArguments {
    agent_id: Arc<str>,
    input: Arc<str>,
    #[serde(default)]
    placement: Option<ChildPlacement>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RunIdArguments {
    #[serde(default)]
    run_id: Option<Arc<str>>,
}

impl Toolset for SubagentToolset {
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
        let allow_list = Arc::clone(&self.allow_list);
        let table = Arc::clone(&self.children);
        Box::pin(async move {
            verify_authority(&ctx)?;
            let name = call.call.tool_name();
            if name != START_NAME && name != STATUS_NAME && name != CANCEL_NAME {
                return Err(tool_error(
                    SUBAGENT_INVALID_ARGUMENTS,
                    ErrorCategory::Validation,
                    "subagent call identity is invalid",
                ));
            }
            let snapshot = table.lock().map(|guard| guard.clone()).unwrap_or_default();
            let result = if name == START_NAME {
                start_child(&invoker, &allow_list, &ctx, &call).await
            } else if name == STATUS_NAME {
                status_child(&snapshot, &ctx, &call)
            } else {
                cancel_child(&invoker, &snapshot, &ctx, &call).await
            }?;
            if let Some(started) = result.started
                && let Ok(mut children) = table.lock()
            {
                children.insert(started.key, started.child);
            }
            Ok(completed(result.output, result.is_error))
        })
    }
}

struct CallOutcome {
    output: RawJson,
    is_error: bool,
    started: Option<StartedRecord>,
}

struct StartedRecord {
    key: ChildKey,
    child: StartedChild,
}

async fn start_child(
    invoker: &Arc<dyn AgentInvoker>,
    allow_list: &[AgentRef],
    ctx: &ToolCallContext,
    call: &ValidatedToolCall,
) -> Result<CallOutcome, ToolError> {
    let arguments: StartArguments = parse_args(call)?;
    let agent_id = AgentId::parse(arguments.agent_id.as_ref()).map_err(|_| {
        tool_error(
            SUBAGENT_INVALID_ARGUMENTS,
            ErrorCategory::Validation,
            "subagent agent_id is invalid",
        )
    })?;
    let Some(agent) = allow_list.iter().find(|allowed| allowed.id == agent_id) else {
        return Ok(error_result(
            SUBAGENT_AGENT_NOT_ALLOWED,
            "agent is not on the subagent allow-list",
        ));
    };
    let placement = arguments
        .placement
        .unwrap_or(ChildPlacement::IsolatedChildSession);
    if matches!(placement, ChildPlacement::RemoteChildSession) {
        return Ok(error_result(
            SUBAGENT_INVALID_ARGUMENTS,
            "remote child placement is not started by this toolset",
        ));
    }
    let locator = child_locator(&ctx.run.locator, placement).map_err(|_| {
        tool_error(
            SUBAGENT_INVALID_ARGUMENTS,
            ErrorCategory::Internal,
            "subagent locator allocation failed",
        )
    })?;
    let input = Arc::from([ContentBlock::Text(
        TextBlock::try_new(arguments.input.as_ref()).map_err(|_| {
            tool_error(
                SUBAGENT_INVALID_ARGUMENTS,
                ErrorCategory::Validation,
                "subagent input is invalid",
            )
        })?,
    )]);
    let request_digest = request_digest(agent, &input, placement, &locator)?;
    let request = ChildRunRequest {
        agent: agent.clone(),
        input,
        placement,
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
    match invoker.start_or_attach(context, request).await {
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
                    child: StartedChild { handle, placement },
                }),
            })
        }
        Err(error) => Ok(invoke_error_result(&error)),
    }
}

fn status_child(
    children: &BTreeMap<ChildKey, StartedChild>,
    ctx: &ToolCallContext,
    call: &ValidatedToolCall,
) -> Result<CallOutcome, ToolError> {
    let arguments: RunIdArguments = parse_args(call)?;
    let Some(run_id) = arguments.run_id.as_ref() else {
        return Ok(error_result(
            SUBAGENT_INVALID_ARGUMENTS,
            "status requires run_id",
        ));
    };
    let Some((key, child)) = lookup_child(children, ctx.run.locator.tenant_scope.as_ref(), run_id)
    else {
        return Ok(error_result(
            SUBAGENT_CHILD_NOT_FOUND,
            "no started child matches run_id",
        ));
    };
    let output = result_json(&serde_json::json!({
        "run_id": key.run_id.as_ref(),
        "session_id": child.handle.locator.operation.session_id.to_string(),
        "status": "accepted",
    }))?;
    Ok(CallOutcome {
        output,
        is_error: false,
        started: None,
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
    invoker: &Arc<dyn AgentInvoker>,
    children: &BTreeMap<ChildKey, StartedChild>,
    ctx: &ToolCallContext,
    call: &ValidatedToolCall,
) -> Result<CallOutcome, ToolError> {
    let arguments: RunIdArguments = parse_args(call)?;
    let Some(run_id) = arguments.run_id.as_ref() else {
        return Ok(error_result(
            SUBAGENT_INVALID_ARGUMENTS,
            "cancel requires run_id",
        ));
    };
    let Some((key, child)) = lookup_child(children, ctx.run.locator.tenant_scope.as_ref(), run_id)
    else {
        return Ok(error_result(
            SUBAGENT_CHILD_NOT_FOUND,
            "no started child matches run_id",
        ));
    };
    let run_id = &key.run_id;
    if let Err(error) = invoker.cancel(&child.handle.locator).await {
        return Ok(invoke_error_result(&error));
    }
    let output = result_json(&serde_json::json!({
        "run_id": run_id.as_ref(),
        "cancelled": true,
        "placement": placement_name(child.placement),
    }))?;
    Ok(CallOutcome {
        output,
        is_error: false,
        started: None,
    })
}

fn child_locator(
    parent: &OperationLocator,
    placement: ChildPlacement,
) -> Result<ChildRunLocator, IdGenerationError> {
    let generator = UuidV7Generator::new(SystemClock, OsRandomSource);
    let run_id = generator.generate::<RunTag>()?;
    let lane_id = generator.generate::<LaneTag>()?;
    let session_id = match placement {
        ChildPlacement::CompatibleLaneInParentSession => parent.session_id,
        ChildPlacement::IsolatedChildSession | ChildPlacement::RemoteChildSession => {
            generator.generate::<SessionTag>()?
        }
    };
    Ok(ChildRunLocator {
        operation: OperationLocator::try_new(
            parent.tenant_scope.as_ref(),
            session_id,
            lane_id,
            run_id,
        )
        .map_err(|_| IdGenerationError::Source("child locator is invalid".into()))?,
        remote: None,
    })
}

fn request_digest(
    agent: &AgentRef,
    input: &[ContentBlock],
    placement: ChildPlacement,
    locator: &ChildRunLocator,
) -> Result<Digest, ToolError> {
    let canonical = serde_json::to_vec(&serde_json::json!({
        "agent_id": agent.id.to_string(),
        "input": input.iter().map(content_text).collect::<Vec<_>>(),
        "placement": placement_name(placement),
        "run_id": locator.operation.run_id.to_string(),
    }))
    .map_err(|_| {
        tool_error(
            SUBAGENT_INVALID_ARGUMENTS,
            ErrorCategory::Internal,
            "subagent request digest serialization failed",
        )
    })?;
    Digest::domain_separated("child-run-request", 1, &canonical).map_err(|_| {
        tool_error(
            SUBAGENT_INVALID_ARGUMENTS,
            ErrorCategory::Internal,
            "subagent request digest failed",
        )
    })
}

fn content_text(block: &ContentBlock) -> String {
    match block {
        ContentBlock::Text(text) => text.text().to_string(),
        _ => String::new(),
    }
}

fn placement_name(placement: ChildPlacement) -> &'static str {
    match placement {
        ChildPlacement::CompatibleLaneInParentSession => "compatible_lane_in_parent_session",
        ChildPlacement::IsolatedChildSession => "isolated_child_session",
        ChildPlacement::RemoteChildSession => "remote_child_session",
    }
}

fn parse_args<T: for<'de> Deserialize<'de>>(call: &ValidatedToolCall) -> Result<T, ToolError> {
    serde_json::from_slice(call.call.arguments().as_bytes()).map_err(|_| {
        tool_error(
            SUBAGENT_INVALID_ARGUMENTS,
            ErrorCategory::Validation,
            "subagent arguments are invalid",
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
    }
}

fn result_json(value: &serde_json::Value) -> Result<RawJson, ToolError> {
    let bytes = serde_json::to_vec(value).map_err(|_| {
        tool_error(
            SUBAGENT_INVALID_ARGUMENTS,
            ErrorCategory::Internal,
            "subagent result serialization failed",
        )
    })?;
    RawJson::parse(bytes).map_err(|_| {
        tool_error(
            SUBAGENT_INVALID_ARGUMENTS,
            ErrorCategory::Internal,
            "subagent result normalization failed",
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
) -> Result<ToolSpec, SubagentError> {
    let spec = ToolSpec {
        id: ToolId::parse(id).map_err(|_| SubagentError::Configuration {
            reason: "invalid_tool_id",
        })?,
        model_name: Arc::from(name),
        title: Arc::from(name),
        description: Arc::from(description),
        input_schema: RawJson::parse(schema).map_err(|_| SubagentError::Configuration {
            reason: "invalid_input_schema",
        })?,
        output_schema: None,
        execution: ToolExecutionMode::Sequential,
        side_effect: SideEffectClass::NonIdempotentWrite,
        retry_safety: RetrySafety::AtMostOnce,
        approval: ApprovalMetadata {
            requirement: ApprovalRequirement::Required,
            reason: Some(Arc::from(
                "child invocation is side-effecting unless the host already authorized ChildRunPolicy::Allow",
            )),
            attributes: Metadata::empty(),
        },
        max_result_bytes: 8_192,
        metadata: Metadata::empty(),
        deferral: ToolDeferralSupport::Never,
    };
    spec.validate().map_err(|_| SubagentError::Configuration {
        reason: "invalid_tool_spec",
    })?;
    Ok(spec)
}

fn tool_error(code: &'static str, category: ErrorCategory, message: &'static str) -> ToolError {
    ToolError::try_new(code, category, false, message, Metadata::empty()).unwrap_or_else(Into::into)
}
