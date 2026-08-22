//! Subagent Toolset over the host [`AgentInvoker`](finstack_ai_runtime::child::AgentInvoker).
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
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use finstack_ai_kernel::{
    AgentId, BudgetRequest, ChildPlacement, ContentBlock, ErrorCategory, Metadata, RawJson,
    RetrySafety, TextBlock, ToolExecutionMode, ToolId, ValidatedToolCall,
};
use finstack_ai_runtime::child::{
    AGENT_INVOKE_INVALID_ACCEPTANCE, AgentInvokeError, AgentRef, ChildRunHandle,
    ChildRunStartRequest, ChildRunStarter,
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
/// The bounded in-process child table is full.
pub const SUBAGENT_LIMIT_EXCEEDED: &str = "subagent_limit_exceeded";

const DEFAULT_MAX_TRACKED_CHILDREN: usize = 1_024;
const MAX_TRACKED_CHILDREN: usize = 65_536;
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

/// Host-bound subagent toolset with a frozen agent allow-list.
pub struct SubagentToolset {
    descriptor: ToolsetDescriptor,
    tools: Arc<[ToolSpec]>,
    starter: Arc<ChildRunStarter>,
    allow_list: Arc<[AgentRef]>,
    children: Arc<Mutex<BTreeMap<ChildKey, StartedChild>>>,
    tracked_slots: Arc<AtomicUsize>,
    max_tracked_children: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ChildKey {
    tenant_scope: Arc<str>,
    owner_session_id: Arc<str>,
    run_id: Arc<str>,
}

#[derive(Clone)]
struct StartedChild {
    handle: ChildRunHandle,
    placement: ChildPlacement,
}

impl SubagentToolset {
    /// Construct the toolset over a host child starter and a non-empty allow-list.
    ///
    /// # Errors
    ///
    /// Returns [`SubagentError::Configuration`] when the allow-list is empty or
    /// a checked-in tool specification cannot be built.
    pub fn try_new(
        starter: Arc<ChildRunStarter>,
        allow_list: impl Into<Arc<[AgentRef]>>,
    ) -> Result<Self, SubagentError> {
        Self::try_with_max_tracked_children(starter, allow_list, DEFAULT_MAX_TRACKED_CHILDREN)
    }

    /// Construct the toolset with an explicit bound for locally tracked children.
    ///
    /// # Errors
    ///
    /// Returns [`SubagentError::Configuration`] when the allow-list is empty,
    /// `max_tracked_children` is outside `1..=65_536`, or a checked-in tool
    /// specification cannot be built.
    pub fn try_with_max_tracked_children(
        starter: Arc<ChildRunStarter>,
        allow_list: impl Into<Arc<[AgentRef]>>,
        max_tracked_children: usize,
    ) -> Result<Self, SubagentError> {
        let allow_list = allow_list.into();
        if allow_list.is_empty() {
            return Err(SubagentError::Configuration {
                reason: "empty_allow_list",
            });
        }
        if !(1..=MAX_TRACKED_CHILDREN).contains(&max_tracked_children) {
            return Err(SubagentError::Configuration {
                reason: "invalid_max_tracked_children",
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
            starter,
            allow_list,
            children: Arc::new(Mutex::new(BTreeMap::new())),
            tracked_slots: Arc::new(AtomicUsize::new(0)),
            max_tracked_children,
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
        let starter = Arc::clone(&self.starter);
        let allow_list = Arc::clone(&self.allow_list);
        let table = Arc::clone(&self.children);
        let tracked_slots = Arc::clone(&self.tracked_slots);
        let max_tracked_children = self.max_tracked_children;
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
                if !reserve_child_slot(&tracked_slots, max_tracked_children) {
                    return Ok(completed(
                        error_result(
                            SUBAGENT_LIMIT_EXCEEDED,
                            "subagent child tracking limit is reached",
                        )
                        .output,
                        true,
                    ));
                }
                let result = Box::pin(start_child(&starter, &allow_list, &ctx, &call)).await;
                if !matches!(result.as_ref(), Ok(outcome) if outcome.started.is_some()) {
                    release_child_slot(&tracked_slots);
                }
                result
            } else if name == STATUS_NAME {
                status_child(&starter, &snapshot, &ctx, &call).await
            } else {
                cancel_child(&starter, &snapshot, &ctx, &call).await
            }?;
            if let Some(key) = result.remove.as_ref()
                && let Ok(mut children) = table.lock()
                && children.remove(key).is_some()
            {
                release_child_slot(&tracked_slots);
            }
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
    remove: Option<ChildKey>,
}

struct StartedRecord {
    key: ChildKey,
    child: StartedChild,
}

async fn start_child(
    starter: &Arc<ChildRunStarter>,
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
    let input = Arc::from([ContentBlock::Text(
        TextBlock::try_new(arguments.input.as_ref()).map_err(|_| {
            tool_error(
                SUBAGENT_INVALID_ARGUMENTS,
                ErrorCategory::Validation,
                "subagent input is invalid",
            )
        })?,
    )]);
    let request = ChildRunStartRequest {
        agent: agent.clone(),
        input,
        placement,
        remote: None,
        requested_deadline: ctx.run.deadline,
        requested_budget: BudgetRequest::default(),
        delegation_id: None,
        metadata: Metadata::empty(),
    };
    match Box::pin(starter.start_or_attach(ctx, request)).await {
        Ok(handle) => {
            let run_id = Arc::<str>::from(handle.locator.operation.run_id.to_string());
            let key = ChildKey {
                tenant_scope: Arc::clone(&ctx.run.locator.tenant_scope),
                owner_session_id: Arc::from(ctx.run.locator.session_id.to_string()),
                run_id: Arc::clone(&run_id),
            };
            let output = result_json(&serde_json::json!({
                "run_id": run_id.as_ref(),
                "session_id": handle.locator.operation.session_id.to_string(),
                "status": "accepted",
            }))?;
            Ok(CallOutcome {
                output,
                is_error: false,
                started: Some(StartedRecord {
                    key,
                    child: StartedChild { handle, placement },
                }),
                remove: None,
            })
        }
        Err(error) => Ok(invoke_error_result(&error)),
    }
}

async fn status_child(
    starter: &Arc<ChildRunStarter>,
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
    let Some((key, child)) = lookup_child(children, &ctx.run.locator, run_id) else {
        return Ok(error_result(
            SUBAGENT_CHILD_NOT_FOUND,
            "no started child matches run_id",
        ));
    };
    let status = match starter.status(&child.handle.locator).await {
        Ok(status) => status,
        Err(error) => return Ok(invoke_error_result(&error)),
    };
    let output = result_json(&serde_json::json!({
        "run_id": key.run_id.as_ref(),
        "session_id": child.handle.locator.operation.session_id.to_string(),
        "status": status.as_str(),
    }))?;
    Ok(CallOutcome {
        output,
        is_error: false,
        started: None,
        remove: status.is_terminal().then(|| key.clone()),
    })
}

fn lookup_child<'a>(
    children: &'a BTreeMap<ChildKey, StartedChild>,
    owner: &finstack_ai_kernel::OperationLocator,
    run_id: &str,
) -> Option<(&'a ChildKey, &'a StartedChild)> {
    let key = ChildKey {
        tenant_scope: Arc::clone(&owner.tenant_scope),
        owner_session_id: Arc::from(owner.session_id.to_string()),
        run_id: Arc::from(run_id),
    };
    children.get_key_value(&key)
}

async fn cancel_child(
    starter: &Arc<ChildRunStarter>,
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
    let Some((key, child)) = lookup_child(children, &ctx.run.locator, run_id) else {
        return Ok(error_result(
            SUBAGENT_CHILD_NOT_FOUND,
            "no started child matches run_id",
        ));
    };
    let run_id = &key.run_id;
    if let Err(error) = starter.cancel(&child.handle.locator).await {
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
        remove: Some(key.clone()),
    })
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
        remove: None,
    }
}

fn reserve_child_slot(slots: &AtomicUsize, max: usize) -> bool {
    slots
        .try_update(Ordering::AcqRel, Ordering::Acquire, |current| {
            (current < max).then_some(current + 1)
        })
        .is_ok()
}

fn release_child_slot(slots: &AtomicUsize) {
    let _ = slots.try_update(Ordering::AcqRel, Ordering::Acquire, |current| {
        current.checked_sub(1)
    });
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
