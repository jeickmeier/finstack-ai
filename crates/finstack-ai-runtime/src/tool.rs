//! Target-neutral Toolset port, offline schema resolution, and stream normalization.

use core::fmt;
use core::future::poll_fn;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use finstack_ai_kernel::{
    ComponentInvocation, ContentBlock, Digest, EffectOutputContract, EffectOutputKind,
    ErrorCategory, ErrorCode, ErrorDescriptor, ExternalHandleRef, JsonBlock, Metadata, RawJson,
    ReconciliationPolicy, SyntheticToolClosure, Timestamp, ToolBatchId, ToolCallBlock, ToolCallId,
    ToolCallPlan, ToolFailurePolicy, ToolId, ToolProgress, ToolResultBlock, Usage,
    ValidatedToolCall, ValidationIssue, ValidationOutcome,
};
use jsonschema::{Draft, Resource, Retrieve, Uri};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{PortFuture, PortObject, PortStream, RunCallContext, ToolSpec, UsageDelta};

const TOOL_TEXT_MAX_BYTES: usize = 1_048_576;
const VALIDATION_ISSUE_MAX: usize = 64;

/// Stable unknown-tool closure code.
pub const UNKNOWN_TOOL: &str = "unknown_tool";
/// Stable invalid-argument closure code.
pub const TOOL_ARGUMENTS_INVALID: &str = "tool_arguments_invalid";
/// Stable invalid-output adapter code.
pub const TOOL_OUTPUT_INVALID: &str = "tool_output_invalid";
/// Stable malformed-stream adapter code.
pub const TOOL_STREAM_INVALID: &str = "tool_stream_invalid";
/// Stable result-bound adapter code.
pub const TOOL_RESULT_LIMIT_EXCEEDED: &str = "tool_result_limit_exceeded";
/// Stable stream-bound adapter code.
pub const TOOL_STREAM_LIMIT_EXCEEDED: &str = "tool_stream_limit_exceeded";
/// Stable approval-required closure code.
pub const TOOL_APPROVAL_REQUIRED: &str = "tool_approval_required";
/// Stable policy-denial closure code.
pub const TOOL_POLICY_DENIED: &str = "tool_policy_denied";
/// Stable cancellation adapter code.
pub const TOOL_CANCELLED: &str = "tool_cancelled";
/// Stable deadline adapter code.
pub const TOOL_DEADLINE_EXCEEDED: &str = "tool_deadline_exceeded";
/// Stable contained native-panic adapter code.
pub const TOOL_PANICKED: &str = "tool_panicked";
/// Stable registration or schema-resolution code.
pub const TOOL_REGISTRATION_INVALID: &str = "tool_registration_invalid";

/// Immutable Toolset descriptor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolsetDescriptor {
    /// Stable Toolset name.
    pub name: Arc<str>,
    /// Bounded non-secret descriptor metadata.
    #[serde(default)]
    pub metadata: Metadata,
}

/// Committed context for one direct tool call.
#[derive(Debug, Clone)]
pub struct ToolCallContext {
    /// Shared identity, authority, deadline, budget, and cancellation context.
    pub run: RunCallContext,
    /// Owning committed tool batch.
    pub tool_batch_id: ToolBatchId,
    /// Committed call identity.
    pub tool_call_id: ToolCallId,
}

/// Provider-neutral tool result before framework envelope injection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolResult {
    /// Canonical application JSON output.
    pub output: RawJson,
    /// Whether the tool reports an application-level error result.
    pub is_error: bool,
}

/// Normalized Toolset stream item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStreamItem {
    /// Transient progress update.
    Progress(ToolProgress),
    /// Cumulative usage snapshot.
    Usage(UsageDelta),
    /// Exactly one terminal result.
    Completed(ToolResult),
}

/// Boxed target-correct tool event stream.
pub type ToolEventStream = PortStream<Result<ToolStreamItem, ToolError>>;

/// Minimal outstanding direct-tool projection for reconciliation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingToolEffect {
    /// Frozen executable call.
    pub call: ValidatedToolCall,
}

/// Terminal external deferral returned by a tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolDeferral {
    /// External handle retaining the original effect identity.
    pub handle: ExternalHandleRef,
    /// Reconciliation policy.
    pub reconciliation: ReconciliationPolicy,
    /// Optional next poll time.
    pub next_poll_at: Option<Timestamp>,
    /// Optional expiry.
    pub expires_at: Option<Timestamp>,
}

/// Direct-tool reconciliation result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolReconcileResult {
    /// Tool reports a completed normalized output.
    Completed(ToolResult),
    /// Tool reports externally deferred work.
    Deferred(ToolDeferral),
    /// Tool proves the effect never started.
    NotStarted,
    /// Tool reports the effect is still running.
    StillRunning(ToolDeferral),
    /// Frozen input may be retried safely.
    RetrySafe,
    /// Tool cannot classify the effect.
    Unknown,
    /// Tool reports non-repeatable uncertainty.
    NonRepeatable,
}

/// Object-safe executable Toolset port.
pub trait Toolset: PortObject {
    /// Immutable Toolset descriptor.
    fn descriptor(&self) -> ToolsetDescriptor;

    /// Data-only tool specifications owned by this Toolset.
    fn tools(&self) -> Arc<[ToolSpec]>;

    /// Start one committed direct tool call.
    fn call(
        &self,
        ctx: ToolCallContext,
        call: ValidatedToolCall,
    ) -> PortFuture<Result<ToolEventStream, ToolError>>;

    /// Reconcile one previously committed outstanding effect.
    fn reconcile(
        &self,
        _ctx: crate::ReconcileContext,
        _effect: PendingToolEffect,
    ) -> PortFuture<Result<ToolReconcileResult, ToolError>> {
        Box::pin(async { Ok(ToolReconcileResult::Unknown) })
    }
}

/// Stable Toolset adapter error.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{code}: {message}")]
pub struct ToolError {
    code: ErrorCode,
    category: ErrorCategory,
    retryable: bool,
    message: Arc<str>,
    metadata: Metadata,
}

impl ToolError {
    /// Construct a bounded safe adapter error.
    ///
    /// # Errors
    ///
    /// Returns a registration error when the supplied error representation is invalid.
    pub fn try_new(
        code: impl AsRef<str>,
        category: ErrorCategory,
        retryable: bool,
        message: impl AsRef<str>,
        metadata: Metadata,
    ) -> Result<Self, Self> {
        let code =
            ErrorCode::new(code).map_err(|_| Self::registration("invalid tool error code"))?;
        if let Some(expected) = reserved_category(code.as_str())
            && (category != expected || retryable)
        {
            return Err(Self::registration(
                "reserved tool adapter code has an invalid classification",
            ));
        }
        let message = message.as_ref();
        if message.is_empty()
            || message.len() > TOOL_TEXT_MAX_BYTES
            || message.as_bytes().contains(&0)
        {
            return Err(Self::registration("invalid tool error message"));
        }
        Ok(Self {
            code,
            category,
            retryable,
            message: Arc::from(message),
            metadata,
        })
    }

    pub(crate) fn stable(code: &'static str, message: &'static str) -> Self {
        Self {
            code: ErrorCode::new(code).expect("frozen tool error code is valid"),
            category: reserved_category(code).unwrap_or(ErrorCategory::Tool),
            retryable: false,
            message: Arc::from(message),
            metadata: Metadata::empty(),
        }
    }

    fn registration(message: &'static str) -> Self {
        Self::stable(TOOL_REGISTRATION_INVALID, message)
    }

    /// Stable adapter code.
    #[must_use]
    pub fn code(&self) -> &str {
        self.code.as_str()
    }

    /// Stable framework category.
    #[must_use]
    pub const fn category(&self) -> ErrorCategory {
        self.category
    }

    /// Retryability classification.
    #[must_use]
    pub const fn retryable(&self) -> bool {
        self.retryable
    }

    /// Safe bounded message.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Bounded namespaced safe metadata.
    #[must_use]
    pub const fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    /// Convert to a source-free durable kernel descriptor.
    ///
    /// # Errors
    ///
    /// Returns a registration error only if an internal invariant is violated.
    pub fn to_descriptor(&self) -> Result<ErrorDescriptor, Self> {
        let mut descriptor = ErrorDescriptor::new(
            self.code.as_str(),
            self.message.as_ref(),
            self.category,
            self.retryable,
        )
        .map_err(|_| Self::registration("tool error descriptor is invalid"))?;
        descriptor.safe_details = self.metadata.clone();
        Ok(descriptor)
    }
}

fn reserved_category(code: &str) -> Option<ErrorCategory> {
    match code {
        TOOL_ARGUMENTS_INVALID | TOOL_OUTPUT_INVALID | TOOL_STREAM_INVALID => {
            Some(ErrorCategory::Validation)
        }
        TOOL_RESULT_LIMIT_EXCEEDED | TOOL_STREAM_LIMIT_EXCEEDED => Some(ErrorCategory::Limit),
        TOOL_CANCELLED => Some(ErrorCategory::Cancellation),
        TOOL_DEADLINE_EXCEEDED => Some(ErrorCategory::Deadline),
        TOOL_PANICKED => Some(ErrorCategory::Internal),
        TOOL_REGISTRATION_INVALID => Some(ErrorCategory::Registration),
        UNKNOWN_TOOL | TOOL_APPROVAL_REQUIRED | TOOL_POLICY_DENIED => Some(ErrorCategory::Tool),
        _ => None,
    }
}

/// Validator-independent compiled JSON Schema adapter.
pub trait ToolValidator: PortObject {
    /// Validate canonical JSON without recompiling the schema.
    fn validate(&self, instance: &RawJson) -> ValidationOutcome;
}

/// Compile-once tool-schema abstraction.
pub trait ToolValidatorCompiler: PortObject {
    /// Compile one schema against an explicit offline resource registry.
    ///
    /// # Errors
    ///
    /// Returns a stable registration error when the schema or an explicit
    /// resource cannot be parsed, resolved, or compiled.
    fn compile(
        &self,
        schema: &RawJson,
        resources: &BTreeMap<Arc<str>, RawJson>,
    ) -> Result<Arc<dyn ToolValidator>, ToolError>;
}

/// Default offline Draft-2020-12 validator compiler.
#[derive(Debug, Clone, Copy, Default)]
pub struct JsonSchemaToolValidatorCompiler;

impl ToolValidatorCompiler for JsonSchemaToolValidatorCompiler {
    fn compile(
        &self,
        schema: &RawJson,
        resources: &BTreeMap<Arc<str>, RawJson>,
    ) -> Result<Arc<dyn ToolValidator>, ToolError> {
        let schema = parse_json(schema)?;
        let mut compiled_resources = Vec::with_capacity(resources.len());
        for (uri, resource) in resources {
            compiled_resources.push((
                uri.to_string(),
                Resource::from_contents(parse_json(resource)?),
            ));
        }
        let validator = jsonschema::options()
            .with_draft(Draft::Draft202012)
            .with_resources(compiled_resources.into_iter())
            .with_retriever(DenyRetriever)
            .build(&schema)
            .map_err(|_| ToolError::registration("tool schema compilation failed"))?;
        Ok(Arc::new(JsonSchemaToolValidator { validator }))
    }
}

fn parse_json(value: &RawJson) -> Result<serde_json::Value, ToolError> {
    serde_json::from_slice(value.as_bytes())
        .map_err(|_| ToolError::registration("canonical tool schema is invalid"))
}

#[derive(Debug)]
struct DenyRetriever;

impl Retrieve for DenyRetriever {
    fn retrieve(
        &self,
        _uri: &Uri<String>,
    ) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
        Err(Box::new(DeniedRetrieval))
    }
}

#[derive(Debug)]
struct DeniedRetrieval;

impl fmt::Display for DeniedRetrieval {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ambient schema retrieval is disabled")
    }
}

impl std::error::Error for DeniedRetrieval {}

struct JsonSchemaToolValidator {
    validator: jsonschema::Validator,
}

impl ToolValidator for JsonSchemaToolValidator {
    fn validate(&self, instance: &RawJson) -> ValidationOutcome {
        let Ok(instance) = serde_json::from_slice(instance.as_bytes()) else {
            return invalid_validator_outcome("", "", None, "canonical JSON is invalid");
        };
        let mut issues = self
            .validator
            .iter_errors(&instance)
            .take(VALIDATION_ISSUE_MAX)
            .filter_map(|error| {
                let schema_path = error.schema_path().as_str().to_owned();
                let keyword = schema_path
                    .rsplit('/')
                    .find(|part| !part.is_empty())
                    .map(Arc::<str>::from);
                ValidationIssue::try_new(
                    Arc::<str>::from(error.instance_path().as_str()),
                    Arc::<str>::from(schema_path),
                    keyword,
                    Arc::<str>::from("value does not satisfy the JSON Schema constraint"),
                )
                .ok()
            })
            .collect::<Vec<_>>();
        if issues.is_empty() {
            return ValidationOutcome::Valid;
        }
        issues.sort_by(|left, right| {
            (&left.instance_path, &left.schema_path, &left.keyword).cmp(&(
                &right.instance_path,
                &right.schema_path,
                &right.keyword,
            ))
        });
        issues.dedup();
        let feedback = validation_feedback(&issues);
        ValidationOutcome::try_invalid(Arc::from(issues), Arc::<str>::from(feedback))
            .unwrap_or_else(|_| invalid_validator_outcome("", "", None, "validation failed"))
    }
}

fn invalid_validator_outcome(
    instance_path: &str,
    schema_path: &str,
    keyword: Option<&str>,
    message: &str,
) -> ValidationOutcome {
    let issue = ValidationIssue::try_new(
        Arc::<str>::from(instance_path),
        Arc::<str>::from(schema_path),
        keyword.map(Arc::<str>::from),
        Arc::<str>::from(message),
    )
    .expect("frozen validation fallback is bounded");
    ValidationOutcome::try_invalid(
        Arc::from([issue]),
        Arc::<str>::from("JSON Schema validation failed"),
    )
    .expect("frozen validation fallback is bounded")
}

fn validation_feedback(issues: &[ValidationIssue]) -> String {
    let mut feedback = String::from("JSON Schema validation failed:");
    for issue in issues {
        if feedback.len() > 16_000 {
            break;
        }
        feedback.push(' ');
        feedback.push_str(&issue.instance_path);
        if let Some(keyword) = &issue.keyword {
            feedback.push_str(" (");
            feedback.push_str(keyword);
            feedback.push(')');
        }
        feedback.push(';');
    }
    feedback
}

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
    /// Required policy keyed by every tool id in this Toolset.
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
    #[must_use]
    pub fn plan_call(
        &self,
        call: ToolCallBlock,
        deadline: Option<Timestamp>,
        middleware: Option<ToolPolicyDecision>,
    ) -> ToolCallPlan {
        let Some(tool) = self.by_name(call.tool_name()) else {
            return synthetic(
                call,
                finstack_ai_kernel::ToolExecutionMode::Sequential,
                ToolFailurePolicy::ReturnToModel,
                &ToolError::stable(UNKNOWN_TOOL, "requested tool is not registered"),
            );
        };
        if matches!(
            tool.input_validator.validate(call.arguments()),
            ValidationOutcome::Invalid { .. }
        ) {
            return synthetic(
                call,
                tool.spec.execution,
                tool.policy.failure_policy,
                &ToolError::stable(
                    TOOL_ARGUMENTS_INVALID,
                    "tool arguments do not satisfy the registered schema",
                ),
            );
        }
        let declared_floor = match tool.spec.approval.requirement {
            crate::ApprovalRequirement::Required => ToolPolicyDecision::RequireApproval,
            crate::ApprovalRequirement::Policy | crate::ApprovalRequirement::NotRequired => {
                ToolPolicyDecision::Allow
            }
        };
        let effective = tool
            .policy
            .approval
            .max(declared_floor)
            .max(middleware.unwrap_or(ToolPolicyDecision::Allow));
        match effective {
            ToolPolicyDecision::Deny => synthetic(
                call,
                tool.spec.execution,
                tool.policy.failure_policy,
                &ToolError::stable(TOOL_POLICY_DENIED, "tool execution was denied by policy"),
            ),
            ToolPolicyDecision::RequireApproval => synthetic(
                call,
                tool.spec.execution,
                tool.policy.failure_policy,
                &ToolError::stable(
                    TOOL_APPROVAL_REQUIRED,
                    "tool execution requires durable approval evidence",
                ),
            ),
            ToolPolicyDecision::Allow => ToolCallPlan::Execute(ValidatedToolCall {
                call,
                tool_id: tool.spec.id.clone(),
                component: tool.component.clone(),
                output_contract: tool.output_contract.clone(),
                retry_safety: tool.spec.retry_safety,
                deadline,
                execution: tool.spec.execution,
                failure_policy: tool.policy.failure_policy,
            }),
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
        .expect("frozen synthetic tool error is valid");
    ToolCallPlan::SyntheticClosure(SyntheticToolClosure {
        call,
        execution,
        failure_policy,
        error: descriptor,
    })
}

/// Target-neutral stream limits applied before durable settlement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolStreamLimits {
    /// Maximum emitted items including the terminal result.
    pub max_items: usize,
    /// Maximum encoded progress and usage bytes.
    pub max_stream_bytes: usize,
}

impl Default for ToolStreamLimits {
    fn default() -> Self {
        Self {
            max_items: 4_096,
            max_stream_bytes: 1_048_576,
        }
    }
}

/// Fully normalized direct tool stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssembledToolStream {
    /// Transient progress updates in arrival order.
    pub progress: Arc<[ToolProgress]>,
    /// Final cumulative usage, when supplied.
    pub usage: Option<Usage>,
    /// Exactly one completed result.
    pub result: ToolResult,
}

/// Target-neutral strict tool stream driver.
#[derive(Debug, Clone, Copy)]
pub struct ToolStreamAssembler {
    limits: ToolStreamLimits,
}

impl ToolStreamAssembler {
    /// Construct with explicit stream limits.
    #[must_use]
    pub const fn new(limits: ToolStreamLimits) -> Self {
        Self { limits }
    }

    /// Consume through EOF and normalize one tool stream.
    ///
    /// # Errors
    ///
    /// Rejects missing/duplicate completion, post-terminal items or errors,
    /// regressing usage, oversized streams/results, and invalid successful output.
    pub async fn assemble(
        self,
        mut stream: ToolEventStream,
        output_validator: Option<&dyn ToolValidator>,
        max_result_bytes: u64,
    ) -> Result<AssembledToolStream, ToolError> {
        let mut count = 0_usize;
        let mut stream_bytes = 0_usize;
        let mut progress = Vec::new();
        let mut usage: Option<Usage> = None;
        let mut result = None;
        while let Some(item) = poll_fn(|cx| stream.as_mut().poll_next(cx)).await {
            count = count.checked_add(1).ok_or_else(stream_limit)?;
            if count > self.limits.max_items {
                return Err(stream_limit());
            }
            if result.is_some() {
                return Err(ToolError::stable(
                    TOOL_STREAM_INVALID,
                    "tool stream emitted data after completion",
                ));
            }
            let item = item.map_err(|error| {
                if result.is_some() {
                    ToolError::stable(
                        TOOL_STREAM_INVALID,
                        "tool stream emitted an error after completion",
                    )
                } else {
                    error
                }
            })?;
            match item {
                ToolStreamItem::Progress(value) => {
                    add_stream_bytes(
                        &mut stream_bytes,
                        serde_json_canonicalizer::to_vec(&value)
                            .map_err(|_| stream_invalid())?
                            .len(),
                        self.limits.max_stream_bytes,
                    )?;
                    progress.push(value);
                }
                ToolStreamItem::Usage(value) => {
                    validate_usage(&value.usage, usage.as_ref())?;
                    add_stream_bytes(
                        &mut stream_bytes,
                        value
                            .usage
                            .canonical_bytes()
                            .map_err(|_| stream_invalid())?
                            .len(),
                        self.limits.max_stream_bytes,
                    )?;
                    usage = Some(value.usage);
                }
                ToolStreamItem::Completed(value) => {
                    if value.output.as_bytes().len() as u64 > max_result_bytes {
                        return Err(ToolError::stable(
                            TOOL_RESULT_LIMIT_EXCEEDED,
                            "tool result exceeds the registered byte limit",
                        ));
                    }
                    if !value.is_error
                        && output_validator.is_some_and(|validator| {
                            matches!(
                                validator.validate(&value.output),
                                ValidationOutcome::Invalid { .. }
                            )
                        })
                    {
                        return Err(ToolError::stable(
                            TOOL_OUTPUT_INVALID,
                            "successful tool output does not satisfy the registered schema",
                        ));
                    }
                    result = Some(value);
                }
            }
        }
        let result = result.ok_or_else(|| {
            ToolError::stable(
                TOOL_STREAM_INVALID,
                "tool stream ended without a completion",
            )
        })?;
        Ok(AssembledToolStream {
            progress: progress.into(),
            usage,
            result,
        })
    }
}

impl Default for ToolStreamAssembler {
    fn default() -> Self {
        Self::new(ToolStreamLimits::default())
    }
}

/// Inject a committed call id and canonical single-JSON-block framework envelope.
///
/// # Errors
///
/// Returns a validation error only if the fixed envelope violates kernel content bounds.
pub fn normalize_tool_result(
    tool_call_id: ToolCallId,
    result: ToolResult,
) -> Result<ToolResultBlock, ToolError> {
    ToolResultBlock::try_new(
        tool_call_id,
        vec![ContentBlock::Json(JsonBlock::new(result.output))],
        result.is_error,
    )
    .map_err(|_| ToolError::stable(TOOL_OUTPUT_INVALID, "tool result envelope is invalid"))
}

fn validate_usage(current: &Usage, previous: Option<&Usage>) -> Result<(), ToolError> {
    current.validate().map_err(|_| stream_invalid())?;
    if let (Some(input), Some(output), Some(total)) = (
        current.input_tokens(),
        current.output_tokens(),
        current.total_tokens(),
    ) && input.checked_add(output) != Some(total)
    {
        return Err(stream_invalid());
    }
    if let Some(previous) = previous
        && (regressed(previous.input_tokens(), current.input_tokens())
            || regressed(previous.output_tokens(), current.output_tokens())
            || regressed(previous.total_tokens(), current.total_tokens())
            || cost_regressed(previous, current)
            || previous.extension_counters().iter().any(|(key, value)| {
                current
                    .extension_counters()
                    .get(key)
                    .is_none_or(|current| current < value)
            }))
    {
        return Err(stream_invalid());
    }
    Ok(())
}

fn cost_regressed(previous: &Usage, current: &Usage) -> bool {
    match (previous.cost(), current.cost()) {
        (Some(_), None) => true,
        (Some(previous), Some(current)) => {
            previous.unit() != current.unit()
                || previous.pricing_policy_version() != current.pricing_policy_version()
                || previous.micros() > current.micros()
        }
        _ => false,
    }
}

fn regressed(previous: Option<u64>, current: Option<u64>) -> bool {
    match (previous, current) {
        (Some(_), None) => true,
        (Some(previous), Some(current)) => current < previous,
        _ => false,
    }
}

fn add_stream_bytes(total: &mut usize, add: usize, max: usize) -> Result<(), ToolError> {
    *total = total.checked_add(add).ok_or_else(stream_limit)?;
    if *total > max {
        return Err(stream_limit());
    }
    Ok(())
}

fn stream_limit() -> ToolError {
    ToolError::stable(
        TOOL_STREAM_LIMIT_EXCEEDED,
        "tool stream exceeds its configured limit",
    )
}

fn stream_invalid() -> ToolError {
    ToolError::stable(TOOL_STREAM_INVALID, "tool stream is malformed")
}
