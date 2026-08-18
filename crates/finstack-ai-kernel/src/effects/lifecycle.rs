//! Effect request, deferral, and settlement envelopes.

use std::sync::Arc;

use serde::de;
use serde::{Deserialize, Serialize};

use crate::content::{BoundedString, LABEL_MAX_BYTES};
use crate::primitives::Digest;
use crate::primitives::ErrorDescriptor;
use crate::primitives::RawJson;
use crate::primitives::Timestamp;
use crate::primitives::{ArtifactRef, ExternalHandleRef, Usage, validated_label, validated_text};
use crate::primitives::{BoundedVec, SEMANTIC_ARRAY_MAX_ITEMS};
use crate::primitives::{BudgetReservationId, EffectId};

use super::EffectError;
use super::kinds::{
    ComponentInvocation, EffectInput, EffectKind, EffectOutputContract, EffectRelation,
    PipelinePosition, RetrySafety,
};

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
    /// # Arguments
    ///
    /// * `effect_id` - Stable effect identity allocated for this request.
    /// * `kind` - Effect family. Must match `input`.
    /// * `relation` - Optional parent/child effect relation; `None` for a root effect.
    /// * `component` - Optional resolved component invocation; `None` when unused.
    /// * `pipeline` - Optional middleware pipeline position; `None` when unused.
    /// * `output_contract` - Required output shape for completion.
    /// * `input` - Kind-specific input payload. Its digest is computed here.
    /// * `retry_safety` - Whether the host may retry the effect.
    /// * `deadline` - Optional semantic deadline; `None` means no deadline.
    ///
    /// # Errors
    ///
    /// Returns [`EffectError`] when kind/input mismatch or digest canonicalization fails.
    ///
    /// # Examples
    ///
    /// ```
    /// use finstack_ai_kernel::{
    ///     Digest, EffectId, EffectInput, EffectKind, EffectOutputContract, EffectOutputKind,
    ///     EffectRequested, RawJson, RetrySafety,
    /// };
    ///
    /// # let effect_id = EffectId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("id");
    /// # let contract = EffectOutputContract {
    /// #     kind: EffectOutputKind::ModelResponse,
    /// #     schema_version: 1,
    /// #     schema_digest: Digest::raw_json(b"{}"),
    /// # };
    /// let requested = EffectRequested::try_new(
    ///     effect_id,
    ///     EffectKind::Model,
    ///     None,
    ///     None,
    ///     None,
    ///     contract,
    ///     EffectInput::Model {
    ///         request: RawJson::parse(r#"{"messages":[]}"#).expect("request"),
    ///     },
    ///     RetrySafety::SafeToRetry,
    ///     None,
    /// )
    /// .expect("requested");
    /// assert_eq!(requested.kind(), EffectKind::Model);
    /// ```
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
        if let EffectInput::Middleware { stage, .. } = &input {
            validated_text(stage, "stage")?;
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

    /// Parent-effect relation.
    #[must_use]
    pub fn relation(&self) -> Option<&EffectRelation> {
        self.relation.as_ref()
    }

    /// Whether this request is a runtime-owned compaction-summary child.
    #[must_use]
    pub fn is_compaction_summary(&self) -> bool {
        self.kind == EffectKind::Model
            && self.relation.as_ref().is_some_and(|relation| {
                matches!(
                    relation.purpose,
                    super::kinds::EffectPurpose::CompactionSummary { .. }
                )
            })
    }

    /// Whether this request is a nested model child of an open parent effect.
    #[must_use]
    pub fn is_nested_model(&self) -> bool {
        self.kind == EffectKind::Model
            && self.relation.as_ref().is_some_and(|relation| {
                matches!(
                    relation.purpose,
                    super::kinds::EffectPurpose::NestedModel { .. }
                )
            })
    }

    /// Compaction or nested model: no assistant turn entry.
    #[must_use]
    pub fn is_runtime_owned_child_model(&self) -> bool {
        self.is_compaction_summary() || self.is_nested_model()
    }

    /// Resolved component invocation metadata.
    #[must_use]
    pub fn component(&self) -> Option<&ComponentInvocation> {
        self.component.as_ref()
    }

    /// Middleware pipeline position.
    #[must_use]
    pub fn pipeline(&self) -> Option<&PipelinePosition> {
        self.pipeline.as_ref()
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

    /// Optional effect deadline.
    #[must_use]
    pub fn deadline(&self) -> Option<Timestamp> {
        self.deadline
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

impl EffectDeferred {
    /// Validate identity and output-contract continuity against the originating request.
    ///
    /// # Errors
    ///
    /// Returns [`EffectError::SettlementMismatch`] on any mismatch.
    pub fn validate_against(&self, requested: &EffectRequested) -> Result<(), EffectError> {
        validate_settlement(requested, self.effect_id, &self.output_contract)
    }
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
    provider_ids: crate::conversation::ProviderIds,
    #[serde(skip_serializing_if = "Option::is_none")]
    completion_id: Option<Arc<str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reservation_id: Option<BudgetReservationId>,
}

impl EffectCompleted {
    /// Construct a completion, computing output/usage digests.
    ///
    /// # Arguments
    ///
    /// * `effect_id` - Identity of the completed effect.
    /// * `output_contract` - Output contract frozen on the request.
    /// * `output` - Canonical output JSON. Its digest is computed here.
    /// * `usage` - Optional normalized usage; `None` omits usage and its digest.
    /// * `artifacts` - Produced artifacts, bounded by the v1 array ceiling.
    /// * `provider_ids` - Opaque provider correlation identifiers.
    /// * `completion_id` - Optional provider completion label; `None` omits it.
    /// * `reservation_id` - Optional budget reservation settled by this completion.
    ///
    /// # Errors
    ///
    /// Returns [`EffectError`] on digest/label failures or usage/digest pairing errors.
    ///
    /// # Examples
    ///
    /// ```
    /// use finstack_ai_kernel::{
    ///     Digest, EffectCompleted, EffectId, EffectOutputContract, EffectOutputKind, ProviderIds,
    ///     RawJson,
    /// };
    ///
    /// # let effect_id = EffectId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("id");
    /// # let contract = EffectOutputContract {
    /// #     kind: EffectOutputKind::ModelResponse,
    /// #     schema_version: 1,
    /// #     schema_digest: Digest::raw_json(b"{}"),
    /// # };
    /// let completed = EffectCompleted::try_new(
    ///     effect_id,
    ///     contract,
    ///     RawJson::parse(r#"{"text":"done"}"#).expect("output"),
    ///     None,
    ///     vec![],
    ///     ProviderIds::empty(),
    ///     None::<&str>,
    ///     None,
    /// )
    /// .expect("completed");
    /// assert!(completed.usage().is_none());
    /// ```
    #[allow(clippy::too_many_arguments)]
    pub fn try_new(
        effect_id: EffectId,
        output_contract: EffectOutputContract,
        output: RawJson,
        usage: Option<Usage>,
        artifacts: Vec<ArtifactRef>,
        provider_ids: crate::conversation::ProviderIds,
        completion_id: Option<impl AsRef<str>>,
        reservation_id: Option<BudgetReservationId>,
    ) -> Result<Self, EffectError> {
        if artifacts.len() > SEMANTIC_ARRAY_MAX_ITEMS {
            return Err(EffectError::TooManyItems {
                field: "artifacts",
                len: artifacts.len(),
                max: SEMANTIC_ARRAY_MAX_ITEMS,
            });
        }
        let usage_digest = match &usage {
            Some(value) => {
                value.validate()?;
                Some(Digest::effect_output(&value.canonical_bytes()?))
            }
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

    /// Normalized output.
    #[must_use]
    pub fn output(&self) -> &RawJson {
        &self.output
    }

    /// Optional normalized usage.
    #[must_use]
    pub fn usage(&self) -> Option<&Usage> {
        self.usage.as_ref()
    }

    /// Optional usage digest.
    #[must_use]
    pub fn usage_digest(&self) -> Option<Digest> {
        self.usage_digest
    }

    /// Staged replay-required artifacts.
    #[must_use]
    pub fn artifacts(&self) -> &[ArtifactRef] {
        &self.artifacts
    }

    /// Provider/tool identifiers.
    #[must_use]
    pub fn provider_ids(&self) -> &crate::conversation::ProviderIds {
        &self.provider_ids
    }

    /// External completion id.
    #[must_use]
    pub fn completion_id(&self) -> Option<&str> {
        self.completion_id.as_deref()
    }

    /// Budget reservation id charged by this completion.
    #[must_use]
    pub fn reservation_id(&self) -> Option<BudgetReservationId> {
        self.reservation_id
    }

    /// Validate identity and output-contract continuity against the originating request.
    ///
    /// # Errors
    ///
    /// Returns [`EffectError::SettlementMismatch`] on any mismatch.
    pub fn validate_against(&self, requested: &EffectRequested) -> Result<(), EffectError> {
        validate_settlement(requested, self.effect_id, &self.output_contract)
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
            artifacts: BoundedVec<ArtifactRef, SEMANTIC_ARRAY_MAX_ITEMS>,
            provider_ids: crate::conversation::ProviderIds,
            #[serde(default)]
            completion_id: Option<BoundedString<LABEL_MAX_BYTES>>,
            #[serde(default)]
            reservation_id: Option<BudgetReservationId>,
        }
        let wire = Wire::deserialize(deserializer)?;
        let constructed = Self::try_new(
            wire.effect_id,
            wire.output_contract,
            wire.output,
            wire.usage,
            wire.artifacts.into_inner(),
            wire.provider_ids,
            wire.completion_id.map(BoundedString::into_inner),
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
    /// # Arguments
    ///
    /// * `effect_id` - Identity of the failed effect.
    /// * `output_contract` - Output contract frozen on the request.
    /// * `error` - Safe failure descriptor.
    /// * `usage` - Optional usage observed before failure; `None` omits it.
    /// * `completion_id` - Optional provider completion label; `None` omits it.
    ///
    /// # Errors
    ///
    /// Returns [`EffectError`] on usage/digest pairing or label failures.
    ///
    /// # Examples
    ///
    /// ```
    /// use finstack_ai_kernel::{
    ///     Digest, EffectFailed, EffectId, EffectOutputContract, EffectOutputKind, ErrorCategory,
    ///     ErrorDescriptor,
    /// };
    ///
    /// # let effect_id = EffectId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("id");
    /// # let contract = EffectOutputContract {
    /// #     kind: EffectOutputKind::ModelResponse,
    /// #     schema_version: 1,
    /// #     schema_digest: Digest::raw_json(b"{}"),
    /// # };
    /// # let error = ErrorDescriptor::new(
    /// #     "provider_failed", "provider failed", ErrorCategory::Model, true,
    /// # ).expect("error");
    /// let failed = EffectFailed::try_new(effect_id, contract, error, None, None::<&str>)
    ///     .expect("failed");
    /// assert!(failed.usage().is_none());
    /// ```
    pub fn try_new(
        effect_id: EffectId,
        output_contract: EffectOutputContract,
        error: ErrorDescriptor,
        usage: Option<Usage>,
        completion_id: Option<impl AsRef<str>>,
    ) -> Result<Self, EffectError> {
        error
            .validate()
            .map_err(EffectError::InvalidErrorDescriptor)?;
        let usage_digest = match &usage {
            Some(value) => {
                value.validate()?;
                Some(Digest::effect_output(&value.canonical_bytes()?))
            }
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

    /// Output contract copied from the originating request.
    #[must_use]
    pub fn output_contract(&self) -> &EffectOutputContract {
        &self.output_contract
    }

    /// Failure descriptor.
    #[must_use]
    pub fn error(&self) -> &ErrorDescriptor {
        &self.error
    }

    /// Optional normalized usage.
    #[must_use]
    pub fn usage(&self) -> Option<&Usage> {
        self.usage.as_ref()
    }

    /// Optional usage digest.
    #[must_use]
    pub fn usage_digest(&self) -> Option<Digest> {
        self.usage_digest
    }

    /// External completion id.
    #[must_use]
    pub fn completion_id(&self) -> Option<&str> {
        self.completion_id.as_deref()
    }

    /// Validate identity and output-contract continuity against the originating request.
    ///
    /// # Errors
    ///
    /// Returns [`EffectError::SettlementMismatch`] on any mismatch.
    pub fn validate_against(&self, requested: &EffectRequested) -> Result<(), EffectError> {
        validate_settlement(requested, self.effect_id, &self.output_contract)
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
            completion_id: Option<BoundedString<LABEL_MAX_BYTES>>,
        }
        let wire = Wire::deserialize(deserializer)?;
        let constructed = Self::try_new(
            wire.effect_id,
            wire.output_contract,
            wire.error,
            wire.usage,
            wire.completion_id.map(BoundedString::into_inner),
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
    /// # Arguments
    ///
    /// * `effect_id` - Identity of the cancelled effect.
    /// * `output_contract` - Output contract frozen on the request.
    /// * `reason` - Optional safe cancellation reason; `None` omits it.
    /// * `completion_id` - Optional provider completion label; `None` omits it.
    ///
    /// # Errors
    ///
    /// Returns [`EffectError`] when labels fail validation.
    ///
    /// # Examples
    ///
    /// ```
    /// use finstack_ai_kernel::{
    ///     Digest, EffectCancelled, EffectId, EffectOutputContract, EffectOutputKind,
    /// };
    ///
    /// # let effect_id = EffectId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("id");
    /// # let contract = EffectOutputContract {
    /// #     kind: EffectOutputKind::ModelResponse,
    /// #     schema_version: 1,
    /// #     schema_digest: Digest::raw_json(b"{}"),
    /// # };
    /// let cancelled = EffectCancelled::try_new(effect_id, contract, Some("shutdown"), None::<&str>)
    ///     .expect("cancelled");
    /// assert_eq!(cancelled.reason(), Some("shutdown"));
    /// ```
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

    /// Output contract copied from the originating request.
    #[must_use]
    pub fn output_contract(&self) -> &EffectOutputContract {
        &self.output_contract
    }

    /// Optional cancellation reason.
    #[must_use]
    pub fn reason(&self) -> Option<&str> {
        self.reason.as_deref()
    }

    /// External completion id.
    #[must_use]
    pub fn completion_id(&self) -> Option<&str> {
        self.completion_id.as_deref()
    }

    /// Validate identity and output-contract continuity against the originating request.
    ///
    /// # Errors
    ///
    /// Returns [`EffectError::SettlementMismatch`] on any mismatch.
    pub fn validate_against(&self, requested: &EffectRequested) -> Result<(), EffectError> {
        validate_settlement(requested, self.effect_id, &self.output_contract)
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
            reason: Option<BoundedString<LABEL_MAX_BYTES>>,
            #[serde(default)]
            completion_id: Option<BoundedString<LABEL_MAX_BYTES>>,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(
            wire.effect_id,
            wire.output_contract,
            wire.reason.map(BoundedString::into_inner),
            wire.completion_id.map(BoundedString::into_inner),
        )
        .map_err(de::Error::custom)
    }
}
fn validate_settlement(
    requested: &EffectRequested,
    effect_id: EffectId,
    output_contract: &EffectOutputContract,
) -> Result<(), EffectError> {
    if effect_id != requested.effect_id() || output_contract != requested.output_contract() {
        return Err(EffectError::SettlementMismatch);
    }
    Ok(())
}
