use core::fmt;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use finstack_ai_kernel::{
    ComponentInvocation, Digest, EffectOutputContract, EffectOutputKind, ErrorCode,
    ErrorDescriptor, ErrorIdentifiers, RawJson, SyntheticToolClosure, Timestamp, ToolCallBlock,
    ToolCallPlan, ToolFailurePolicy, ToolId, ValidatedToolCall, ValidationOutcome,
};

use serde::{Deserialize, Serialize};

use crate::ToolSpec;

use super::error::{
    TOOL_APPROVAL_REQUIRED, TOOL_ARGUMENTS_INVALID, TOOL_POLICY_DENIED, ToolError, UNKNOWN_TOOL,
};
use super::port::Toolset;
use super::types::ToolsetDescriptor;
use super::validator::{ToolValidator, ToolValidatorCompiler, parse_json};

/// Fail-closed host or middleware policy decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolPolicyDecision {
    /// Permit execution.
    Allow,
    /// Require durable approval evidence before execution.
    RequireApproval,
    /// Deny execution.
    Deny,
}

/// Per-call approval evidence used by [`ResolvedToolCatalog::decide_plan`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalState {
    /// No grant or refusal has been recorded for this call yet.
    Unpaid,
    /// A granting terminal released this call.
    Granted,
    /// The approver refused this call, or the request expired or was cancelled.
    Refused,
}

/// Single-boundary catalog planning outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
#[expect(
    clippy::large_enum_variant,
    reason = "Ready carries the frozen ToolCallPlan; boxing would add a heap hop on every catalog decision"
)]
pub enum ToolCatalogPlan {
    /// Validated execute plan or a non-approval synthetic closure.
    Ready(ToolCallPlan),
    /// Durable approval must be requested before this call may execute.
    RequireApproval,
}

/// Explicit host-owned execution policy for one resolved tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolExecutionPolicy {
    /// Framework behavior on failed execution.
    pub failure_policy: ToolFailurePolicy,
    /// Host approval decision.
    pub approval: ToolPolicyDecision,
    /// Executor-local per-tool concurrency ceiling.
    pub max_concurrency: usize,
}

/// One Toolset and its trusted host-owned resolution inputs.
#[derive(Clone)]
pub struct ToolsetRegistration {
    /// Direct executable Toolset.
    pub toolset: Arc<dyn Toolset>,
    /// Required host policy keyed by every tool id in this Toolset.
    ///
    /// Host [`ToolPolicyDecision::Allow`] cannot weaken a tool that
    /// declared [`crate::ApprovalRequirement::Policy`].
    pub policies: BTreeMap<ToolId, ToolExecutionPolicy>,
    /// Optional component invocation keyed by tool id.
    pub components: BTreeMap<ToolId, ComponentInvocation>,
}

impl fmt::Debug for ToolsetRegistration {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ToolsetRegistration")
            .field("descriptor", &self.toolset.descriptor())
            .field("policies", &self.policies)
            .field("components", &self.components)
            .finish()
    }
}

/// Fully resolved executable tool retained by the catalog.
#[derive(Clone)]
pub struct ResolvedTool {
    /// Unchanged PR-015 data-only specification.
    pub spec: ToolSpec,
    /// Direct Toolset implementation.
    pub toolset: Arc<dyn Toolset>,
    /// Optional component invocation identity.
    pub component: Option<ComponentInvocation>,
    /// Precompiled input validator.
    pub input_validator: Arc<dyn ToolValidator>,
    /// Optional precompiled application-output validator.
    pub output_validator: Option<Arc<dyn ToolValidator>>,
    /// Frozen framework/application output envelope contract.
    pub output_contract: EffectOutputContract,
    /// Trusted host execution policy.
    pub policy: ToolExecutionPolicy,
}

impl fmt::Debug for ResolvedTool {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResolvedTool")
            .field("spec", &self.spec)
            .field("component", &self.component)
            .field("output_contract", &self.output_contract)
            .field("policy", &self.policy)
            .finish_non_exhaustive()
    }
}

/// Immutable catalog published only after complete validation and resolution.
#[derive(Debug, Clone)]
pub struct ResolvedToolCatalog {
    by_id: BTreeMap<ToolId, Arc<ResolvedTool>>,
    by_name: BTreeMap<Arc<str>, Arc<ResolvedTool>>,
}

impl ResolvedToolCatalog {
    /// Maximum resolved tools in one catalog.
    pub const MAX_TOOLS: usize = 1_024;
    /// Maximum explicit offline schema resources in one catalog.
    pub const MAX_RESOURCES: usize = 256;

    /// Resolve and compile all Toolset entries before publishing the catalog.
    ///
    /// # Errors
    ///
    /// Rejects invalid descriptors/specs/schemas, duplicate ids or names,
    /// missing or unknown policy entries, unknown component entries, and zero
    /// concurrency ceilings.
    pub fn try_new(
        registrations: impl IntoIterator<Item = ToolsetRegistration>,
        resources: &BTreeMap<Arc<str>, RawJson>,
        compiler: &dyn ToolValidatorCompiler,
    ) -> Result<Self, ToolError> {
        if resources.len() > Self::MAX_RESOURCES {
            return Err(ToolError::registration(
                "tool schema resource limit exceeded",
            ));
        }
        let mut by_id = BTreeMap::new();
        let mut by_name = BTreeMap::new();
        for registration in registrations {
            validate_descriptor(&registration.toolset.descriptor())?;
            let specs = registration.toolset.tools();
            let spec_ids = specs
                .iter()
                .map(|spec| spec.id.clone())
                .collect::<BTreeSet<_>>();
            if registration
                .policies
                .keys()
                .any(|id| !spec_ids.contains(id))
                || registration
                    .components
                    .keys()
                    .any(|id| !spec_ids.contains(id))
            {
                return Err(ToolError::registration(
                    "tool registration contains an unknown policy or component entry",
                ));
            }
            for spec in specs.iter() {
                if by_id.len() >= Self::MAX_TOOLS {
                    return Err(ToolError::registration("resolved tool limit exceeded"));
                }
                spec.validate().map_err(|_| {
                    ToolError::registration("tool specification metadata is invalid")
                })?;
                if by_id.contains_key(&spec.id) || by_name.contains_key(&spec.model_name) {
                    return Err(ToolError::registration(
                        "duplicate tool id or model-visible name",
                    ));
                }
                let policy = registration
                    .policies
                    .get(&spec.id)
                    .copied()
                    .ok_or_else(|| ToolError::registration("tool execution policy is missing"))?;
                if policy.max_concurrency == 0 {
                    return Err(ToolError::registration(
                        "tool concurrency limit must be positive",
                    ));
                }
                let input_validator = compiler.compile(&spec.input_schema, resources)?;
                let output_validator = spec
                    .output_schema
                    .as_ref()
                    .map(|schema| compiler.compile(schema, resources))
                    .transpose()?;
                let output_contract = tool_output_contract(spec)?;
                let resolved = Arc::new(ResolvedTool {
                    spec: spec.clone(),
                    toolset: Arc::clone(&registration.toolset),
                    component: registration.components.get(&spec.id).cloned(),
                    input_validator,
                    output_validator,
                    output_contract,
                    policy,
                });
                by_id.insert(spec.id.clone(), Arc::clone(&resolved));
                by_name.insert(Arc::clone(&spec.model_name), resolved);
            }
        }
        Ok(Self { by_id, by_name })
    }

    /// Look up a resolved tool by stable id.
    #[must_use]
    pub fn by_id(&self, id: &ToolId) -> Option<&Arc<ResolvedTool>> {
        self.by_id.get(id)
    }

    /// Look up a resolved tool by model-visible name.
    #[must_use]
    pub fn by_name(&self, name: &str) -> Option<&Arc<ResolvedTool>> {
        self.by_name.get(name)
    }

    /// Number of resolved tools.
    #[must_use]
    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    /// Whether the catalog is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    /// Iterate resolved tools in stable tool-id order.
    pub fn tools(&self) -> impl Iterator<Item = &Arc<ResolvedTool>> {
        self.by_id.values()
    }

    /// Produce the sole validated/synthetic planning outcome for one source call.
    ///
    /// Approval-required tools return [`ToolCatalogPlan::RequireApproval`] until
    /// this call's [`ApprovalState`] is [`ApprovalState::Granted`]. Denied or
    /// expired approvals close with a diagnostic synthetic and never become
    /// `Execute`. [`crate::ApprovalRequirement::Policy`] is a mandatory floor
    /// and cannot be weakened by host [`ToolPolicyDecision::Allow`].
    #[must_use]
    pub fn decide_plan(
        &self,
        call: ToolCallBlock,
        deadline: Option<Timestamp>,
        middleware: Option<ToolPolicyDecision>,
        approval: ApprovalState,
    ) -> ToolCatalogPlan {
        let Some(tool) = self.by_name(call.tool_name()) else {
            return ToolCatalogPlan::Ready(synthetic(
                call,
                finstack_ai_kernel::ToolExecutionMode::Sequential,
                ToolFailurePolicy::ReturnToModel,
                &ToolError::stable(UNKNOWN_TOOL, "requested tool is not registered"),
            ));
        };
        if matches!(
            tool.input_validator.validate(call.arguments()),
            ValidationOutcome::Invalid { .. }
        ) {
            return ToolCatalogPlan::Ready(synthetic(
                call,
                tool.spec.execution,
                tool.policy.failure_policy,
                &ToolError::stable(
                    TOOL_ARGUMENTS_INVALID,
                    "tool arguments do not satisfy the registered schema",
                ),
            ));
        }
        let declared_floor = match tool.spec.approval.requirement {
            crate::ApprovalRequirement::Required | crate::ApprovalRequirement::Policy => {
                ToolPolicyDecision::RequireApproval
            }
            crate::ApprovalRequirement::NotRequired => ToolPolicyDecision::Allow,
        };
        let effective = tool
            .policy
            .approval
            .max(declared_floor)
            .max(middleware.unwrap_or(ToolPolicyDecision::Allow));
        match effective {
            ToolPolicyDecision::Deny => ToolCatalogPlan::Ready(synthetic(
                call,
                tool.spec.execution,
                tool.policy.failure_policy,
                &ToolError::stable(TOOL_POLICY_DENIED, "tool execution was denied by policy"),
            )),
            ToolPolicyDecision::RequireApproval if approval == ApprovalState::Granted => {
                ToolCatalogPlan::Ready(ToolCallPlan::Execute(ValidatedToolCall {
                    call,
                    tool_id: tool.spec.id.clone(),
                    component: tool.component.clone(),
                    output_contract: tool.output_contract.clone(),
                    retry_safety: tool.spec.retry_safety,
                    deadline,
                    execution: tool.spec.execution,
                    failure_policy: tool.policy.failure_policy,
                }))
            }
            ToolPolicyDecision::RequireApproval if approval == ApprovalState::Refused => {
                ToolCatalogPlan::Ready(synthetic(
                    call,
                    tool.spec.execution,
                    tool.policy.failure_policy,
                    // Terminal, and it has to read that way: a model told only
                    // that approval was missing reissues the identical call and
                    // re-prompts the approver until the run exhausts its cycles.
                    &ToolError::stable(
                        TOOL_APPROVAL_REQUIRED,
                        "the approver refused this tool call; do not retry it, and continue without it",
                    ),
                ))
            }
            ToolPolicyDecision::RequireApproval => ToolCatalogPlan::RequireApproval,
            ToolPolicyDecision::Allow => {
                ToolCatalogPlan::Ready(ToolCallPlan::Execute(ValidatedToolCall {
                    call,
                    tool_id: tool.spec.id.clone(),
                    component: tool.component.clone(),
                    output_contract: tool.output_contract.clone(),
                    retry_safety: tool.spec.retry_safety,
                    deadline,
                    execution: tool.spec.execution,
                    failure_policy: tool.policy.failure_policy,
                }))
            }
        }
    }

    /// Produce the sole validated/synthetic planning outcome for one source call.
    #[must_use]
    pub fn plan_call(
        &self,
        call: ToolCallBlock,
        deadline: Option<Timestamp>,
        middleware: Option<ToolPolicyDecision>,
    ) -> ToolCallPlan {
        match self.decide_plan(call.clone(), deadline, middleware, ApprovalState::Unpaid) {
            ToolCatalogPlan::Ready(plan) => plan,
            ToolCatalogPlan::RequireApproval => synthetic(
                call,
                finstack_ai_kernel::ToolExecutionMode::Sequential,
                ToolFailurePolicy::ReturnToModel,
                &ToolError::stable(
                    TOOL_APPROVAL_REQUIRED,
                    "tool execution requires durable approval evidence",
                ),
            ),
        }
    }
}

fn validate_descriptor(descriptor: &ToolsetDescriptor) -> Result<(), ToolError> {
    if descriptor.name.is_empty()
        || descriptor.name.len() > 256
        || descriptor.name.as_bytes().contains(&0)
    {
        return Err(ToolError::registration(
            "toolset descriptor name is invalid",
        ));
    }
    Ok(())
}

fn tool_output_contract(spec: &ToolSpec) -> Result<EffectOutputContract, ToolError> {
    let application_schema = spec.output_schema.as_ref().map(parse_json).transpose()?;
    let envelope = serde_json::json!({
        "application_schema": application_schema,
        "content": "single_json_block",
        "kind": "tool_result",
        "schema_version": 1,
    });
    let bytes = serde_json_canonicalizer::to_vec(&envelope)
        .map_err(|_| ToolError::registration("tool output contract is not serializable"))?;
    Ok(EffectOutputContract {
        kind: EffectOutputKind::ToolResult,
        schema_version: 1,
        schema_digest: Digest::raw_json(&bytes),
    })
}

fn synthetic(
    call: ToolCallBlock,
    execution: finstack_ai_kernel::ToolExecutionMode,
    failure_policy: ToolFailurePolicy,
    error: &ToolError,
) -> ToolCallPlan {
    let descriptor = error
        .to_descriptor()
        .unwrap_or_else(|fallback| ErrorDescriptor {
            code: ErrorCode::new(fallback.code())
                .unwrap_or_else(|_| finstack_ai_kernel::static_error_code!("internal")),
            message: Arc::from(fallback.message()),
            category: fallback.category(),
            retryable: fallback.retryable(),
            identifiers: ErrorIdentifiers::default(),
            safe_details: fallback.metadata().clone(),
        });
    ToolCallPlan::SyntheticClosure(SyntheticToolClosure {
        call,
        execution,
        failure_policy,
        error: descriptor,
    })
}
