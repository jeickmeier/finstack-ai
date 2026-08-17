//! Provider-neutral model port, immutable context profiles, and stream assembly.

use core::fmt;
use core::future::{Future, poll_fn, ready};
use core::task::Waker;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

use finstack_ai_kernel::{
    BudgetScopeId, ContentBlock, Digest, EffectId, ErrorCategory, ErrorDescriptor,
    ExternalHandleRef, KernelState, LABEL_MAX_BYTES, Message, Metadata, ModelRequestId,
    OperationLocator, OutputSpec, PendingModelEffect, PrincipalRef, ProviderIds, RawJson,
    ReconciliationPolicy, RetrySafety, RunPhase, Timestamp, ToolExecutionMode, ToolId, Usage,
    label_is_valid,
};
use serde::de;
use serde::{Deserialize, Deserializer, Serialize};
use thiserror::Error;

use crate::error::{PortErrorData, PortErrorInvalid};
use crate::{PortFuture, PortObject, PortStream};

const STREAM_TEXT_MAX_BYTES: usize = 1_048_576;
const STREAM_REASONING_MAX_BYTES: usize = 1_048_576;

/// Stable invalid-request adapter code.
pub const MODEL_REQUEST_INVALID: &str = "model_request_invalid";
/// Stable invalid-profile adapter code.
pub const MODEL_PROFILE_INVALID: &str = "model_profile_invalid";
/// Stable profile-relaxation adapter code.
pub const MODEL_PROFILE_RELAXATION: &str = "model_profile_relaxation";
/// Stable forbidden-run-override adapter code.
pub const MODEL_PROFILE_OVERRIDE_NOT_ALLOWED: &str = "model_profile_override_not_allowed";
/// Stable estimator mismatch adapter code.
pub const MODEL_ESTIMATOR_MISMATCH: &str = "model_estimator_mismatch";
/// Stable context-limit adapter code.
pub const MODEL_CONTEXT_LIMIT_EXCEEDED: &str = "model_context_limit_exceeded";
/// Stable missing-terminal adapter code.
pub const MODEL_STREAM_MISSING_COMPLETION: &str = "model_stream_missing_completion";
/// Stable duplicate-terminal adapter code.
pub const MODEL_STREAM_DUPLICATE_COMPLETION: &str = "model_stream_duplicate_completion";
/// Stable post-terminal item adapter code.
pub const MODEL_STREAM_ITEM_AFTER_COMPLETION: &str = "model_stream_item_after_completion";
/// Stable post-terminal error adapter code.
pub const MODEL_STREAM_ERROR_AFTER_COMPLETION: &str = "model_stream_error_after_completion";
/// Stable stream-bound adapter code.
pub const MODEL_STREAM_LIMIT_EXCEEDED: &str = "model_stream_limit_exceeded";
/// Stable malformed tool-call delta adapter code.
pub const MODEL_TOOL_CALL_DELTA_INVALID: &str = "model_tool_call_delta_invalid";
/// Stable incomplete tool-call adapter code.
pub const MODEL_TOOL_CALL_INCOMPLETE: &str = "model_tool_call_incomplete";
/// Stable invalid tool arguments adapter code.
pub const MODEL_TOOL_CALL_ARGUMENTS_INVALID: &str = "model_tool_call_arguments_invalid";
/// Stable invalid usage adapter code.
pub const MODEL_USAGE_INVALID: &str = "model_usage_invalid";
/// Stable response/stream mismatch adapter code.
pub const MODEL_RESPONSE_MISMATCH: &str = "model_response_mismatch";
/// Stable non-resumable model-reconciliation adapter code.
pub const MODEL_RECONCILIATION_UNSUPPORTED: &str = "model_reconciliation_unsupported";

/// Validated provider model name.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct ModelName(Arc<str>);

impl ModelName {
    /// Construct a non-empty bounded model name.
    ///
    /// # Errors
    ///
    /// Returns a stable request error when the name is empty, oversized, or NUL-bearing.
    pub fn try_new(value: impl AsRef<str>) -> Result<Self, ModelError> {
        Ok(Self(validated_label(value.as_ref(), "model")?))
    }

    /// Borrow the model name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for ModelName {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl<'de> Deserialize<'de> for ModelName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::try_new(value).map_err(de::Error::custom)
    }
}

/// Immutable provider/model descriptor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelDescriptor {
    /// Provider identity.
    pub provider: Arc<str>,
    /// Supported provider model names.
    pub models: Arc<[ModelName]>,
    /// Bounded provider-specific descriptor metadata.
    #[serde(default)]
    pub metadata: Metadata,
}

impl ModelDescriptor {
    /// Maximum supported model names on one provider descriptor.
    pub const MAX_MODELS: usize = 256;

    /// Validate descriptor labels, cardinality, and uniqueness.
    ///
    /// # Errors
    ///
    /// Returns `model_profile_invalid` for an invalid provider or model set.
    pub fn validate(&self) -> Result<(), ModelError> {
        validated_label(&self.provider, "model_descriptor.provider").map_err(|_| {
            ModelError::validation(MODEL_PROFILE_INVALID, "model provider identity is invalid")
        })?;
        if self.models.is_empty() || self.models.len() > Self::MAX_MODELS {
            return Err(ModelError::validation(
                MODEL_PROFILE_INVALID,
                "model descriptor has an invalid model count",
            ));
        }
        let unique = self.models.iter().collect::<BTreeSet<_>>();
        if unique.len() != self.models.len() {
            return Err(ModelError::validation(
                MODEL_PROFILE_INVALID,
                "model descriptor contains duplicate model names",
            ));
        }
        Ok(())
    }
}

/// Provider-neutral accepted input classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "the frozen compatibility DTO is a set of independent input capabilities"
)]
pub struct InputCapabilities {
    /// Plain text messages are accepted.
    pub text: bool,
    /// Structured JSON blocks are accepted.
    pub json: bool,
    /// Image references are accepted.
    pub images: bool,
    /// Audio references are accepted.
    pub audio: bool,
    /// File references are accepted.
    pub files: bool,
}

/// Provider structured-output support level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StructuredOutputCapability {
    /// No structured-output support.
    Unsupported,
    /// Prompt-enforced structured output.
    Prompted,
    /// Provider-native structured output.
    Native,
}

/// Exact or conservative estimator source classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenEstimatorSource {
    /// Provider tokenizer implementation.
    ProviderTokenizer,
    /// Project-owned exact implementation.
    ProjectExact,
    /// Documented conservative upper bound.
    ConservativeUpperBound,
}

/// Versioned estimator identity.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TokenEstimatorRef {
    /// Stable estimator identifier.
    pub id: Arc<str>,
    /// Stable estimator version.
    pub version: Arc<str>,
    /// Exactness/source class.
    pub source: TokenEstimatorSource,
}

/// Provider context ceilings and safety margins for one model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelContextProfile {
    /// Resolved provider identity.
    pub provider: Arc<str>,
    /// Resolved provider model.
    pub model: ModelName,
    /// Maximum canonical request bytes.
    pub hard_input_bytes: u64,
    /// Total context window.
    pub context_window_tokens: u64,
    /// Provider maximum output tokens.
    pub max_output_tokens: u64,
    /// Reserved output safety margin.
    pub reserved_output_tokens: u64,
    /// Provider framing overhead margin.
    pub provider_overhead_tokens: u64,
    /// Bound estimator identity.
    pub estimator: TokenEstimatorRef,
}

/// Optional tightening overlay for one context profile.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelContextProfileOverride {
    /// Optional lower byte ceiling.
    pub hard_input_bytes: Option<u64>,
    /// Optional lower context ceiling.
    pub context_window_tokens: Option<u64>,
    /// Optional lower output ceiling.
    pub max_output_tokens: Option<u64>,
    /// Optional higher reserved-output margin.
    pub reserved_output_tokens: Option<u64>,
    /// Optional higher provider-overhead margin.
    pub provider_overhead_tokens: Option<u64>,
}

impl ModelContextProfileOverride {
    fn is_empty(&self) -> bool {
        self.hard_input_bytes.is_none()
            && self.context_window_tokens.is_none()
            && self.max_output_tokens.is_none()
            && self.reserved_output_tokens.is_none()
            && self.provider_overhead_tokens.is_none()
    }
}

/// Canonical locked model context profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockedModelContextProfile {
    /// Effective immutable profile.
    pub profile: ModelContextProfile,
    /// Digest of its canonical JSON bytes.
    pub digest: Digest,
}

/// Provider capabilities for one named model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "the frozen compatibility DTO exposes independent provider capabilities"
)]
pub struct ModelCapabilities {
    /// Supported input classes.
    pub input: InputCapabilities,
    /// Provider context profile.
    pub context_profile: ModelContextProfile,
    /// Native tool-call support.
    pub native_tool_calls: bool,
    /// Parallel tool-call support.
    pub parallel_tool_calls: bool,
    /// Structured-output support.
    pub structured_output: StructuredOutputCapability,
    /// Reasoning stream support.
    pub reasoning: bool,
    /// Prompt-cache support.
    pub prompt_cache: bool,
    /// Resumable stream support.
    pub resumable_stream: bool,
    /// Provider idempotency support.
    pub idempotent_requests: bool,
    /// Namespaced native capabilities.
    #[serde(default)]
    pub native_capabilities: BTreeSet<Arc<str>>,
}

/// Canonical provider-specific model settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelSettings {
    /// Bounded canonical provider-specific values.
    pub values: RawJson,
}

/// Effective per-request ceilings copied from a locked profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelRequestLimits {
    /// Maximum canonical request bytes.
    pub max_input_bytes: u64,
    /// Maximum estimated input tokens.
    pub max_input_tokens: u64,
    /// Maximum requested output tokens.
    pub max_output_tokens: u64,
}

/// Approval policy floor carried as model-visible tool metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalRequirement {
    /// Defer to resolved host policy.
    Policy,
    /// Approval is mandatory and cannot be weakened.
    Required,
    /// No tool-declared approval floor; stricter policy may still apply.
    NotRequired,
}

/// Data-only tool approval metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalMetadata {
    /// Tool-declared approval floor.
    pub requirement: ApprovalRequirement,
    /// Optional safe explanation.
    pub reason: Option<Arc<str>>,
    /// Bounded namespaced policy hints without authority.
    #[serde(default)]
    pub attributes: Metadata,
}

/// Tool side-effect classification carried to a model request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SideEffectClass {
    /// Pure or observational operation.
    ReadOnly,
    /// Repeatable mutation under an idempotency key.
    IdempotentWrite,
    /// Non-idempotent mutation.
    NonIdempotentWrite,
}

/// Complete data-only tool description sent to a model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolSpec {
    /// Stable tool identity.
    pub id: ToolId,
    /// Unique name exposed to the model.
    pub model_name: Arc<str>,
    /// Display title.
    pub title: Arc<str>,
    /// Model-facing description.
    pub description: Arc<str>,
    /// Strict input schema document.
    pub input_schema: RawJson,
    /// Optional output schema document.
    pub output_schema: Option<RawJson>,
    /// Requested execution grouping.
    pub execution: ToolExecutionMode,
    /// Side-effect class.
    pub side_effect: SideEffectClass,
    /// Retry-safety class.
    pub retry_safety: RetrySafety,
    /// Approval metadata without authority.
    pub approval: ApprovalMetadata,
    /// Maximum normalized result bytes.
    pub max_result_bytes: u64,
    /// Bounded namespaced metadata.
    #[serde(default)]
    pub metadata: Metadata,
}

impl ToolSpec {
    /// Validate bounded model-visible tool metadata.
    ///
    /// # Errors
    ///
    /// Returns `model_request_invalid` for invalid labels, text, or result bounds.
    pub fn validate(&self) -> Result<(), ModelError> {
        validated_label(&self.model_name, "tool.model_name")?;
        validated_label(&self.title, "tool.title")?;
        if self.description.is_empty()
            || self.description.len() > STREAM_TEXT_MAX_BYTES
            || self.description.as_bytes().contains(&0)
            || self.max_result_bytes == 0
        {
            return Err(ModelError::validation(
                MODEL_REQUEST_INVALID,
                "model tool metadata or result bound is invalid",
            ));
        }
        if self.approval.reason.as_ref().is_some_and(|reason| {
            reason.is_empty()
                || reason.len() > STREAM_TEXT_MAX_BYTES
                || reason.as_bytes().contains(&0)
        }) {
            return Err(ModelError::validation(
                MODEL_REQUEST_INVALID,
                "model tool approval reason is invalid",
            ));
        }
        Ok(())
    }
}

/// Strict compatibility-controlled model request draft committed by the kernel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelRequestDraft {
    /// Provider model name.
    pub model: ModelName,
    /// Canonical input messages.
    pub messages: Arc<[Message]>,
    /// Source-ordered model-visible tools.
    pub tools: Arc<[ToolSpec]>,
    /// Output contract.
    pub output: OutputSpec,
    /// Provider-specific canonical settings.
    pub settings: ModelSettings,
    /// Effective request ceilings.
    pub limits: ModelRequestLimits,
}

impl ModelRequestDraft {
    /// Maximum canonical message count before byte/token validation.
    pub const MAX_MESSAGES: usize = 4_096;
    /// Maximum model-visible tools per request.
    pub const MAX_TOOLS: usize = 1_024;

    /// Validate collection and data-only tool metadata bounds.
    ///
    /// # Errors
    ///
    /// Returns `model_request_invalid` for an oversized collection, invalid
    /// tool metadata, or duplicate model-visible tool name.
    pub fn validate(&self) -> Result<(), ModelError> {
        if self.messages.len() > Self::MAX_MESSAGES || self.tools.len() > Self::MAX_TOOLS {
            return Err(ModelError::validation(
                MODEL_REQUEST_INVALID,
                "model request collection exceeds its cardinality limit",
            ));
        }
        let mut tool_names = BTreeSet::new();
        for tool in self.tools.iter() {
            tool.validate()?;
            if !tool_names.insert(tool.model_name.as_ref()) {
                return Err(ModelError::validation(
                    MODEL_REQUEST_INVALID,
                    "model request contains a duplicate tool name",
                ));
            }
        }
        Ok(())
    }

    /// Canonical JSON bytes committed in `EffectInput::Model`.
    ///
    /// # Errors
    ///
    /// Returns a stable request error if canonical serialization fails.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, ModelError> {
        self.validate()?;
        serde_json_canonicalizer::to_vec(self).map_err(|_| {
            ModelError::validation(MODEL_REQUEST_INVALID, "model request is not serializable")
        })
    }
}

/// Full authorization projection passed to a runtime port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizationContext {
    /// Authenticated durable principal projection.
    pub principal: PrincipalRef,
    /// Authentication method.
    pub authentication_method: Arc<str>,
    /// Assurance level.
    pub assurance_level: Arc<str>,
    /// Roles granted by the exact decision.
    pub roles: Arc<[Arc<str>]>,
    /// Permitted resource scopes.
    pub permitted_scopes: Arc<[Arc<str>]>,
    /// Bounded safe claims only.
    pub safe_claims: Metadata,
    /// Authorization policy version.
    pub policy_version: Arc<str>,
    /// Authorization decision identity.
    pub decision_id: Arc<str>,
}

#[derive(Debug)]
struct CancellationState {
    cancelled: AtomicBool,
    waiters: Mutex<Vec<Waker>>,
    children: Mutex<Vec<Weak<CancellationState>>>,
}

/// Cloneable, target-portable, effect-local cancellation signal.
#[derive(Clone, Debug)]
pub struct CancellationSignal(Arc<CancellationState>);

impl Default for CancellationSignal {
    fn default() -> Self {
        Self::new()
    }
}

impl CancellationSignal {
    /// Construct an active signal.
    #[must_use]
    pub fn new() -> Self {
        Self(Arc::new(CancellationState {
            cancelled: AtomicBool::new(false),
            waiters: Mutex::new(Vec::new()),
            children: Mutex::new(Vec::new()),
        }))
    }

    /// Construct a descendant that is cancelled when this signal is cancelled.
    ///
    /// Cancelling the child never changes its parent or siblings. Registration
    /// is race-safe with concurrent parent cancellation: the new child is
    /// either registered before propagation or observes the cancelled parent
    /// and starts cancelled.
    #[must_use]
    pub fn child(&self) -> Self {
        let child = Self::new();
        let Ok(mut children) = self.0.children.lock() else {
            // A poisoned hierarchy cannot safely promise propagation.
            child.cancel();
            return child;
        };
        if self.is_cancelled() {
            drop(children);
            child.cancel();
        } else {
            children.push(Arc::downgrade(&child.0));
        }
        child
    }

    /// Mark the signal cancelled and wake registered observers.
    pub fn cancel(&self) {
        if self.0.cancelled.swap(true, Ordering::AcqRel) {
            return;
        }
        if let Ok(mut waiters) = self.0.waiters.lock() {
            for waiter in waiters.drain(..) {
                waiter.wake();
            }
        }
        let children = self
            .0
            .children
            .lock()
            .map(|mut children| children.drain(..).collect::<Vec<_>>())
            .unwrap_or_default();
        for child in children.into_iter().filter_map(|child| child.upgrade()) {
            Self(child).cancel();
        }
    }

    /// Observe cancellation without blocking.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.cancelled.load(Ordering::Acquire)
    }

    /// Wait until cancellation is observed.
    pub async fn cancelled(&self) {
        poll_fn(|cx| {
            if self.is_cancelled() {
                return core::task::Poll::Ready(());
            }
            if let Ok(mut waiters) = self.0.waiters.lock()
                && !waiters.iter().any(|waiter| waiter.will_wake(cx.waker()))
            {
                waiters.push(cx.waker().clone());
            }
            if self.is_cancelled() {
                core::task::Poll::Ready(())
            } else {
                core::task::Poll::Pending
            }
        })
        .await;
    }
}

/// Identity, security, deadline, budget, and cancellation for one port call.
#[derive(Debug, Clone)]
pub struct RunCallContext {
    /// Complete durable operation locator.
    pub locator: OperationLocator,
    /// Authorized principal projection.
    pub authorization: AuthorizationContext,
    /// Committed effect identity — **except at a middleware stage boundary**.
    ///
    /// For every port that runs under a committed effect (Model, Tool, Context,
    /// Artifact, Budget) this is that effect's journaled `EffectId` and may be
    /// used as a journal key.
    ///
    /// A middleware stage boundary has no committed effect: no `KernelInput`
    /// commits an `EffectKind::Middleware` `EffectRequested`, and stage
    /// settlement emits only `StageOutcomeRecorded`. The chain driver therefore
    /// fills this field with
    /// [`middleware_driver::derived_stage_effect_id`](crate::middleware_driver::derived_stage_effect_id),
    /// a deterministic, domain-separated **correlation id** that names nothing
    /// in the journal. The value is projected verbatim to WIT guests by
    /// `sanitize_call_context` (`plugins/finstack-ai-wit/src/mapping.rs`), so a
    /// host or plugin that looks it up as a committed effect is wrong. See the
    /// [`middleware_driver`](crate::middleware_driver) module contract.
    pub effect_id: EffectId,
    /// One-based execution attempt.
    pub attempt: u32,
    /// Semantic deadline.
    pub deadline: Option<Timestamp>,
    /// Optional shared budget scope.
    pub budget_scope_id: Option<BudgetScopeId>,
    /// Effect-local cancellation signal.
    pub cancellation: CancellationSignal,
}

/// Model-specific call context.
#[derive(Debug, Clone)]
pub struct ModelCallContext {
    /// Shared run-call context.
    pub run: RunCallContext,
    /// Committed logical model-request identity.
    pub request_id: ModelRequestId,
}

/// Model construction warmup context.
#[derive(Debug, Clone)]
pub struct ModelWarmupContext {
    /// Construction cancellation signal.
    pub cancellation: CancellationSignal,
    /// Construction deadline.
    pub deadline: Option<Timestamp>,
    /// Bounded non-secret construction metadata.
    pub metadata: Metadata,
}

/// Reconciliation context for the original effect.
///
/// `run.effect_id` is the application-level idempotency key for tools and
/// models. Implementations should key retries and external lookups on that
/// identity rather than allocating a new one.
#[derive(Debug, Clone)]
pub struct ReconcileContext {
    /// Original run-call context.
    pub run: RunCallContext,
    /// Frozen original input digest.
    pub original_input_digest: Digest,
}

/// Post-commit model request passed to the provider port.
#[derive(Debug, Clone)]
pub struct ModelRequest {
    /// Committed identity and authorization context.
    pub call: ModelCallContext,
    /// Frozen committed draft.
    pub draft: ModelRequestDraft,
    /// Optional bounded opaque continuation state.
    pub continuation_state: Option<RawJson>,
}

/// Estimator output bound to its exact implementation identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelTokenEstimate {
    /// Estimated input tokens.
    pub input_tokens: u64,
    /// Exact estimator identity used.
    pub estimator: TokenEstimatorRef,
}

/// Source-ordered provider-neutral tool call without a framework ID.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelToolCall {
    /// Model-visible tool name.
    pub name: Arc<str>,
    /// Strict canonical arguments.
    pub arguments: RawJson,
}

/// Final normalized successful model response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelResponse {
    /// Assistant content excluding tool-call/result blocks.
    pub assistant_content: Arc<[ContentBlock]>,
    /// Source-ordered calls without framework IDs.
    pub tool_calls: Arc<[ModelToolCall]>,
    /// Final normalized usage.
    pub usage: Usage,
    /// Provider correlation identities.
    pub provider_ids: ProviderIds,
    /// Non-empty provider completion identity.
    pub completion_id: Arc<str>,
    /// Optional bounded opaque continuation state.
    pub continuation_state: Option<RawJson>,
}

/// Terminal external deferral returned by a model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelDeferral {
    /// External handle.
    pub handle: ExternalHandleRef,
    /// Reconciliation policy.
    pub reconciliation: ReconciliationPolicy,
    /// Optional next poll time.
    pub next_poll_at: Option<Timestamp>,
    /// Optional expiry.
    pub expires_at: Option<Timestamp>,
}

/// Journal-first recovery action for one outstanding model effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelResumeAction {
    /// No outstanding model effect remains.
    NoOutstanding,
    /// A recorded settlement already covers the effect.
    UseRecorded,
    /// Call the provider reconcile hook before dispatch.
    Reconcile,
    /// Re-dispatch the original committed request. First-pass never returns this.
    Retry,
    /// Wait for an external completion or later poll.
    WaitExternal,
    /// Do not request or fabricate a completion.
    SuspendUncertain,
}

/// Classify recovery from committed journal state only.
///
/// First-pass never returns [`ModelResumeAction::Retry`]. Unstarted, in-flight,
/// and completed-but-uncommitted journals are identical (`AwaitingModel` plus
/// pending, no settlement) and classify as [`ModelResumeAction::Reconcile`].
#[must_use]
pub fn model_resume_action(state: &KernelState) -> ModelResumeAction {
    let Some(pending) = state.pending_model_effect.as_ref() else {
        return ModelResumeAction::NoOutstanding;
    };
    if state
        .model_settlements
        .contains_key(&pending.requested.effect_id())
    {
        return ModelResumeAction::UseRecorded;
    }
    match state.phase {
        Some(RunPhase::AwaitingExternal) => match pending
            .deferred
            .as_ref()
            .map(|deferred| deferred.reconciliation)
        {
            Some(ReconciliationPolicy::CallbackOnly | ReconciliationPolicy::ExternalWorkflow) => {
                ModelResumeAction::WaitExternal
            }
            Some(ReconciliationPolicy::Poll | ReconciliationPolicy::CallbackOrPoll) | None => {
                ModelResumeAction::Reconcile
            }
        },
        _ => ModelResumeAction::Reconcile,
    }
}

/// Whether the committed request plus provider capabilities allow a same-identity retry.
#[must_use]
pub fn model_retry_allowed(
    requested: &finstack_ai_kernel::EffectRequested,
    capabilities: &ModelCapabilities,
) -> bool {
    matches!(
        requested.retry_safety(),
        RetrySafety::SafeToRetry | RetrySafety::IdempotentWithKey
    ) && capabilities.idempotent_requests
}

/// Model reconciliation outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelReconcileResult {
    /// Provider reports completed normalized output.
    Completed(ModelResponse),
    /// Provider reports externally deferred output.
    Deferred(ModelDeferral),
    /// Provider proves the effect never started.
    NotStarted,
    /// Provider reports the effect is still running.
    StillRunning(ModelDeferral),
    /// Frozen request may be retried safely.
    RetrySafe,
    /// Provider cannot classify the effect.
    Unknown,
    /// Provider reports non-repeatable uncertainty.
    NonRepeatable,
}

/// Map one provider reconcile result onto the documented post-reconcile action.
#[must_use]
pub fn map_model_reconcile_result(
    state: &KernelState,
    result: &ModelReconcileResult,
    retry_allowed: bool,
) -> ModelResumeAction {
    let Some(pending) = state.pending_model_effect.as_ref() else {
        return ModelResumeAction::NoOutstanding;
    };
    if state
        .model_settlements
        .contains_key(&pending.requested.effect_id())
    {
        return ModelResumeAction::UseRecorded;
    }
    let awaiting_external =
        state.phase == Some(RunPhase::AwaitingExternal) || pending.deferred.is_some();
    match result {
        ModelReconcileResult::Completed(_) => ModelResumeAction::UseRecorded,
        ModelReconcileResult::Deferred(_) | ModelReconcileResult::StillRunning(_) => {
            ModelResumeAction::WaitExternal
        }
        ModelReconcileResult::NonRepeatable => ModelResumeAction::SuspendUncertain,
        ModelReconcileResult::NotStarted | ModelReconcileResult::RetrySafe => {
            if awaiting_external {
                ModelResumeAction::SuspendUncertain
            } else {
                ModelResumeAction::Retry
            }
        }
        ModelReconcileResult::Unknown => {
            if awaiting_external || !retry_allowed {
                ModelResumeAction::SuspendUncertain
            } else {
                ModelResumeAction::Retry
            }
        }
    }
}

/// Object-safe provider-neutral model port.
///
/// Implementors fulfill one committed model request after the runtime has
/// recorded intent. Native objects are `Send + Sync`. Browser-WASM hosts stay
/// local and must not be marked thread-safe.
///
/// # Required methods
///
/// - [`descriptor`](Self::descriptor): immutable provider/model identity
/// - [`capabilities`](Self::capabilities): flags for one model name
/// - [`estimate_input_tokens`](Self::estimate_input_tokens): synchronous estimate, no I/O
/// - [`request`](Self::request): start one normalized event stream
///
/// `warmup` and `reconcile` have default implementations.
pub trait Model: PortObject {
    /// Immutable provider/model descriptor.
    fn descriptor(&self) -> ModelDescriptor;

    /// Capabilities for one descriptor model name.
    ///
    /// # Arguments
    ///
    /// * `model` - Name advertised by [`Self::descriptor`].
    fn capabilities(&self, model: &ModelName) -> ModelCapabilities;

    /// Perform one-time construction warmup.
    fn warmup(&self, _ctx: ModelWarmupContext) -> PortFuture<Result<(), ModelError>> {
        Box::pin(async { Ok(()) })
    }

    /// Estimate canonical request input synchronously without I/O.
    ///
    /// # Arguments
    ///
    /// * `model` - Name whose estimator is bound at warmup.
    /// * `canonical_request` - Canonical UTF-8 request bytes, not provider JSON.
    ///
    /// # Errors
    ///
    /// Returns a provider or validation error when the named estimator cannot
    /// produce a bound estimate for the canonical request.
    fn estimate_input_tokens(
        &self,
        model: &ModelName,
        canonical_request: &[u8],
    ) -> Result<ModelTokenEstimate, ModelError>;

    /// Start one normalized model stream.
    ///
    /// # Arguments
    ///
    /// * `request` - Committed, immutable model request. Do not mutate caller state.
    fn request(&self, request: ModelRequest) -> PortFuture<Result<ModelEventStream, ModelError>>;

    /// Reconcile one previously committed outstanding effect.
    fn reconcile(
        &self,
        _ctx: ReconcileContext,
        _effect: PendingModelEffect,
    ) -> PortFuture<Result<ModelReconcileResult, ModelError>> {
        Box::pin(async { Ok(ModelReconcileResult::Unknown) })
    }
}

/// Boxed target-correct model event stream.
pub type ModelEventStream = PortStream<Result<ModelStreamItem, ModelError>>;

/// Text fragment emitted by a model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextDelta {
    /// Non-empty UTF-8 fragment.
    pub text: Arc<str>,
}

/// Confidential provider reasoning fragment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReasoningDelta {
    /// Non-empty UTF-8 fragment.
    pub text: Arc<str>,
}

/// Cumulative normalized usage snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsageDelta {
    /// Cumulative usage.
    pub usage: Usage,
}

/// One source tool-call fragment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolCallDelta {
    /// Provider-local call index.
    pub index: u32,
    /// Name supplied on first or a later non-mutating fragment.
    pub name: Option<Arc<str>>,
    /// UTF-8 arguments fragment.
    pub arguments_delta: Arc<str>,
}

/// Opaque bounded provider event retained only inside the driver.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpaqueProviderEvent {
    /// Namespaced provider event kind.
    pub namespace: Arc<str>,
    /// Canonical bounded payload.
    pub payload: RawJson,
}

/// Normalized model stream item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelStreamItem {
    /// Assistant text fragment.
    TextDelta(TextDelta),
    /// Confidential reasoning fragment.
    ReasoningDelta(ReasoningDelta),
    /// Tool-call fragment.
    ToolCallDelta(ToolCallDelta),
    /// Cumulative usage snapshot.
    Usage(UsageDelta),
    /// Explicit provider heartbeat metadata.
    Heartbeat(Metadata),
    /// Driver-local opaque provider event.
    ProviderEvent(OpaqueProviderEvent),
    /// Successful terminal response.
    Completed(ModelResponse),
    /// Suspended terminal response.
    Deferred(ModelDeferral),
}

/// Stable model adapter error.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{data}")]
pub struct ModelError {
    data: PortErrorData,
}

impl From<PortErrorInvalid> for ModelError {
    fn from(error: PortErrorInvalid) -> Self {
        match error {
            PortErrorInvalid::InvalidCode => {
                Self::validation(MODEL_REQUEST_INVALID, "invalid model error code")
            }
            PortErrorInvalid::InvalidMessage => {
                Self::validation(MODEL_REQUEST_INVALID, "invalid model error message")
            }
            PortErrorInvalid::InvalidClassification => Self::validation(
                MODEL_REQUEST_INVALID,
                "reserved model adapter code has an invalid classification",
            ),
        }
    }
}

impl ModelError {
    /// Construct a bounded adapter error.
    ///
    /// # Errors
    ///
    /// Returns [`PortErrorInvalid`] when the supplied code, message, or reserved
    /// classification is invalid.
    pub fn try_new(
        code: impl AsRef<str>,
        category: ErrorCategory,
        retryable: bool,
        message: impl AsRef<str>,
        metadata: Metadata,
    ) -> Result<Self, PortErrorInvalid> {
        let data = PortErrorData::try_from_parts(
            code,
            category,
            retryable,
            message,
            metadata,
            STREAM_TEXT_MAX_BYTES,
        )?;
        if let Some(expected) = reserved_adapter_category(data.code.as_str())
            && (data.category != expected || data.retryable)
        {
            return Err(PortErrorInvalid::InvalidClassification);
        }
        Ok(Self { data })
    }

    fn validation(code: &'static str, message: &'static str) -> Self {
        Self {
            data: PortErrorData::frozen(code, ErrorCategory::Validation, false, message),
        }
    }

    fn limit(code: &'static str, message: &'static str) -> Self {
        Self {
            data: PortErrorData::frozen(code, ErrorCategory::Limit, false, message),
        }
    }

    /// Stable adapter code.
    #[must_use]
    pub fn code(&self) -> &str {
        self.data.code.as_str()
    }

    /// Stable framework category.
    #[must_use]
    pub const fn category(&self) -> ErrorCategory {
        self.data.category
    }

    /// Retryability classification.
    #[must_use]
    pub const fn retryable(&self) -> bool {
        self.data.retryable
    }

    /// Safe bounded message.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.data.message
    }

    /// Bounded namespaced safe metadata.
    #[must_use]
    pub const fn metadata(&self) -> &Metadata {
        &self.data.metadata
    }

    /// Convert to the kernel's source-free durable descriptor.
    ///
    /// # Errors
    ///
    /// Returns a stable request error only if an internal invariant is violated.
    pub fn to_descriptor(&self) -> Result<ErrorDescriptor, ModelError> {
        let mut descriptor = ErrorDescriptor::new(
            self.data.code.as_str(),
            self.data.message.as_ref(),
            self.data.category,
            self.data.retryable,
        )
        .map_err(|_| {
            Self::validation(MODEL_REQUEST_INVALID, "model error descriptor is invalid")
        })?;
        descriptor.safe_details = self.data.metadata.clone();
        Ok(descriptor)
    }
}

fn reserved_adapter_category(code: &str) -> Option<ErrorCategory> {
    match code {
        MODEL_CONTEXT_LIMIT_EXCEEDED | MODEL_STREAM_LIMIT_EXCEEDED => Some(ErrorCategory::Limit),
        MODEL_REQUEST_INVALID
        | MODEL_PROFILE_INVALID
        | MODEL_PROFILE_RELAXATION
        | MODEL_PROFILE_OVERRIDE_NOT_ALLOWED
        | MODEL_ESTIMATOR_MISMATCH
        | MODEL_STREAM_MISSING_COMPLETION
        | MODEL_STREAM_DUPLICATE_COMPLETION
        | MODEL_STREAM_ITEM_AFTER_COMPLETION
        | MODEL_STREAM_ERROR_AFTER_COMPLETION
        | MODEL_TOOL_CALL_DELTA_INVALID
        | MODEL_TOOL_CALL_INCOMPLETE
        | MODEL_TOOL_CALL_ARGUMENTS_INVALID
        | MODEL_USAGE_INVALID
        | MODEL_RESPONSE_MISMATCH
        | MODEL_RECONCILIATION_UNSUPPORTED => Some(ErrorCategory::Validation),
        _ => None,
    }
}

/// Validate and lock one provider profile with tightening overlays.
///
/// # Errors
///
/// Rejects invalid profiles, relaxations, forbidden run overrides, and arithmetic overflow.
pub fn resolve_model_context_profile(
    provider: ModelContextProfile,
    agent: Option<&ModelContextProfileOverride>,
    run: Option<&ModelContextProfileOverride>,
    run_override_allowed: bool,
) -> Result<LockedModelContextProfile, ModelError> {
    validate_profile(&provider)?;
    let mut profile = provider;
    if let Some(overlay) = agent {
        apply_profile_override(&mut profile, overlay)?;
    }
    if let Some(overlay) = run {
        if !run_override_allowed && !overlay.is_empty() {
            return Err(ModelError::validation(
                MODEL_PROFILE_OVERRIDE_NOT_ALLOWED,
                "run model-profile override is not allowlisted",
            ));
        }
        apply_profile_override(&mut profile, overlay)?;
    }
    validate_profile(&profile)?;
    let bytes = serde_json_canonicalizer::to_vec(&profile).map_err(|_| {
        ModelError::validation(MODEL_PROFILE_INVALID, "model profile is not serializable")
    })?;
    Ok(LockedModelContextProfile {
        profile,
        digest: Digest::raw_json(&bytes),
    })
}

fn apply_profile_override(
    profile: &mut ModelContextProfile,
    overlay: &ModelContextProfileOverride,
) -> Result<(), ModelError> {
    tighten_ceiling(&mut profile.hard_input_bytes, overlay.hard_input_bytes)?;
    tighten_ceiling(
        &mut profile.context_window_tokens,
        overlay.context_window_tokens,
    )?;
    tighten_ceiling(&mut profile.max_output_tokens, overlay.max_output_tokens)?;
    tighten_margin(
        &mut profile.reserved_output_tokens,
        overlay.reserved_output_tokens,
    )?;
    tighten_margin(
        &mut profile.provider_overhead_tokens,
        overlay.provider_overhead_tokens,
    )?;
    validate_profile(profile)
}

fn tighten_ceiling(current: &mut u64, candidate: Option<u64>) -> Result<(), ModelError> {
    if let Some(candidate) = candidate {
        if candidate > *current {
            return Err(ModelError::validation(
                MODEL_PROFILE_RELAXATION,
                "model-profile ceiling override would relax the current profile",
            ));
        }
        *current = (*current).min(candidate);
    }
    Ok(())
}

fn tighten_margin(current: &mut u64, candidate: Option<u64>) -> Result<(), ModelError> {
    if let Some(candidate) = candidate {
        if candidate < *current {
            return Err(ModelError::validation(
                MODEL_PROFILE_RELAXATION,
                "model-profile margin override would relax the current profile",
            ));
        }
        *current = (*current).max(candidate);
    }
    Ok(())
}

fn validate_profile(profile: &ModelContextProfile) -> Result<(), ModelError> {
    if profile.provider.is_empty()
        || profile.provider.len() > LABEL_MAX_BYTES
        || profile.provider.as_bytes().contains(&0)
        || profile.hard_input_bytes == 0
        || profile.context_window_tokens == 0
        || profile.max_output_tokens == 0
        || profile.estimator.id.is_empty()
        || profile.estimator.version.is_empty()
        || profile.estimator.id.len() > LABEL_MAX_BYTES
        || profile.estimator.version.len() > LABEL_MAX_BYTES
        || profile.estimator.id.as_bytes().contains(&0)
        || profile.estimator.version.as_bytes().contains(&0)
    {
        return Err(ModelError::validation(
            MODEL_PROFILE_INVALID,
            "model profile requires non-zero ceilings and estimator identity",
        ));
    }
    let margins = profile
        .reserved_output_tokens
        .checked_add(profile.provider_overhead_tokens)
        .ok_or_else(|| {
            ModelError::validation(MODEL_PROFILE_INVALID, "model profile margin overflow")
        })?;
    if margins > profile.context_window_tokens {
        return Err(ModelError::validation(
            MODEL_PROFILE_INVALID,
            "model profile margins exceed the context window",
        ));
    }
    Ok(())
}

/// Successful pre-commit request validation result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelRequestValidation {
    /// Exact canonical request bytes.
    pub canonical_request: Arc<[u8]>,
    /// Bound model-supplied token estimate.
    pub estimated_input_tokens: u64,
    /// Available input-token budget after safety margins.
    pub available_input_tokens: u64,
}

/// Validate canonical bytes, estimator binding, and effective token/output limits.
///
/// # Errors
///
/// Returns stable request/profile/estimator/context-limit errors.
pub fn validate_model_request(
    model: &dyn Model,
    draft: &ModelRequestDraft,
    locked: &LockedModelContextProfile,
) -> Result<ModelRequestValidation, ModelError> {
    if draft.model != locked.profile.model {
        return Err(ModelError::validation(
            MODEL_REQUEST_INVALID,
            "model request does not match the locked profile",
        ));
    }
    let canonical = draft.canonical_bytes()?;
    let canonical_len = u64::try_from(canonical.len()).map_err(|_| {
        ModelError::limit(MODEL_CONTEXT_LIMIT_EXCEEDED, "request byte length overflow")
    })?;
    let byte_limit = draft
        .limits
        .max_input_bytes
        .min(locked.profile.hard_input_bytes);
    if byte_limit == 0 || canonical_len > byte_limit {
        return Err(ModelError::limit(
            MODEL_CONTEXT_LIMIT_EXCEEDED,
            "model request exceeds the canonical byte ceiling",
        ));
    }
    if draft.limits.max_input_bytes > locked.profile.hard_input_bytes
        || draft.limits.max_output_tokens > locked.profile.max_output_tokens
        || draft.limits.max_input_tokens == 0
        || draft.limits.max_output_tokens == 0
    {
        return Err(ModelError::limit(
            MODEL_CONTEXT_LIMIT_EXCEEDED,
            "model request limits exceed the locked profile",
        ));
    }
    let estimate = model.estimate_input_tokens(&draft.model, &canonical)?;
    if estimate.estimator != locked.profile.estimator {
        return Err(ModelError::validation(
            MODEL_ESTIMATOR_MISMATCH,
            "model token estimator does not match the locked profile",
        ));
    }
    let margins = locked
        .profile
        .reserved_output_tokens
        .checked_add(locked.profile.provider_overhead_tokens)
        .ok_or_else(|| {
            ModelError::validation(MODEL_PROFILE_INVALID, "model profile margin overflow")
        })?;
    let available = locked
        .profile
        .context_window_tokens
        .checked_sub(margins)
        .ok_or_else(|| {
            ModelError::validation(MODEL_PROFILE_INVALID, "model profile margins are invalid")
        })?;
    let input_limit = draft.limits.max_input_tokens.min(available);
    if estimate.input_tokens > input_limit
        || draft.limits.max_output_tokens > locked.profile.reserved_output_tokens
    {
        return Err(ModelError::limit(
            MODEL_CONTEXT_LIMIT_EXCEEDED,
            "model request exceeds the effective token budget",
        ));
    }
    Ok(ModelRequestValidation {
        canonical_request: canonical.into(),
        estimated_input_tokens: estimate.input_tokens,
        available_input_tokens: available,
    })
}

/// Aggregate stream limits applied before durable settlement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelStreamLimits {
    /// Maximum stream items including the terminal item.
    pub max_items: usize,
    /// Maximum aggregate observed payload bytes.
    pub max_bytes: usize,
    /// Maximum distinct source tool calls.
    pub max_tool_calls: usize,
}

impl Default for ModelStreamLimits {
    fn default() -> Self {
        Self {
            max_items: 4_096,
            max_bytes: 2 * 1_048_576,
            max_tool_calls: 1_024,
        }
    }
}

/// Existing transient-progress vocabulary emitted by the model driver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelProgress {
    /// Assistant text fragment.
    Text(Arc<str>),
    /// Confidential reasoning fragment.
    Reasoning(Arc<str>),
    /// Explicit heartbeat metadata.
    Heartbeat(Metadata),
}

/// Validated stream terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelTerminal {
    /// Successful response.
    Completed(ModelResponse),
    /// External suspension.
    Deferred(ModelDeferral),
}

/// Completely validated stream result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssembledModelStream {
    /// Existing transient progress in source order.
    pub progress: Arc<[ModelProgress]>,
    /// Exactly one validated terminal.
    pub terminal: ModelTerminal,
}

#[derive(Default)]
struct PartialToolCall {
    name: Option<Arc<str>>,
    arguments: String,
}

/// Pure bounded stream validator and assembler.
#[derive(Debug, Clone, Copy)]
pub struct ModelStreamAssembler {
    limits: ModelStreamLimits,
}

impl ModelStreamAssembler {
    /// Construct with explicit aggregate limits.
    ///
    /// # Errors
    ///
    /// Rejects zero bounds.
    pub fn new(limits: ModelStreamLimits) -> Result<Self, ModelError> {
        if limits.max_items == 0 || limits.max_bytes == 0 || limits.max_tool_calls == 0 {
            return Err(ModelError::validation(
                MODEL_STREAM_LIMIT_EXCEEDED,
                "model stream limits must be non-zero",
            ));
        }
        Ok(Self { limits })
    }

    /// Consume through EOF and validate exactly one terminal item.
    ///
    /// # Errors
    ///
    /// Returns stable ordering, bound, usage, tool-call, and response mismatch errors.
    pub async fn assemble(
        &self,
        stream: ModelEventStream,
    ) -> Result<AssembledModelStream, ModelError> {
        let mut progress = Vec::new();
        let terminal = self
            .assemble_incremental(stream, |item| {
                progress.push(item);
                ready(Ok(()))
            })
            .await?;
        Ok(AssembledModelStream {
            progress: progress.into(),
            terminal,
        })
    }

    #[expect(
        clippy::too_many_lines,
        reason = "the stream state machine keeps all ordering and terminal transitions contiguous"
    )]
    pub(crate) async fn assemble_incremental<F, Fut>(
        &self,
        mut stream: ModelEventStream,
        mut emit_progress: F,
    ) -> Result<ModelTerminal, ModelError>
    where
        F: FnMut(ModelProgress) -> Fut,
        Fut: Future<Output = Result<(), ModelError>>,
    {
        let mut item_count = 0_usize;
        let mut byte_count = 0_usize;
        let mut text = String::new();
        let mut reasoning_bytes = 0_usize;
        let mut tools = BTreeMap::<u32, PartialToolCall>::new();
        let mut order = Vec::<u32>::new();
        let mut usage: Option<Usage> = None;
        let mut terminal: Option<ModelTerminal> = None;

        while let Some(item) = poll_fn(|cx| stream.as_mut().poll_next(cx)).await {
            if terminal.is_some() {
                return match item {
                    Err(_) => Err(ModelError::validation(
                        MODEL_STREAM_ERROR_AFTER_COMPLETION,
                        "model stream returned an error after its terminal item",
                    )),
                    Ok(ModelStreamItem::Completed(_) | ModelStreamItem::Deferred(_)) => {
                        Err(ModelError::validation(
                            MODEL_STREAM_DUPLICATE_COMPLETION,
                            "model stream returned more than one terminal item",
                        ))
                    }
                    Ok(_) => Err(ModelError::validation(
                        MODEL_STREAM_ITEM_AFTER_COMPLETION,
                        "model stream returned an item after its terminal item",
                    )),
                };
            }
            let item = item?;
            item_count = item_count.checked_add(1).ok_or_else(stream_limit_error)?;
            if item_count > self.limits.max_items {
                return Err(stream_limit_error());
            }
            match item {
                ModelStreamItem::TextDelta(delta) => {
                    validate_delta(&delta.text)?;
                    add_bytes(&mut byte_count, delta.text.len(), self.limits.max_bytes)?;
                    text.push_str(&delta.text);
                    if text.len() > STREAM_TEXT_MAX_BYTES {
                        return Err(stream_limit_error());
                    }
                    emit_progress(ModelProgress::Text(delta.text)).await?;
                }
                ModelStreamItem::ReasoningDelta(delta) => {
                    validate_delta(&delta.text)?;
                    add_bytes(&mut byte_count, delta.text.len(), self.limits.max_bytes)?;
                    reasoning_bytes = reasoning_bytes
                        .checked_add(delta.text.len())
                        .ok_or_else(stream_limit_error)?;
                    if reasoning_bytes > STREAM_REASONING_MAX_BYTES {
                        return Err(stream_limit_error());
                    }
                    emit_progress(ModelProgress::Reasoning(delta.text)).await?;
                }
                ModelStreamItem::ToolCallDelta(delta) => {
                    let is_new = !tools.contains_key(&delta.index);
                    if is_new {
                        if tools.len() >= self.limits.max_tool_calls {
                            return Err(stream_limit_error());
                        }
                        order.push(delta.index);
                    }
                    let partial = tools.entry(delta.index).or_default();
                    if let Some(name) = delta.name {
                        validated_label(&name, "tool_call.name")?;
                        add_bytes(&mut byte_count, name.len(), self.limits.max_bytes)?;
                        if partial
                            .name
                            .as_ref()
                            .is_some_and(|current| current != &name)
                        {
                            return Err(ModelError::validation(
                                MODEL_TOOL_CALL_DELTA_INVALID,
                                "model tool-call name mutated across fragments",
                            ));
                        }
                        partial.name = Some(name);
                    }
                    add_bytes(
                        &mut byte_count,
                        delta.arguments_delta.len(),
                        self.limits.max_bytes,
                    )?;
                    partial.arguments.push_str(&delta.arguments_delta);
                }
                ModelStreamItem::Usage(delta) => {
                    validate_usage(&delta.usage, usage.as_ref())?;
                    usage = Some(delta.usage);
                }
                ModelStreamItem::Heartbeat(metadata) => {
                    add_bytes(
                        &mut byte_count,
                        metadata.as_bytes().len(),
                        self.limits.max_bytes,
                    )?;
                    emit_progress(ModelProgress::Heartbeat(metadata)).await?;
                }
                ModelStreamItem::ProviderEvent(event) => {
                    validated_label(&event.namespace, "provider_event.namespace")?;
                    add_bytes(
                        &mut byte_count,
                        event.namespace.len(),
                        self.limits.max_bytes,
                    )?;
                    add_bytes(
                        &mut byte_count,
                        event.payload.as_bytes().len(),
                        self.limits.max_bytes,
                    )?;
                }
                ModelStreamItem::Completed(response) => {
                    let encoded = serde_json_canonicalizer::to_vec(&response).map_err(|_| {
                        ModelError::validation(
                            MODEL_RESPONSE_MISMATCH,
                            "model response is not serializable",
                        )
                    })?;
                    add_bytes(&mut byte_count, encoded.len(), self.limits.max_bytes)?;
                    terminal = Some(ModelTerminal::Completed(response));
                }
                ModelStreamItem::Deferred(deferral) => {
                    let encoded = serde_json_canonicalizer::to_vec(&deferral).map_err(|_| {
                        ModelError::validation(
                            MODEL_RESPONSE_MISMATCH,
                            "model deferral is not serializable",
                        )
                    })?;
                    add_bytes(&mut byte_count, encoded.len(), self.limits.max_bytes)?;
                    terminal = Some(ModelTerminal::Deferred(deferral));
                }
            }
        }

        let terminal = terminal.ok_or_else(|| {
            ModelError::validation(
                MODEL_STREAM_MISSING_COMPLETION,
                "model stream ended without a terminal item",
            )
        })?;
        match &terminal {
            ModelTerminal::Completed(response) => {
                validate_completed_response(response, &text, &tools, &order, usage.as_ref())?;
            }
            ModelTerminal::Deferred(deferral) => {
                if !tools.is_empty() {
                    return Err(ModelError::validation(
                        MODEL_TOOL_CALL_INCOMPLETE,
                        "deferred model stream contains an unsettled tool call",
                    ));
                }
                if deferral
                    .next_poll_at
                    .zip(deferral.expires_at)
                    .is_some_and(|(next, expires)| next > expires)
                {
                    return Err(ModelError::validation(
                        MODEL_RESPONSE_MISMATCH,
                        "model deferral poll time exceeds its expiry",
                    ));
                }
            }
        }
        Ok(terminal)
    }
}

fn validate_completed_response(
    response: &ModelResponse,
    streamed_text: &str,
    tools: &BTreeMap<u32, PartialToolCall>,
    order: &[u32],
    usage: Option<&Usage>,
) -> Result<(), ModelError> {
    validated_label(&response.completion_id, "completion_id")?;
    let mut final_text = String::new();
    for block in response.assistant_content.iter() {
        match block {
            ContentBlock::Text(value) => final_text.push_str(value.text()),
            ContentBlock::ToolCall(_) | ContentBlock::ToolResult(_) => {
                return Err(ModelError::validation(
                    MODEL_RESPONSE_MISMATCH,
                    "model response content contains a framework-owned tool block",
                ));
            }
            _ => {}
        }
    }
    if final_text != streamed_text {
        return Err(ModelError::validation(
            MODEL_RESPONSE_MISMATCH,
            "streamed text does not match the final response",
        ));
    }
    let mut assembled_calls = Vec::with_capacity(order.len());
    for index in order {
        let partial = &tools[index];
        let name = partial.name.clone().ok_or_else(|| {
            ModelError::validation(
                MODEL_TOOL_CALL_INCOMPLETE,
                "model tool call completed without a name",
            )
        })?;
        if partial.arguments.is_empty() {
            return Err(ModelError::validation(
                MODEL_TOOL_CALL_INCOMPLETE,
                "model tool call completed without arguments",
            ));
        }
        let arguments = RawJson::parse(partial.arguments.as_bytes()).map_err(|_| {
            ModelError::validation(
                MODEL_TOOL_CALL_ARGUMENTS_INVALID,
                "model tool-call arguments are not strict JSON",
            )
        })?;
        assembled_calls.push(ModelToolCall { name, arguments });
    }
    if assembled_calls.as_slice() != response.tool_calls.as_ref() {
        return Err(ModelError::validation(
            MODEL_RESPONSE_MISMATCH,
            "streamed tool calls do not match the final response",
        ));
    }
    validate_usage(&response.usage, None)?;
    if usage.is_some_and(|streamed| streamed != &response.usage) {
        return Err(ModelError::validation(
            MODEL_RESPONSE_MISMATCH,
            "streamed usage does not match the final response",
        ));
    }
    Ok(())
}

fn validate_usage(current: &Usage, previous: Option<&Usage>) -> Result<(), ModelError> {
    current
        .validate()
        .map_err(|_| ModelError::validation(MODEL_USAGE_INVALID, "model usage is invalid"))?;
    if let (Some(input), Some(output), Some(total)) = (
        current.input_tokens(),
        current.output_tokens(),
        current.total_tokens(),
    ) && input.checked_add(output) != Some(total)
    {
        return Err(ModelError::validation(
            MODEL_USAGE_INVALID,
            "model usage total is inconsistent",
        ));
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
        return Err(ModelError::validation(
            MODEL_USAGE_INVALID,
            "model usage regressed across cumulative snapshots",
        ));
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

fn validate_delta(value: &str) -> Result<(), ModelError> {
    if value.is_empty() || value.as_bytes().contains(&0) {
        return Err(ModelError::validation(
            MODEL_STREAM_LIMIT_EXCEEDED,
            "model stream delta must be non-empty UTF-8 without NUL",
        ));
    }
    Ok(())
}

fn add_bytes(total: &mut usize, add: usize, max: usize) -> Result<(), ModelError> {
    *total = total.checked_add(add).ok_or_else(stream_limit_error)?;
    if *total > max {
        return Err(stream_limit_error());
    }
    Ok(())
}

fn stream_limit_error() -> ModelError {
    ModelError::limit(
        MODEL_STREAM_LIMIT_EXCEEDED,
        "model stream exceeded a configured aggregate limit",
    )
}

fn validated_label(value: &str, _field: &'static str) -> Result<Arc<str>, ModelError> {
    if !label_is_valid(value) {
        return Err(ModelError::validation(
            MODEL_REQUEST_INVALID,
            "model label is empty, oversized, or contains NUL",
        ));
    }
    Ok(Arc::from(value))
}

impl fmt::Display for ModelName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct EstimatorModel {
        profile: ModelContextProfile,
        estimate: u64,
        estimator: TokenEstimatorRef,
    }

    impl Model for EstimatorModel {
        fn descriptor(&self) -> ModelDescriptor {
            ModelDescriptor {
                provider: Arc::from("test"),
                models: Arc::from([self.profile.model.clone()]),
                metadata: Metadata::empty(),
            }
        }

        fn capabilities(&self, _model: &ModelName) -> ModelCapabilities {
            ModelCapabilities {
                input: InputCapabilities {
                    text: true,
                    json: true,
                    images: false,
                    audio: false,
                    files: false,
                },
                context_profile: self.profile.clone(),
                native_tool_calls: false,
                parallel_tool_calls: false,
                structured_output: StructuredOutputCapability::Unsupported,
                reasoning: false,
                prompt_cache: false,
                resumable_stream: false,
                idempotent_requests: true,
                native_capabilities: BTreeSet::new(),
            }
        }

        fn estimate_input_tokens(
            &self,
            _model: &ModelName,
            _canonical_request: &[u8],
        ) -> Result<ModelTokenEstimate, ModelError> {
            Ok(ModelTokenEstimate {
                input_tokens: self.estimate,
                estimator: self.estimator.clone(),
            })
        }

        fn request(
            &self,
            _request: ModelRequest,
        ) -> PortFuture<Result<ModelEventStream, ModelError>> {
            Box::pin(async {
                Err(ModelError::validation(
                    "model_request_invalid",
                    "not used by profile tests",
                ))
            })
        }
    }

    fn profile() -> ModelContextProfile {
        ModelContextProfile {
            provider: Arc::from("scripted"),
            model: ModelName::try_new("scripted-1").expect("model"),
            hard_input_bytes: 1_000,
            context_window_tokens: 100,
            max_output_tokens: 20,
            reserved_output_tokens: 20,
            provider_overhead_tokens: 5,
            estimator: TokenEstimatorRef {
                id: Arc::from("bytes-upper-bound"),
                version: Arc::from("1"),
                source: TokenEstimatorSource::ConservativeUpperBound,
            },
        }
    }

    fn draft() -> ModelRequestDraft {
        ModelRequestDraft {
            model: profile().model,
            messages: Arc::from([]),
            tools: Arc::from([]),
            output: OutputSpec::PlainText,
            settings: ModelSettings {
                values: RawJson::parse(b"{}").expect("settings"),
            },
            limits: ModelRequestLimits {
                max_input_bytes: 1_000,
                max_input_tokens: 75,
                max_output_tokens: 20,
            },
        }
    }

    #[test]
    fn cancellation_hierarchy_propagates_downward_only_and_late_children_start_cancelled() {
        let run = CancellationSignal::new();
        let model = run.child();
        let tool_batch = run.child();
        let first_tool = tool_batch.child();
        let second_tool = tool_batch.child();

        first_tool.cancel();
        assert!(first_tool.is_cancelled());
        assert!(!second_tool.is_cancelled());
        assert!(!tool_batch.is_cancelled());
        assert!(!run.is_cancelled());

        run.cancel();
        assert!(model.is_cancelled());
        assert!(tool_batch.is_cancelled());
        assert!(second_tool.is_cancelled());
        assert!(run.child().is_cancelled());
    }

    #[test]
    fn profile_overrides_only_tighten_and_lock_stably() {
        let overlay = ModelContextProfileOverride {
            hard_input_bytes: Some(900),
            max_output_tokens: Some(10),
            reserved_output_tokens: Some(25),
            ..ModelContextProfileOverride::default()
        };
        let first =
            resolve_model_context_profile(profile(), Some(&overlay), None, false).expect("profile");
        let second =
            resolve_model_context_profile(profile(), Some(&overlay), None, false).expect("profile");
        assert_eq!(first, second);
        assert_eq!(first.profile.hard_input_bytes, 900);
        assert_eq!(first.profile.max_output_tokens, 10);
        assert_eq!(first.profile.reserved_output_tokens, 25);

        let mut other_provider = profile();
        other_provider.provider = Arc::from("other");
        let other = resolve_model_context_profile(other_provider, Some(&overlay), None, false)
            .expect("other provider");
        assert_ne!(first.digest, other.digest);
    }

    #[test]
    fn profile_rejects_relaxation_and_nonallowlisted_run_override() {
        let relaxation = ModelContextProfileOverride {
            max_output_tokens: Some(21),
            ..ModelContextProfileOverride::default()
        };
        let error = resolve_model_context_profile(profile(), Some(&relaxation), None, false)
            .expect_err("relaxation");
        assert_eq!(error.code(), MODEL_PROFILE_RELAXATION);

        let tightening = ModelContextProfileOverride {
            max_output_tokens: Some(19),
            ..ModelContextProfileOverride::default()
        };
        let error = resolve_model_context_profile(profile(), None, Some(&tightening), false)
            .expect_err("allowlist");
        assert_eq!(error.code(), MODEL_PROFILE_OVERRIDE_NOT_ALLOWED);

        let allowed = resolve_model_context_profile(profile(), None, Some(&tightening), true)
            .expect("allowlisted tightening");
        assert_eq!(allowed.profile.max_output_tokens, 19);
    }

    #[test]
    fn profile_rejects_invalid_boundaries_and_overflow() {
        let mut invalid = profile();
        invalid.hard_input_bytes = 0;
        assert_eq!(
            resolve_model_context_profile(invalid, None, None, false)
                .expect_err("zero")
                .code(),
            MODEL_PROFILE_INVALID
        );
        let mut overflow = profile();
        overflow.reserved_output_tokens = u64::MAX;
        overflow.provider_overhead_tokens = 1;
        assert_eq!(
            resolve_model_context_profile(overflow, None, None, false)
                .expect_err("overflow")
                .code(),
            MODEL_PROFILE_INVALID
        );

        let mut exact_margin = profile();
        exact_margin.reserved_output_tokens = 95;
        assert!(resolve_model_context_profile(exact_margin, None, None, false).is_ok());
        let mut margin_over = profile();
        margin_over.reserved_output_tokens = 96;
        assert_eq!(
            resolve_model_context_profile(margin_over, None, None, false)
                .expect_err("margin + 1")
                .code(),
            MODEL_PROFILE_INVALID
        );
    }

    #[test]
    fn request_validator_accepts_exact_limits_and_rejects_limit_plus_one() {
        let mut request = draft();
        for _ in 0..4 {
            let length =
                u64::try_from(request.canonical_bytes().expect("bytes").len()).expect("length");
            if request.limits.max_input_bytes == length {
                break;
            }
            request.limits.max_input_bytes = length;
        }
        let exact_bytes = request.limits.max_input_bytes;
        assert_eq!(
            u64::try_from(request.canonical_bytes().expect("bytes").len()).expect("length"),
            exact_bytes
        );
        let mut exact_profile = profile();
        exact_profile.hard_input_bytes = exact_bytes;
        let locked = resolve_model_context_profile(exact_profile.clone(), None, None, false)
            .expect("profile");
        let exact_model = EstimatorModel {
            profile: exact_profile.clone(),
            estimate: 75,
            estimator: exact_profile.estimator.clone(),
        };
        let validated =
            validate_model_request(&exact_model, &request, &locked).expect("exact limits");
        assert_eq!(validated.estimated_input_tokens, 75);
        assert_eq!(validated.available_input_tokens, 75);

        let token_over = EstimatorModel {
            profile: exact_profile.clone(),
            estimate: 76,
            estimator: exact_profile.estimator.clone(),
        };
        assert_eq!(
            validate_model_request(&token_over, &request, &locked)
                .expect_err("token + 1")
                .code(),
            MODEL_CONTEXT_LIMIT_EXCEEDED
        );

        let mut byte_over = request.clone();
        byte_over.settings.values = RawJson::parse(br#"{"extra":true}"#).expect("settings");
        assert_eq!(
            validate_model_request(&exact_model, &byte_over, &locked)
                .expect_err("bytes + 1")
                .code(),
            MODEL_CONTEXT_LIMIT_EXCEEDED
        );

        let mut output_over = request;
        output_over.limits.max_output_tokens = 21;
        assert_eq!(
            validate_model_request(&exact_model, &output_over, &locked)
                .expect_err("output + 1")
                .code(),
            MODEL_CONTEXT_LIMIT_EXCEEDED
        );
    }

    #[test]
    fn request_validator_rejects_estimator_identity_mismatch() {
        let request = draft();
        let provider = profile();
        let locked =
            resolve_model_context_profile(provider.clone(), None, None, false).expect("profile");
        let model = EstimatorModel {
            profile: provider,
            estimate: 1,
            estimator: TokenEstimatorRef {
                id: Arc::from("different"),
                version: Arc::from("1"),
                source: TokenEstimatorSource::ConservativeUpperBound,
            },
        };
        assert_eq!(
            validate_model_request(&model, &request, &locked)
                .expect_err("estimator mismatch")
                .code(),
            MODEL_ESTIMATOR_MISMATCH
        );
    }

    #[test]
    fn reserved_adapter_codes_enforce_category_and_retryability() {
        let error = ModelError::try_new(
            MODEL_STREAM_LIMIT_EXCEEDED,
            ErrorCategory::Validation,
            false,
            "wrong category",
            Metadata::empty(),
        )
        .expect_err("reserved category");
        assert_eq!(ModelError::from(error).code(), MODEL_REQUEST_INVALID);

        let error = ModelError::try_new(
            MODEL_RESPONSE_MISMATCH,
            ErrorCategory::Validation,
            true,
            "wrong retryability",
            Metadata::empty(),
        )
        .expect_err("reserved retryability");
        assert_eq!(ModelError::from(error).code(), MODEL_REQUEST_INVALID);

        let exact = ModelError::try_new(
            MODEL_STREAM_LIMIT_EXCEEDED,
            ErrorCategory::Limit,
            false,
            "bounded",
            Metadata::empty(),
        )
        .expect("reserved classification");
        assert_eq!(exact.category(), ErrorCategory::Limit);
        assert!(!exact.retryable());
    }

    fn id<T: finstack_ai_kernel::IdTag>(ordinal: u64) -> finstack_ai_kernel::Id<T> {
        let mut bytes = [0_u8; 16];
        bytes[6] = 0x70;
        bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        finstack_ai_kernel::Id::from_bytes(bytes)
    }

    fn requested(retry_safety: RetrySafety) -> finstack_ai_kernel::EffectRequested {
        finstack_ai_kernel::EffectRequested::try_new(
            id(4),
            finstack_ai_kernel::EffectKind::Model,
            None,
            None,
            None,
            finstack_ai_kernel::EffectOutputContract {
                kind: finstack_ai_kernel::EffectOutputKind::ModelResponse,
                schema_version: 1,
                schema_digest: Digest::raw_json(b"model-response"),
            },
            finstack_ai_kernel::EffectInput::Model {
                request: RawJson::parse(b"{}").expect("request"),
            },
            retry_safety,
            None,
        )
        .expect("requested")
    }

    fn pending(
        deferred: Option<finstack_ai_kernel::EffectDeferred>,
    ) -> finstack_ai_kernel::PendingModelEffect {
        finstack_ai_kernel::PendingModelEffect {
            cycle: 0,
            turn_id: id(7),
            model_request_id: id(5),
            requested: requested(RetrySafety::SafeToRetry),
            deferred,
        }
    }

    fn deferred(policy: ReconciliationPolicy) -> finstack_ai_kernel::EffectDeferred {
        finstack_ai_kernel::EffectDeferred {
            effect_id: id(4),
            handle: ExternalHandleRef::try_new(
                finstack_ai_kernel::ComponentId::parse("finstack.model.scripted")
                    .expect("component"),
                "handle-1",
                RawJson::parse(b"{}").expect("metadata"),
            )
            .expect("handle"),
            reconciliation: policy,
            next_poll_at: None,
            expires_at: None,
            output_contract: requested(RetrySafety::SafeToRetry)
                .output_contract()
                .clone(),
        }
    }

    fn state_with(
        phase: Option<RunPhase>,
        pending_effect: Option<finstack_ai_kernel::PendingModelEffect>,
        settled: bool,
    ) -> KernelState {
        let mut state = KernelState {
            phase,
            pending_model_effect: pending_effect,
            ..KernelState::default()
        };
        if settled && let Some(pending) = state.pending_model_effect.as_ref() {
            let effect_id = pending.requested.effect_id();
            state.model_settlements.insert(
                effect_id,
                finstack_ai_kernel::ModelSettlementFingerprint {
                    kind: finstack_ai_kernel::ModelSettlementKind::Completed,
                    digest: Digest::raw_json(b"settled"),
                },
            );
        }
        state
    }

    #[test]
    fn model_resume_action_classifies_journal_only_states() {
        assert_eq!(
            model_resume_action(&KernelState::default()),
            ModelResumeAction::NoOutstanding
        );
        assert_eq!(
            model_resume_action(&state_with(
                Some(RunPhase::AwaitingModel),
                Some(pending(None)),
                false
            )),
            ModelResumeAction::Reconcile
        );
        assert_eq!(
            model_resume_action(&state_with(
                Some(RunPhase::AwaitingModel),
                Some(pending(None)),
                true
            )),
            ModelResumeAction::UseRecorded
        );
        assert_eq!(
            model_resume_action(&state_with(
                Some(RunPhase::AwaitingExternal),
                Some(pending(Some(deferred(ReconciliationPolicy::CallbackOnly)))),
                false
            )),
            ModelResumeAction::WaitExternal
        );
        assert_eq!(
            model_resume_action(&state_with(
                Some(RunPhase::AwaitingExternal),
                Some(pending(Some(deferred(ReconciliationPolicy::Poll)))),
                false
            )),
            ModelResumeAction::Reconcile
        );
        assert_eq!(
            model_resume_action(&state_with(
                Some(RunPhase::AwaitingExternal),
                Some(pending(Some(deferred(
                    ReconciliationPolicy::CallbackOrPoll
                )))),
                false
            )),
            ModelResumeAction::Reconcile
        );
        let settled = state_with(Some(RunPhase::BeforeFinalize), None, false);
        assert_eq!(
            model_resume_action(&settled),
            ModelResumeAction::NoOutstanding
        );
        assert_ne!(
            model_resume_action(&state_with(
                Some(RunPhase::AwaitingModel),
                Some(pending(None)),
                false
            )),
            ModelResumeAction::Retry
        );
    }

    #[test]
    fn model_resume_maps_reconcile_results_to_documented_actions() {
        let awaiting = state_with(Some(RunPhase::AwaitingModel), Some(pending(None)), false);
        let deferred_state = state_with(
            Some(RunPhase::AwaitingExternal),
            Some(pending(Some(deferred(
                ReconciliationPolicy::CallbackOrPoll,
            )))),
            false,
        );
        let response = ModelResponse {
            assistant_content: Arc::from([]),
            tool_calls: Arc::from([]),
            usage: Usage::empty(),
            provider_ids: ProviderIds::empty(),
            completion_id: Arc::from("completion-1"),
            continuation_state: None,
        };
        assert_eq!(
            map_model_reconcile_result(
                &awaiting,
                &ModelReconcileResult::Completed(response.clone()),
                true
            ),
            ModelResumeAction::UseRecorded
        );
        assert_eq!(
            map_model_reconcile_result(
                &awaiting,
                &ModelReconcileResult::StillRunning(ModelDeferral {
                    handle: deferred(ReconciliationPolicy::CallbackOrPoll).handle,
                    reconciliation: ReconciliationPolicy::CallbackOrPoll,
                    next_poll_at: None,
                    expires_at: None,
                }),
                true
            ),
            ModelResumeAction::WaitExternal
        );
        assert_eq!(
            map_model_reconcile_result(&awaiting, &ModelReconcileResult::NotStarted, true),
            ModelResumeAction::Retry
        );
        assert_eq!(
            map_model_reconcile_result(&awaiting, &ModelReconcileResult::Unknown, true),
            ModelResumeAction::Retry
        );
        assert_eq!(
            map_model_reconcile_result(&awaiting, &ModelReconcileResult::Unknown, false),
            ModelResumeAction::SuspendUncertain
        );
        assert_eq!(
            map_model_reconcile_result(&awaiting, &ModelReconcileResult::NonRepeatable, true),
            ModelResumeAction::SuspendUncertain
        );
        assert_eq!(
            map_model_reconcile_result(&deferred_state, &ModelReconcileResult::NotStarted, true),
            ModelResumeAction::SuspendUncertain
        );
        assert_eq!(
            map_model_reconcile_result(
                &state_with(Some(RunPhase::AwaitingModel), Some(pending(None)), true),
                &ModelReconcileResult::Completed(response),
                true
            ),
            ModelResumeAction::UseRecorded
        );
        assert!(!model_retry_allowed(
            &requested(RetrySafety::AtMostOnce),
            &ModelCapabilities {
                input: InputCapabilities {
                    text: true,
                    json: false,
                    images: false,
                    audio: false,
                    files: false,
                },
                context_profile: profile(),
                native_tool_calls: false,
                parallel_tool_calls: false,
                structured_output: StructuredOutputCapability::Unsupported,
                reasoning: false,
                prompt_cache: false,
                resumable_stream: false,
                idempotent_requests: true,
                native_capabilities: BTreeSet::new(),
            }
        ));
    }
}
