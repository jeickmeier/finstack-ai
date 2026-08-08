//! Effect and interaction envelopes (TDD §12.3–§12.4).

use std::sync::Arc;

use serde::de;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::content::{ContentBlock, ContentError, ToolCallBlock, validate_content_items};
use crate::digest::Digest;
use crate::error::ErrorDescriptor;
use crate::ids::{BudgetReservationId, ComponentId, EffectId, EffectOutputKey, InteractionId};
use crate::raw_json::{Metadata, RawJson};
use crate::refs::{
    ArtifactRef, AssigneeHint, AuthorizationEvidence, ComponentRef, ExternalHandleRef,
    PrincipalRef, RefsError, Usage, Version, validated_label,
};
use crate::time::Timestamp;

/// Effect kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectKind {
    /// Model call.
    Model,
    /// Tool call.
    Tool,
    /// Context provider.
    Context,
    /// Middleware stage.
    Middleware,
    /// Interaction.
    Interaction,
    /// Timer.
    Timer,
}

/// Effect execution retry safety.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetrySafety {
    /// Safe to retry.
    SafeToRetry,
    /// Idempotent when keyed.
    IdempotentWithKey,
    /// At most once.
    AtMostOnce,
    /// Unknown.
    Unknown,
}

/// Invocation recovery class for a component.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InvocationRecovery {
    /// Recompute-safe.
    RecomputeSafe,
    /// Reconcile required.
    Reconcile,
    /// Non-repeatable.
    NonRepeatable,
}

/// Component invocation metadata on an effect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComponentInvocation {
    /// Component id.
    pub component: ComponentId,
    /// Component version.
    pub version: Version,
    /// Configuration digest.
    pub configuration_digest: Digest,
    /// Recovery class.
    pub recovery: InvocationRecovery,
}

/// Middleware pipeline position.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PipelinePosition {
    chain_digest: Digest,
    stage: Arc<str>,
    index: u32,
}

impl PipelinePosition {
    /// Construct a pipeline position.
    ///
    /// # Errors
    ///
    /// Returns [`EffectError::Refs`] when `stage` is invalid.
    pub fn try_new(
        chain_digest: Digest,
        stage: impl AsRef<str>,
        index: u32,
    ) -> Result<Self, EffectError> {
        Ok(Self {
            chain_digest,
            stage: validated_label(stage.as_ref(), "stage")?,
            index,
        })
    }
}

impl<'de> Deserialize<'de> for PipelinePosition {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            chain_digest: Digest,
            stage: String,
            index: u32,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(wire.chain_digest, wire.stage, wire.index).map_err(de::Error::custom)
    }
}

/// Parent effect relation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectRelation {
    /// Parent effect id.
    pub parent_effect_id: EffectId,
    /// Purpose.
    pub purpose: EffectPurpose,
}

/// Why a child effect exists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EffectPurpose {
    /// Compaction summary owned by middleware.
    CompactionSummary {
        /// Middleware component id.
        middleware_component_id: ComponentId,
    },
}

/// Normalized effect input.
///
/// Serialized with externally tagged `snake_case` variants so `RawJson` members
/// deserialize through the ordinary human-readable path (internally tagged
/// enums buffer content and break `RawJson`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectInput {
    /// Model request JSON.
    Model {
        /// Request payload.
        request: RawJson,
    },
    /// Tool call block.
    Tool {
        /// Call.
        call: ToolCallBlock,
    },
    /// Context request JSON.
    Context {
        /// Request payload.
        request: RawJson,
    },
    /// Middleware stage input.
    Middleware {
        /// Stage name.
        stage: Arc<str>,
        /// Input payload.
        input: RawJson,
    },
    /// Interaction paired by digest.
    Interaction {
        /// Interaction id.
        interaction_id: InteractionId,
        /// Digest of the paired interaction request.
        request_digest: Digest,
    },
    /// Timer due-at.
    Timer {
        /// Due timestamp.
        due_at: Timestamp,
    },
}

impl EffectInput {
    /// Canonical bytes for `effect-input` digests.
    ///
    /// # Errors
    ///
    /// Returns [`EffectError`] when serialization fails.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, EffectError> {
        let json = serde_json::to_string(self).map_err(|error| EffectError::Serialize {
            detail: error.to_string(),
        })?;
        let value: serde_json::Value =
            serde_json::from_str(&json).map_err(|error| EffectError::Serialize {
                detail: error.to_string(),
            })?;
        let canonical = serde_json_canonicalizer::to_string(&value).map_err(|error| {
            EffectError::Serialize {
                detail: error.to_string(),
            }
        })?;
        Ok(canonical.into_bytes())
    }

    /// Digest under the `effect-input` domain.
    ///
    /// # Errors
    ///
    /// Returns [`EffectError`] when canonicalization fails.
    pub fn digest(&self) -> Result<Digest, EffectError> {
        Ok(Digest::effect_input(&self.canonical_bytes()?))
    }

    /// Matching [`EffectKind`].
    #[must_use]
    pub const fn kind(&self) -> EffectKind {
        match self {
            Self::Model { .. } => EffectKind::Model,
            Self::Tool { .. } => EffectKind::Tool,
            Self::Context { .. } => EffectKind::Context,
            Self::Middleware { .. } => EffectKind::Middleware,
            Self::Interaction { .. } => EffectKind::Interaction,
            Self::Timer { .. } => EffectKind::Timer,
        }
    }
}

/// Output kind for an effect contract.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectOutputKind {
    /// Model response.
    ModelResponse,
    /// Tool result.
    ToolResult,
    /// Context contribution.
    ContextContribution,
    /// Middleware outcome.
    MiddlewareOutcome,
    /// Interaction resolution.
    InteractionResolution,
    /// Timer firing.
    TimerFiring,
    /// Artifact receipt.
    ArtifactReceipt,
    /// Custom namespaced kind.
    Custom {
        /// Custom key.
        key: EffectOutputKey,
    },
}

/// Non-optional output contract carried through deferral/completion.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectOutputContract {
    /// Output kind.
    pub kind: EffectOutputKind,
    /// Schema version.
    pub schema_version: u16,
    /// Schema digest.
    pub schema_digest: Digest,
}

/// Effect requested record body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EffectRequested {
    effect_id: EffectId,
    kind: EffectKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    relation: Option<EffectRelation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    component: Option<ComponentInvocation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pipeline: Option<PipelinePosition>,
    output_contract: EffectOutputContract,
    input: EffectInput,
    input_digest: Digest,
    retry_safety: RetrySafety,
    #[serde(skip_serializing_if = "Option::is_none")]
    deadline: Option<Timestamp>,
}

impl EffectRequested {
    /// Construct an effect request and compute `input_digest`.
    ///
    /// # Errors
    ///
    /// Returns [`EffectError`] when kind/input mismatch or digest canonicalization fails.
    #[allow(clippy::too_many_arguments)]
    pub fn try_new(
        effect_id: EffectId,
        kind: EffectKind,
        relation: Option<EffectRelation>,
        component: Option<ComponentInvocation>,
        pipeline: Option<PipelinePosition>,
        output_contract: EffectOutputContract,
        input: EffectInput,
        retry_safety: RetrySafety,
        deadline: Option<Timestamp>,
    ) -> Result<Self, EffectError> {
        if input.kind() != kind {
            return Err(EffectError::KindMismatch);
        }
        let input_digest = input.digest()?;
        Ok(Self {
            effect_id,
            kind,
            relation,
            component,
            pipeline,
            output_contract,
            input,
            input_digest,
            retry_safety,
            deadline,
        })
    }

    /// Effect id.
    #[must_use]
    pub fn effect_id(&self) -> EffectId {
        self.effect_id
    }

    /// Kind.
    #[must_use]
    pub fn kind(&self) -> EffectKind {
        self.kind
    }

    /// Output contract.
    #[must_use]
    pub fn output_contract(&self) -> &EffectOutputContract {
        &self.output_contract
    }

    /// Input.
    #[must_use]
    pub fn input(&self) -> &EffectInput {
        &self.input
    }

    /// Input digest.
    #[must_use]
    pub fn input_digest(&self) -> Digest {
        self.input_digest
    }

    /// Retry safety.
    #[must_use]
    pub fn retry_safety(&self) -> RetrySafety {
        self.retry_safety
    }
}

impl<'de> Deserialize<'de> for EffectRequested {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            effect_id: EffectId,
            kind: EffectKind,
            #[serde(default)]
            relation: Option<EffectRelation>,
            #[serde(default)]
            component: Option<ComponentInvocation>,
            #[serde(default)]
            pipeline: Option<PipelinePosition>,
            output_contract: EffectOutputContract,
            input: EffectInput,
            input_digest: Digest,
            retry_safety: RetrySafety,
            #[serde(default)]
            deadline: Option<Timestamp>,
        }
        let wire = Wire::deserialize(deserializer)?;
        let constructed = Self::try_new(
            wire.effect_id,
            wire.kind,
            wire.relation,
            wire.component,
            wire.pipeline,
            wire.output_contract,
            wire.input,
            wire.retry_safety,
            wire.deadline,
        )
        .map_err(de::Error::custom)?;
        if constructed.input_digest != wire.input_digest {
            return Err(de::Error::custom("input_digest mismatch"));
        }
        Ok(constructed)
    }
}

/// Deferred external effect handle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectDeferred {
    /// Effect id (unchanged).
    pub effect_id: EffectId,
    /// External handle.
    pub handle: ExternalHandleRef,
    /// Reconciliation policy.
    pub reconciliation: ReconciliationPolicy,
    /// Next poll time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_poll_at: Option<Timestamp>,
    /// Expiry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<Timestamp>,
    /// Output contract copied from the request.
    pub output_contract: EffectOutputContract,
}

/// How to reconcile a deferred effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReconciliationPolicy {
    /// Callback only.
    CallbackOnly,
    /// Poll.
    Poll,
    /// Callback or poll.
    CallbackOrPoll,
    /// External workflow.
    ExternalWorkflow,
}

/// Successful effect completion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EffectCompleted {
    effect_id: EffectId,
    output_contract: EffectOutputContract,
    output: RawJson,
    output_digest: Digest,
    #[serde(skip_serializing_if = "Option::is_none")]
    usage: Option<Usage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    usage_digest: Option<Digest>,
    artifacts: Arc<[ArtifactRef]>,
    provider_ids: crate::message::ProviderIds,
    #[serde(skip_serializing_if = "Option::is_none")]
    completion_id: Option<Arc<str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reservation_id: Option<BudgetReservationId>,
}

impl EffectCompleted {
    /// Construct a completion, computing output/usage digests.
    ///
    /// # Errors
    ///
    /// Returns [`EffectError`] on digest/label failures or usage/digest pairing errors.
    #[allow(clippy::too_many_arguments)]
    pub fn try_new(
        effect_id: EffectId,
        output_contract: EffectOutputContract,
        output: RawJson,
        usage: Option<Usage>,
        artifacts: Vec<ArtifactRef>,
        provider_ids: crate::message::ProviderIds,
        completion_id: Option<impl AsRef<str>>,
        reservation_id: Option<BudgetReservationId>,
    ) -> Result<Self, EffectError> {
        let usage_digest = match &usage {
            Some(value) => Some(Digest::effect_output(&value.canonical_bytes()?)),
            None => None,
        };
        let completion_id = match completion_id {
            Some(value) => Some(validated_label(value.as_ref(), "completion_id")?),
            None => None,
        };
        Ok(Self {
            effect_id,
            output_contract,
            output_digest: Digest::effect_output(output.as_str().as_bytes()),
            output,
            usage,
            usage_digest,
            artifacts: artifacts.into(),
            provider_ids,
            completion_id,
            reservation_id,
        })
    }

    /// Effect id.
    #[must_use]
    pub fn effect_id(&self) -> EffectId {
        self.effect_id
    }

    /// Output contract.
    #[must_use]
    pub fn output_contract(&self) -> &EffectOutputContract {
        &self.output_contract
    }

    /// Output digest.
    #[must_use]
    pub fn output_digest(&self) -> Digest {
        self.output_digest
    }
}

impl<'de> Deserialize<'de> for EffectCompleted {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            effect_id: EffectId,
            output_contract: EffectOutputContract,
            output: RawJson,
            output_digest: Digest,
            #[serde(default)]
            usage: Option<Usage>,
            #[serde(default)]
            usage_digest: Option<Digest>,
            #[serde(default)]
            artifacts: Vec<ArtifactRef>,
            provider_ids: crate::message::ProviderIds,
            #[serde(default)]
            completion_id: Option<String>,
            #[serde(default)]
            reservation_id: Option<BudgetReservationId>,
        }
        let wire = Wire::deserialize(deserializer)?;
        let constructed = Self::try_new(
            wire.effect_id,
            wire.output_contract,
            wire.output,
            wire.usage,
            wire.artifacts,
            wire.provider_ids,
            wire.completion_id,
            wire.reservation_id,
        )
        .map_err(de::Error::custom)?;
        if constructed.output_digest != wire.output_digest
            || constructed.usage_digest != wire.usage_digest
        {
            return Err(de::Error::custom("completion digest mismatch"));
        }
        Ok(constructed)
    }
}

/// Failed effect completion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EffectFailed {
    effect_id: EffectId,
    output_contract: EffectOutputContract,
    error: ErrorDescriptor,
    #[serde(skip_serializing_if = "Option::is_none")]
    usage: Option<Usage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    usage_digest: Option<Digest>,
    #[serde(skip_serializing_if = "Option::is_none")]
    completion_id: Option<Arc<str>>,
}

impl EffectFailed {
    /// Construct a failed completion.
    ///
    /// # Errors
    ///
    /// Returns [`EffectError`] on usage/digest pairing or label failures.
    pub fn try_new(
        effect_id: EffectId,
        output_contract: EffectOutputContract,
        error: ErrorDescriptor,
        usage: Option<Usage>,
        completion_id: Option<impl AsRef<str>>,
    ) -> Result<Self, EffectError> {
        let usage_digest = match &usage {
            Some(value) => Some(Digest::effect_output(&value.canonical_bytes()?)),
            None => None,
        };
        let completion_id = match completion_id {
            Some(value) => Some(validated_label(value.as_ref(), "completion_id")?),
            None => None,
        };
        Ok(Self {
            effect_id,
            output_contract,
            error,
            usage,
            usage_digest,
            completion_id,
        })
    }

    /// Effect id.
    #[must_use]
    pub fn effect_id(&self) -> EffectId {
        self.effect_id
    }
}

impl<'de> Deserialize<'de> for EffectFailed {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            effect_id: EffectId,
            output_contract: EffectOutputContract,
            error: ErrorDescriptor,
            #[serde(default)]
            usage: Option<Usage>,
            #[serde(default)]
            usage_digest: Option<Digest>,
            #[serde(default)]
            completion_id: Option<String>,
        }
        let wire = Wire::deserialize(deserializer)?;
        let constructed = Self::try_new(
            wire.effect_id,
            wire.output_contract,
            wire.error,
            wire.usage,
            wire.completion_id,
        )
        .map_err(de::Error::custom)?;
        if constructed.usage_digest != wire.usage_digest {
            return Err(de::Error::custom("usage_digest mismatch"));
        }
        Ok(constructed)
    }
}

/// Cancelled effect completion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EffectCancelled {
    effect_id: EffectId,
    output_contract: EffectOutputContract,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<Arc<str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    completion_id: Option<Arc<str>>,
}

impl EffectCancelled {
    /// Construct a cancellation record.
    ///
    /// # Errors
    ///
    /// Returns [`EffectError`] when labels fail validation.
    pub fn try_new(
        effect_id: EffectId,
        output_contract: EffectOutputContract,
        reason: Option<impl AsRef<str>>,
        completion_id: Option<impl AsRef<str>>,
    ) -> Result<Self, EffectError> {
        let reason = match reason {
            Some(value) => Some(validated_label(value.as_ref(), "reason")?),
            None => None,
        };
        let completion_id = match completion_id {
            Some(value) => Some(validated_label(value.as_ref(), "completion_id")?),
            None => None,
        };
        Ok(Self {
            effect_id,
            output_contract,
            reason,
            completion_id,
        })
    }

    /// Effect id.
    #[must_use]
    pub fn effect_id(&self) -> EffectId {
        self.effect_id
    }
}

impl<'de> Deserialize<'de> for EffectCancelled {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            effect_id: EffectId,
            output_contract: EffectOutputContract,
            #[serde(default)]
            reason: Option<String>,
            #[serde(default)]
            completion_id: Option<String>,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(
            wire.effect_id,
            wire.output_contract,
            wire.reason,
            wire.completion_id,
        )
        .map_err(de::Error::custom)
    }
}

/// Interaction kind (approval is a profile, not a separate record family).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InteractionKind {
    /// Approval profile.
    Approval,
    /// Choice.
    Choice,
    /// Form.
    Form,
    /// Free text.
    FreeText,
    /// Review.
    Review,
    /// Correction.
    Correction,
    /// Custom.
    Custom {
        /// Custom name.
        name: Arc<str>,
    },
}

/// Interaction request payload (`InteractionRequested` record body).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InteractionRequest {
    request_version: u16,
    interaction_id: InteractionId,
    effect_id: EffectId,
    kind: InteractionKind,
    prompt: Arc<[ContentBlock]>,
    prompt_digest: Digest,
    response_schema: RawJson,
    response_schema_digest: Digest,
    policy_component: ComponentRef,
    policy_version: Version,
    #[serde(skip_serializing_if = "Option::is_none")]
    assignee_hint: Option<AssigneeHint>,
    #[serde(skip_serializing_if = "Option::is_none")]
    expires_at: Option<Timestamp>,
    delegatable: bool,
    metadata: Metadata,
}

impl InteractionRequest {
    /// Construct an interaction request and compute digests.
    ///
    /// # Errors
    ///
    /// Returns [`EffectError`] on content/digest failures.
    #[allow(clippy::too_many_arguments)]
    pub fn try_new(
        request_version: u16,
        interaction_id: InteractionId,
        effect_id: EffectId,
        kind: InteractionKind,
        prompt: Vec<ContentBlock>,
        response_schema: RawJson,
        policy_component: ComponentRef,
        policy_version: Version,
        assignee_hint: Option<AssigneeHint>,
        expires_at: Option<Timestamp>,
        delegatable: bool,
        metadata: Metadata,
    ) -> Result<Self, EffectError> {
        validate_content_items(&prompt)?;
        let prompt_json =
            serde_json::to_string(&prompt).map_err(|error| EffectError::Serialize {
                detail: error.to_string(),
            })?;
        let prompt_value: serde_json::Value =
            serde_json::from_str(&prompt_json).map_err(|error| EffectError::Serialize {
                detail: error.to_string(),
            })?;
        let prompt_canonical =
            serde_json_canonicalizer::to_string(&prompt_value).map_err(|error| {
                EffectError::Serialize {
                    detail: error.to_string(),
                }
            })?;
        Ok(Self {
            request_version,
            interaction_id,
            effect_id,
            kind,
            prompt: prompt.into(),
            prompt_digest: Digest::effect_input(prompt_canonical.as_bytes()),
            response_schema_digest: Digest::effect_input(response_schema.as_str().as_bytes()),
            response_schema,
            policy_component,
            policy_version,
            assignee_hint,
            expires_at,
            delegatable,
            metadata,
        })
    }

    /// Interaction id.
    #[must_use]
    pub fn interaction_id(&self) -> InteractionId {
        self.interaction_id
    }

    /// Effect id.
    #[must_use]
    pub fn effect_id(&self) -> EffectId {
        self.effect_id
    }

    /// Kind.
    #[must_use]
    pub fn kind(&self) -> &InteractionKind {
        &self.kind
    }

    /// Prompt digest.
    #[must_use]
    pub fn prompt_digest(&self) -> Digest {
        self.prompt_digest
    }

    /// Request digest for pairing with `EffectInput::Interaction`.
    ///
    /// # Errors
    ///
    /// Returns [`EffectError`] when serialization fails.
    pub fn request_digest(&self) -> Result<Digest, EffectError> {
        let json = serde_json::to_string(self).map_err(|error| EffectError::Serialize {
            detail: error.to_string(),
        })?;
        let value: serde_json::Value =
            serde_json::from_str(&json).map_err(|error| EffectError::Serialize {
                detail: error.to_string(),
            })?;
        let canonical = serde_json_canonicalizer::to_string(&value).map_err(|error| {
            EffectError::Serialize {
                detail: error.to_string(),
            }
        })?;
        Ok(Digest::effect_input(canonical.as_bytes()))
    }
}

impl<'de> Deserialize<'de> for InteractionRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            request_version: u16,
            interaction_id: InteractionId,
            effect_id: EffectId,
            kind: InteractionKind,
            prompt: Vec<ContentBlock>,
            prompt_digest: Digest,
            response_schema: RawJson,
            response_schema_digest: Digest,
            policy_component: ComponentRef,
            policy_version: Version,
            #[serde(default)]
            assignee_hint: Option<AssigneeHint>,
            #[serde(default)]
            expires_at: Option<Timestamp>,
            delegatable: bool,
            metadata: Metadata,
        }
        let wire = Wire::deserialize(deserializer)?;
        let constructed = Self::try_new(
            wire.request_version,
            wire.interaction_id,
            wire.effect_id,
            wire.kind,
            wire.prompt,
            wire.response_schema,
            wire.policy_component,
            wire.policy_version,
            wire.assignee_hint,
            wire.expires_at,
            wire.delegatable,
            wire.metadata,
        )
        .map_err(de::Error::custom)?;
        if constructed.prompt_digest != wire.prompt_digest
            || constructed.response_schema_digest != wire.response_schema_digest
        {
            return Err(de::Error::custom("interaction digest mismatch"));
        }
        Ok(constructed)
    }
}

/// Interaction resolution payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InteractionResolution {
    interaction_id: InteractionId,
    resolution_id: Arc<str>,
    principal: PrincipalRef,
    authorization: AuthorizationEvidence,
    response: RawJson,
    #[serde(skip_serializing_if = "Option::is_none")]
    comment: Option<Arc<str>>,
}

impl InteractionResolution {
    /// Construct a resolution.
    ///
    /// # Errors
    ///
    /// Returns [`EffectError`] when labels fail validation.
    pub fn try_new(
        interaction_id: InteractionId,
        resolution_id: impl AsRef<str>,
        principal: PrincipalRef,
        authorization: AuthorizationEvidence,
        response: RawJson,
        comment: Option<impl AsRef<str>>,
    ) -> Result<Self, EffectError> {
        Ok(Self {
            interaction_id,
            resolution_id: validated_label(resolution_id.as_ref(), "resolution_id")?,
            principal,
            authorization,
            response,
            comment: match comment {
                Some(value) => Some(validated_label(value.as_ref(), "comment")?),
                None => None,
            },
        })
    }

    /// Interaction id.
    #[must_use]
    pub fn interaction_id(&self) -> InteractionId {
        self.interaction_id
    }
}

impl<'de> Deserialize<'de> for InteractionResolution {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            interaction_id: InteractionId,
            resolution_id: String,
            principal: PrincipalRef,
            authorization: AuthorizationEvidence,
            response: RawJson,
            #[serde(default)]
            comment: Option<String>,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(
            wire.interaction_id,
            wire.resolution_id,
            wire.principal,
            wire.authorization,
            wire.response,
            wire.comment,
        )
        .map_err(de::Error::custom)
    }
}

/// Interaction expired.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionExpired {
    /// Interaction id.
    pub interaction_id: InteractionId,
    /// Expiry timestamp.
    pub expired_at: Timestamp,
}

/// Interaction cancelled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionCancelled {
    /// Interaction id.
    pub interaction_id: InteractionId,
    /// Optional principal for principal-initiated cancellation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub principal: Option<PrincipalRef>,
    /// Optional authorization paired with principal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorization: Option<AuthorizationEvidence>,
    /// Optional reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<Arc<str>>,
}

impl InteractionCancelled {
    /// Construct a cancellation with principal/authorization pairing rules.
    ///
    /// # Errors
    ///
    /// Returns [`EffectError::InvalidCancellationPair`] when exactly one of
    /// principal/authorization is present.
    pub fn try_new(
        interaction_id: InteractionId,
        principal: Option<PrincipalRef>,
        authorization: Option<AuthorizationEvidence>,
        reason: Option<impl AsRef<str>>,
    ) -> Result<Self, EffectError> {
        match (&principal, &authorization) {
            (Some(_), Some(_)) | (None, None) => {}
            _ => return Err(EffectError::InvalidCancellationPair),
        }
        Ok(Self {
            interaction_id,
            principal,
            authorization,
            reason: match reason {
                Some(value) => Some(validated_label(value.as_ref(), "reason")?),
                None => None,
            },
        })
    }
}

/// Effect/interaction errors.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum EffectError {
    /// Effect kind did not match input variant.
    #[error("effect kind/input mismatch")]
    KindMismatch,
    /// Principal/authorization pairing invalid.
    #[error("interaction cancellation requires both principal and authorization or neither")]
    InvalidCancellationPair,
    /// Label error.
    #[error(transparent)]
    Refs(#[from] RefsError),
    /// Content error.
    #[error(transparent)]
    Content(#[from] ContentError),
    /// Serialization failed.
    #[error("serialize failed: {detail}")]
    Serialize {
        /// Detail.
        detail: String,
    },
}

impl EffectError {
    /// Stable error code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::KindMismatch => "effect_kind_mismatch",
            Self::InvalidCancellationPair => "invalid_cancellation_pair",
            Self::Refs(inner) => inner.code(),
            Self::Content(_) => "invalid_content",
            Self::Serialize { .. } => "serialize_failed",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raw_json::RawJson;

    #[test]
    fn effect_input_external_tag_round_trips_raw_json() {
        let input = EffectInput::Model {
            request: RawJson::parse(r#"{"a":1}"#).expect("json"),
        };
        let json = serde_json::to_string(&input).expect("ser");
        assert_eq!(json, r#"{"model":{"request":{"a":1}}}"#);
        let round: EffectInput = serde_json::from_str(&json).expect("de");
        assert_eq!(round, input);
        assert_eq!(
            input.digest().expect("digest").to_hex(),
            "c59ec0a2d3310a96d14147f8ea38ec979fe0623a5aa3ad6f15981ae26e67eab7"
        );
    }

    #[test]
    fn effect_request_digests_input() {
        let effect_id = EffectId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("id");
        let input = EffectInput::Model {
            request: RawJson::parse(r#"{"b":1,"a":2}"#).expect("json"),
        };
        let contract = EffectOutputContract {
            kind: EffectOutputKind::ModelResponse,
            schema_version: 1,
            schema_digest: Digest::raw_json(br#"{"schema":1}"#),
        };
        let requested = EffectRequested::try_new(
            effect_id,
            EffectKind::Model,
            None,
            None,
            None,
            contract,
            input,
            RetrySafety::SafeToRetry,
            None,
        )
        .expect("requested");
        assert_eq!(
            requested.input_digest(),
            requested.input().digest().expect("digest")
        );
        assert!(
            EffectRequested::try_new(
                effect_id,
                EffectKind::Tool,
                None,
                None,
                None,
                requested.output_contract().clone(),
                EffectInput::Model {
                    request: RawJson::parse("{}").expect("json"),
                },
                RetrySafety::Unknown,
                None,
            )
            .is_err()
        );
    }
}
