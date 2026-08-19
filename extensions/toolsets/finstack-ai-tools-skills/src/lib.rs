//! Model-facing `capability_list` / `capability_activate` Toolset.
//!
//! Activate is additions-only. The toolset unions the named id with the host's
//! current set and asks the host to submit that complete set.

#![warn(missing_docs)]

use std::collections::BTreeMap;
use std::sync::Arc;

use finstack_ai_kernel::{ActiveCapability, CapabilityActivationSource, CapabilityId, RunId};
use finstack_ai_kernel::{
    ErrorCategory, Metadata, RawJson, RetrySafety, ToolExecutionMode, ToolId, ValidatedToolCall,
};
use finstack_ai_runtime::{
    ApprovalMetadata, ApprovalRequirement, PortFuture, SideEffectClass, ToolCallContext,
    ToolDeferralSupport, ToolError, ToolEventStream, ToolResult, ToolSpec, ToolStreamItem, Toolset,
    ToolsetDescriptor, verify_authority,
};
use futures_util::stream;
use serde::Deserialize;
use thiserror::Error;

#[cfg(test)]
mod tests;

const LIST_ID: &str = "finstack.tools.skills.list";
const ACTIVATE_ID: &str = "finstack.tools.skills.activate";
const LIST_NAME: &str = "capability_list";
const ACTIVATE_NAME: &str = "capability_activate";

/// Stable constructor failure.
pub const SKILLS_CONFIGURATION_INVALID: &str = "skills_configuration_invalid";
/// Tool arguments failed validation.
pub const SKILLS_INVALID_ARGUMENTS: &str = "skills_invalid_arguments";
/// Named capability is not in the compact catalog.
pub const SKILLS_UNKNOWN_CAPABILITY: &str = "skills_unknown_capability";
/// Concurrent activation bound reached; nothing is evicted.
pub const SKILLS_ACTIVATION_BOUND: &str = "skills_activation_bound";
/// Host could not queue or read the activation.
pub const SKILLS_ACTIVATION_FAILED: &str = "skills_activation_failed";

/// Construction failure for [`SkillsToolset`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SkillsError {
    /// Catalog or tool specification is invalid.
    #[error("{SKILLS_CONFIGURATION_INVALID}: {reason}")]
    Configuration {
        /// Stable non-secret reason.
        reason: &'static str,
    },
}

/// Host failure returned from [`SkillsHost`] callbacks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillsHostError {
    /// Concurrent activation bound reached.
    Bound,
    /// Host could not read or queue the activation.
    Failed {
        /// Stable non-secret reason.
        reason: Arc<str>,
    },
}

/// Callbacks the runtime uses to read the current set and submit a complete set.
pub struct SkillsHost {
    /// Compact `id: description` catalog rendered by `capability_list`.
    pub catalog: String,
    /// Current committed active set for the calling run.
    pub active: Arc<dyn Fn(RunId) -> Result<Vec<ActiveCapability>, SkillsHostError> + Send + Sync>,
    /// Queue or submit `current ∪ named` as the complete intended set.
    pub activate:
        Arc<dyn Fn(RunId, Vec<ActiveCapability>) -> Result<(), SkillsHostError> + Send + Sync>,
}

/// Model-facing capability list and additions-only activate tools.
pub struct SkillsToolset {
    descriptor: ToolsetDescriptor,
    tools: Arc<[ToolSpec]>,
    host: SkillsHost,
    catalog_ids: BTreeMap<CapabilityId, Arc<str>>,
}

impl SkillsToolset {
    /// Construct the toolset over a compact catalog and host callbacks.
    ///
    /// # Errors
    ///
    /// Returns [`SkillsError::Configuration`] when the catalog or a checked-in
    /// tool specification is invalid.
    pub fn try_new(host: SkillsHost) -> Result<Self, SkillsError> {
        let catalog_ids = parse_catalog(&host.catalog)?;
        let tools = Arc::from([
            tool_spec(
                LIST_ID,
                LIST_NAME,
                "List model-activated capabilities as id: description lines.",
                br#"{"additionalProperties":false,"properties":{},"type":"object"}"#,
            )?,
            tool_spec(
                ACTIVATE_ID,
                ACTIVATE_NAME,
                "Activate one catalog capability without dropping already-active ones.",
                br#"{"additionalProperties":false,"properties":{"id":{"type":"string"}},"required":["id"],"type":"object"}"#,
            )?,
        ]);
        Ok(Self {
            descriptor: ToolsetDescriptor {
                name: Arc::from("finstack-skills"),
                metadata: Metadata::empty(),
            },
            tools,
            host,
            catalog_ids,
        })
    }
}

impl Toolset for SkillsToolset {
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
        if let Err(error) = verify_authority(&ctx) {
            return Box::pin(async move { Err(error) });
        }
        let name = call.call.tool_name().to_string();
        let run_id = ctx.run.locator.run_id;
        match name.as_str() {
            LIST_NAME => {
                let catalog = self.host.catalog.clone();
                Box::pin(async move { Ok(completed(list_output(&catalog), false)) })
            }
            ACTIVATE_NAME => {
                let named = match parse_activate_id(&call) {
                    Ok(id) => id,
                    Err(error) => return Box::pin(async move { Ok(completed_error(&error)) }),
                };
                if !self.catalog_ids.contains_key(&named) {
                    return Box::pin(async move {
                        Ok(completed_error(&tool_error(
                            SKILLS_UNKNOWN_CAPABILITY,
                            ErrorCategory::Validation,
                            "capability is not in the compact catalog",
                        )))
                    });
                }
                let current = match (self.host.active)(run_id) {
                    Ok(active) => active,
                    Err(error) => {
                        return Box::pin(async move { Ok(completed_error(&host_error(&error))) });
                    }
                };
                let complete = union_named(current, named);
                if let Err(error) = (self.host.activate)(run_id, complete.clone()) {
                    return Box::pin(async move { Ok(completed_error(&host_error(&error))) });
                }
                Box::pin(async move { Ok(completed(activate_output(&complete), false)) })
            }
            _ => Box::pin(async move {
                Ok(completed_error(&tool_error(
                    SKILLS_INVALID_ARGUMENTS,
                    ErrorCategory::Validation,
                    "unknown skills tool",
                )))
            }),
        }
    }
}

fn parse_catalog(catalog: &str) -> Result<BTreeMap<CapabilityId, Arc<str>>, SkillsError> {
    let mut ids = BTreeMap::new();
    for line in catalog.lines().filter(|line| !line.is_empty()) {
        let Some((id, description)) = line.split_once(':') else {
            return Err(SkillsError::Configuration {
                reason: "catalog_line_missing_separator",
            });
        };
        let id = CapabilityId::parse(id.trim()).map_err(|_| SkillsError::Configuration {
            reason: "invalid_catalog_id",
        })?;
        ids.insert(id, Arc::from(description.trim()));
    }
    Ok(ids)
}

fn parse_activate_id(call: &ValidatedToolCall) -> Result<CapabilityId, ToolError> {
    #[derive(Deserialize)]
    struct Args {
        id: String,
    }
    let args: Args = serde_json::from_slice(call.call.arguments().as_bytes()).map_err(|_| {
        tool_error(
            SKILLS_INVALID_ARGUMENTS,
            ErrorCategory::Validation,
            "capability_activate requires id",
        )
    })?;
    CapabilityId::parse(args.id).map_err(|_| {
        tool_error(
            SKILLS_INVALID_ARGUMENTS,
            ErrorCategory::Validation,
            "capability id is invalid",
        )
    })
}

fn union_named(current: Vec<ActiveCapability>, named: CapabilityId) -> Vec<ActiveCapability> {
    let mut merged: BTreeMap<CapabilityId, ActiveCapability> = BTreeMap::new();
    for item in current {
        merged.insert(item.capability_id.clone(), item);
    }
    merged.entry(named.clone()).or_insert(ActiveCapability {
        capability_id: named,
        source: CapabilityActivationSource::Model,
    });
    merged.into_values().collect()
}

fn list_output(catalog: &str) -> RawJson {
    result_json(&serde_json::json!({ "catalog": catalog }))
}

fn activate_output(complete: &[ActiveCapability]) -> RawJson {
    let ids = complete
        .iter()
        .map(|item| item.capability_id.as_str().to_owned())
        .collect::<Vec<_>>();
    result_json(&serde_json::json!({ "active": ids }))
}

fn result_json(value: &serde_json::Value) -> RawJson {
    RawJson::parse(serde_json::to_vec(value).expect("skills payload")).expect("skills json")
}

fn completed(output: RawJson, is_error: bool) -> ToolEventStream {
    Box::pin(stream::once(async move {
        Ok(ToolStreamItem::Completed(ToolResult { output, is_error }))
    }))
}

fn completed_error(error: &ToolError) -> ToolEventStream {
    completed(
        result_json(&serde_json::json!({
            "code": error.code(),
            "message": error.to_string(),
        })),
        true,
    )
}

fn host_error(error: &SkillsHostError) -> ToolError {
    match error {
        SkillsHostError::Bound => tool_error(
            SKILLS_ACTIVATION_BOUND,
            ErrorCategory::Limit,
            "concurrent capability activations reached the configured bound",
        ),
        SkillsHostError::Failed { reason } => {
            let _ = reason;
            tool_error(
                SKILLS_ACTIVATION_FAILED,
                ErrorCategory::Internal,
                "capability activation host failed",
            )
        }
    }
}

fn tool_spec(
    id: &'static str,
    name: &'static str,
    description: &'static str,
    schema: &'static [u8],
) -> Result<ToolSpec, SkillsError> {
    let spec = ToolSpec {
        id: ToolId::parse(id).map_err(|_| SkillsError::Configuration {
            reason: "invalid_tool_id",
        })?,
        model_name: Arc::from(name),
        title: Arc::from(name),
        description: Arc::from(description),
        input_schema: RawJson::parse(schema).map_err(|_| SkillsError::Configuration {
            reason: "invalid_input_schema",
        })?,
        output_schema: None,
        execution: ToolExecutionMode::Sequential,
        side_effect: SideEffectClass::NonIdempotentWrite,
        retry_safety: RetrySafety::AtMostOnce,
        approval: ApprovalMetadata {
            requirement: ApprovalRequirement::NotRequired,
            reason: Some(Arc::from(
                "capability activation changes the committed tool and instruction mask",
            )),
            attributes: Metadata::empty(),
        },
        max_result_bytes: 8_192,
        metadata: Metadata::empty(),
        deferral: ToolDeferralSupport::Never,
    };
    spec.validate().map_err(|_| SkillsError::Configuration {
        reason: "invalid_tool_spec",
    })?;
    Ok(spec)
}

fn tool_error(code: &'static str, category: ErrorCategory, message: &'static str) -> ToolError {
    ToolError::try_new(code, category, false, message, Metadata::empty()).unwrap_or_else(Into::into)
}
