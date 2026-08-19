use std::collections::BTreeSet;
use std::sync::Arc;

use finstack_ai_kernel::{
    ContentBlock, ExternalHandleRef, Message, Metadata, OutputSpec, ProviderIds, RawJson,
    ReconciliationPolicy, RetrySafety, Timestamp, ToolExecutionMode, ToolId, Usage,
};
use serde::{Deserialize, Serialize};

use super::MODEL_REQUEST_INVALID;
use super::error::{ModelError, STREAM_TEXT_MAX_BYTES};
use super::identity::{ModelName, validated_label};
use super::profile::TokenEstimatorRef;

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

/// Declares whether a tool may return a deferred outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolDeferralSupport {
    /// The tool must complete within the active call.
    #[default]
    Never,
    /// The tool may defer completion for later reconciliation.
    Supported,
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
    /// Whether the tool may return a deferred outcome.
    #[serde(default)]
    pub deferral: ToolDeferralSupport,
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
    /// Provider-native call identity that must be replayed on the next turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_call_id: Option<Arc<str>>,
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

/// Successful pre-commit request validation result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModelRequestValidation {
    /// Exact canonical request bytes.
    pub canonical_request: Arc<[u8]>,
    /// Bound model-supplied token estimate.
    pub estimated_input_tokens: u64,
    /// Available input-token budget after safety margins.
    pub available_input_tokens: u64,
}
