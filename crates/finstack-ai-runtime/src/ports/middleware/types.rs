use std::sync::Arc;

use finstack_ai_kernel::{
    ArtifactRef, BudgetScopeId, ComponentId, ComponentInvocation, ComponentRef, Digest, EntryId,
    ErrorDescriptor, InteractionRequest, Message, Metadata, ModelRequestId, RawJson,
    RetryDirective, Sensitivity,
};
use serde::{Deserialize, Serialize};

use crate::context::ContextItem;
use crate::{ModelRequestDraft, RunCallContext};

use super::error::MiddlewareError;
use super::{
    MIDDLEWARE_COMMIT_REQUIRED, MIDDLEWARE_OUTCOME_NOT_ALLOWED, MIDDLEWARE_RESOLUTION_INVALID,
};

/// One of the seven normalized behavior-changing stages.
pub use finstack_ai_kernel::Stage;

/// Compact bit mask declaring a middleware component's stages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct StageMask(u8);

impl StageMask {
    /// Empty stage set.
    pub const EMPTY: Self = Self(0);
    /// All seven stable stages.
    pub const ALL: Self = Self(0b0111_1111);

    /// Create a mask from a stage sequence.
    #[must_use]
    pub fn from_stages(stages: impl IntoIterator<Item = Stage>) -> Self {
        let mut value = 0_u8;
        for stage in stages {
            value |= stage_bit(stage);
        }
        Self(value)
    }

    /// Whether the stage is declared.
    #[must_use]
    pub const fn contains(self, stage: Stage) -> bool {
        self.0 & stage_bit(stage) != 0
    }

    fn validate(self) -> Result<Self, MiddlewareError> {
        if self.0 == 0 || self.0 & !Self::ALL.0 != 0 {
            return Err(MiddlewareError::stable(
                MIDDLEWARE_RESOLUTION_INVALID,
                "middleware stage mask is empty or contains an unknown stage",
            ));
        }
        Ok(self)
    }
}

const fn stage_bit(stage: Stage) -> u8 {
    1 << match stage {
        Stage::BeforeRun => 0,
        Stage::PrepareContext => 1,
        Stage::BeforeModel => 2,
        Stage::AfterModel => 3,
        Stage::BeforeToolBatch => 4,
        Stage::AfterToolBatch => 5,
        Stage::BeforeFinalize => 6,
    }
}

/// Fixed ordering tier resolved before numeric priority and named constraints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderTier {
    /// Ordinary stage middleware outside the specialized `BeforeModel` tiers.
    Standard,
    /// Request and policy shaping.
    RequestShaping,
    /// Instruction/context mutation.
    ContextMutation,
    /// The unique context-compaction owner.
    ContextCompaction,
    /// Validation that cannot mutate model context.
    PostCompactionValidation,
}

/// Named deterministic middleware ordering declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MiddlewareOrder {
    /// Fixed tier.
    pub tier: OrderTier,
    /// Lower values execute first within one tier.
    pub priority: i32,
    /// Components that must execute after this component.
    pub before: Arc<[ComponentId]>,
    /// Components that must execute before this component.
    pub after: Arc<[ComponentId]>,
}

/// Descriptor role used for compaction uniqueness and post-compaction restrictions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MiddlewareRole {
    /// Ordinary middleware.
    Standard,
    /// Unique owner of model-context compaction policy.
    ContextCompactor {
        /// Stable strategy family.
        strategy_id: Arc<str>,
        /// Strategy contract version.
        strategy_version: u32,
    },
    /// Late validator that cannot mutate model context.
    PostCompactionValidator,
}

/// Immutable middleware descriptor locked at resolution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MiddlewareDescriptor {
    /// Exact component/version/configuration/recovery identity.
    pub invocation: ComponentInvocation,
    /// Stable stage set.
    pub stages: StageMask,
    /// Ordering declaration.
    pub order: MiddlewareOrder,
    /// Specialized role.
    pub role: MiddlewareRole,
    /// Non-secret descriptor metadata.
    #[serde(default)]
    pub metadata: Metadata,
}

impl MiddlewareDescriptor {
    pub(crate) fn validate(&self) -> Result<(), MiddlewareError> {
        self.stages.validate()?;
        if self.order.before.len() > finstack_ai_kernel::SEMANTIC_ARRAY_MAX_ITEMS
            || self.order.after.len() > finstack_ai_kernel::SEMANTIC_ARRAY_MAX_ITEMS
        {
            return Err(MiddlewareError::stable(
                MIDDLEWARE_RESOLUTION_INVALID,
                "middleware ordering declaration exceeds semantic bounds",
            ));
        }
        if self
            .order
            .before
            .iter()
            .chain(self.order.after.iter())
            .any(|component| component == &self.invocation.component)
        {
            return Err(MiddlewareError::stable(
                MIDDLEWARE_RESOLUTION_INVALID,
                "middleware cannot order itself",
            ));
        }
        match &self.role {
            MiddlewareRole::ContextCompactor {
                strategy_id,
                strategy_version,
            } => {
                validate_label(strategy_id, "compaction strategy id is invalid")?;
                if *strategy_version == 0
                    || !self.stages.contains(Stage::BeforeModel)
                    || self.order.tier != OrderTier::ContextCompaction
                {
                    return Err(MiddlewareError::stable(
                        MIDDLEWARE_RESOLUTION_INVALID,
                        "context compactor descriptor has an invalid stage, tier, or version",
                    ));
                }
            }
            MiddlewareRole::PostCompactionValidator => {
                if !self.stages.contains(Stage::BeforeModel)
                    || self.order.tier != OrderTier::PostCompactionValidation
                {
                    return Err(MiddlewareError::stable(
                        MIDDLEWARE_RESOLUTION_INVALID,
                        "post-compaction validator has an invalid stage or tier",
                    ));
                }
            }
            MiddlewareRole::Standard => {
                if matches!(
                    self.order.tier,
                    OrderTier::ContextCompaction | OrderTier::PostCompactionValidation
                ) {
                    return Err(MiddlewareError::stable(
                        MIDDLEWARE_RESOLUTION_INVALID,
                        "standard middleware cannot occupy a specialized compaction tier",
                    ));
                }
            }
        }
        Ok(())
    }
}

/// One canonical history entry supplied to compaction validation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompactionSourceEntry {
    /// Canonical conversation entry identity.
    pub entry_id: EntryId,
    /// Immutable canonical message.
    pub message: Message,
    /// Sensitivity inherited by summaries and checkpoints.
    pub sensitivity: Sensitivity,
    /// Provenance digest retained through derived projections.
    pub provenance_digest: Digest,
    /// Whether the message is non-compactable.
    pub protected: bool,
}

/// `BeforeModel` stage input required for compaction validation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BeforeModelInput {
    /// Fully assembled provider-neutral request.
    pub request: ModelRequestDraft,
    /// Canonical entry/message mapping in source order.
    pub source_entries: Arc<[CompactionSourceEntry]>,
    /// Locked model-context profile digest.
    pub model_context_profile_digest: Digest,
    /// Maximum permitted model-input tokens after output/overhead reservation.
    pub hard_input_tokens: u64,
    /// Optional latest compatible checkpoint, not yet trusted by the runtime.
    pub checkpoint: Option<CompactionCheckpoint>,
}

/// Immutable input for one middleware invocation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "stage", rename_all = "snake_case")]
pub enum StageInput {
    /// Before run.
    BeforeRun {
        /// Immutable normalized stage input.
        value: RawJson,
    },
    /// Prepare context.
    PrepareContext {
        /// Immutable normalized stage input.
        value: RawJson,
    },
    /// Before model.
    BeforeModel(Box<BeforeModelInput>),
    /// After model.
    AfterModel {
        /// Immutable normalized stage input.
        value: RawJson,
    },
    /// Before tool batch.
    BeforeToolBatch {
        /// Immutable normalized stage input.
        value: RawJson,
    },
    /// After tool batch.
    AfterToolBatch {
        /// Immutable normalized stage input.
        value: RawJson,
    },
    /// Before final terminal commit.
    BeforeFinalize {
        /// Candidate terminal value before any terminal record exists.
        candidate: RawJson,
    },
}

impl StageInput {
    /// Matching stable stage.
    #[must_use]
    pub const fn stage(&self) -> Stage {
        match self {
            Self::BeforeRun { .. } => Stage::BeforeRun,
            Self::PrepareContext { .. } => Stage::PrepareContext,
            Self::BeforeModel(_) => Stage::BeforeModel,
            Self::AfterModel { .. } => Stage::AfterModel,
            Self::BeforeToolBatch { .. } => Stage::BeforeToolBatch,
            Self::AfterToolBatch { .. } => Stage::AfterToolBatch,
            Self::BeforeFinalize { .. } => Stage::BeforeFinalize,
        }
    }

    /// Canonical raw JSON for one stage-input payload.
    ///
    /// # Errors
    ///
    /// Returns the historical `middleware_commit_required` code when
    /// canonicalization fails. That string does not imply a committed
    /// middleware effect.
    pub fn to_raw_json(&self) -> Result<RawJson, MiddlewareError> {
        RawJson::parse(canonical_bytes(self)?).map_err(|_| {
            MiddlewareError::stable(
                MIDDLEWARE_COMMIT_REQUIRED,
                "middleware input could not be normalized",
            )
        })
    }
}

/// Versioned prompt-cache effect of a compacted projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptCacheImpact {
    /// The stable prefix is byte-identical.
    StablePrefixPreserved,
    /// Only a mutable suffix changed.
    MutableSuffixChanged,
    /// The provider prompt cache is invalidated.
    CacheInvalidated,
}

/// Compaction integrity and attribution evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompactionEvidence {
    /// Strategy family.
    pub strategy_id: Arc<str>,
    /// Strategy contract version.
    pub strategy_version: u32,
    /// Locked strategy configuration.
    pub configuration_digest: Digest,
    /// Locked model-context profile.
    pub model_context_profile_digest: Digest,
    /// Exact source-entry digest.
    pub source_digest: Digest,
    /// Exact protected-entry-set digest.
    pub protected_item_set_digest: Digest,
    /// Source entries covered by the projection.
    pub covered_entry_ids: Arc<[EntryId]>,
    /// Source entries retained verbatim.
    pub retained_entry_ids: Arc<[EntryId]>,
    /// Exact replacement-message digest.
    pub projection_digest: Digest,
    /// Pre-compaction estimate.
    pub estimated_tokens_before: u64,
    /// Post-compaction estimate.
    pub estimated_tokens_after: u64,
    /// Optional derived-summary digest.
    pub summary_digest: Option<Digest>,
    /// Prompt-cache impact.
    pub cache_impact: PromptCacheImpact,
}

/// Inline or artifact-backed reusable summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactedSummary {
    /// Inline derived context.
    Inline(Arc<[ContextItem]>),
    /// Durable digest-verified artifact.
    Artifact(Box<ArtifactRef>),
}

/// Optional versioned compaction checkpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompactionCheckpoint {
    /// Owning middleware component.
    pub component_id: ComponentId,
    /// Strategy family.
    pub strategy_id: Arc<str>,
    /// Strategy contract version.
    pub strategy_version: u32,
    /// Locked strategy configuration.
    pub configuration_digest: Digest,
    /// Locked model-context profile.
    pub model_context_profile_digest: Digest,
    /// Last covered canonical entry.
    pub covered_through_entry_id: EntryId,
    /// Exact covered source digest.
    pub source_digest: Digest,
    /// Derived summary.
    pub summary: CompactedSummary,
    /// Exact summary digest.
    pub summary_digest: Digest,
    /// Sensitivity inherited from covered sources.
    pub sensitivity: Sensitivity,
}

/// Model-visible projection produced by the unique compactor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompactionResult {
    /// Replacement model-visible messages only.
    pub replacement_messages: Arc<[Message]>,
    /// Derived untrusted summaries.
    pub derived_summaries: Arc<[ContextItem]>,
    /// Integrity and attribution evidence.
    pub evidence: CompactionEvidence,
    /// Optional reusable derived checkpoint.
    pub checkpoint: Option<CompactionCheckpoint>,
}

/// Authorized child-model request used only for compaction summary generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompactionModelRequest {
    /// Resolved secondary model.
    pub model: ComponentRef,
    /// Frozen provider-neutral request.
    pub request: ModelRequestDraft,
    /// Explicit usage budget scope.
    pub budget_scope_id: BudgetScopeId,
    /// Full source sensitivity.
    pub source_sensitivity: Sensitivity,
    /// Authorized residency/egress policy digest.
    pub residency_policy_digest: Digest,
    /// Bounded opaque resume state for the same middleware identity.
    pub resume_state: RawJson,
}

/// Normalized behavior-changing middleware outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StageOutcome {
    /// Continue unchanged.
    Continue,
    /// Replace the stage value.
    Replace(RawJson),
    /// Add normalized instructions.
    AddInstructions(Arc<[ContextItem]>),
    /// Add normalized context.
    AddContext(Arc<[ContextItem]>),
    /// Replace only the model-visible context projection.
    CompactContext(Box<CompactionResult>),
    /// Request one related committed Model subeffect.
    RequestCompactionModel(Box<CompactionModelRequest>),
    /// Retain only the named tools.
    FilterTools(Arc<[finstack_ai_kernel::ToolId]>),
    /// Suspend for a typed interaction; approval is one standard profile.
    RequestInteraction(Box<InteractionRequest>),
    /// Request a bounded semantic retry.
    Retry(RetryDirective),
    /// Suspend with a stable reason.
    Suspend(Box<ErrorDescriptor>),
    /// Complete with normalized application output.
    Complete(RawJson),
    /// Fail with a safe descriptor.
    Fail(Box<ErrorDescriptor>),
}

impl StageOutcome {
    /// Convert to canonical stage-outcome JSON.
    ///
    /// # Errors
    ///
    /// Returns a stable error when canonicalization fails.
    pub fn to_raw_json(&self) -> Result<RawJson, MiddlewareError> {
        RawJson::parse(canonical_bytes(self)?).map_err(|_| {
            MiddlewareError::stable(
                MIDDLEWARE_OUTCOME_NOT_ALLOWED,
                "middleware outcome could not be normalized",
            )
        })
    }
}

/// Shared identity and optional child-model resume state for one invocation.
#[derive(Debug, Clone)]
pub struct MiddlewareContext {
    /// Shared committed port-call context.
    pub run: RunCallContext,
    /// Locked chain digest.
    pub chain_digest: Digest,
    /// Locked zero-based stage index.
    pub chain_index: u32,
    /// Optional normalized child-model result for the same parent effect.
    pub compaction_resume: Option<CompactionModelResume>,
}

/// Child-model result used to resume the same middleware invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactionModelResume {
    /// Child request identity.
    pub request_id: ModelRequestId,
    /// Child effect identity.
    pub effect_id: finstack_ai_kernel::EffectId,
    /// Normalized provider result.
    pub result: crate::ModelResponse,
    /// Original bounded resume state.
    pub resume_state: RawJson,
}

pub(super) fn canonical_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, MiddlewareError> {
    serde_json_canonicalizer::to_vec(value).map_err(|_| {
        MiddlewareError::stable(
            MIDDLEWARE_OUTCOME_NOT_ALLOWED,
            "middleware value could not be canonicalized",
        )
    })
}

/// Stable wire-format name for a stage, used to build the `PipelinePosition`
/// stage string.
#[must_use]
pub fn stage_name(stage: Stage) -> &'static str {
    match stage {
        Stage::BeforeRun => "before_run",
        Stage::PrepareContext => "prepare_context",
        Stage::BeforeModel => "before_model",
        Stage::AfterModel => "after_model",
        Stage::BeforeToolBatch => "before_tool_batch",
        Stage::AfterToolBatch => "after_tool_batch",
        Stage::BeforeFinalize => "before_finalize",
    }
}

#[cfg(test)]
pub(crate) fn parse_stage(value: &str) -> Option<Stage> {
    Some(match value {
        "before_run" => Stage::BeforeRun,
        "prepare_context" => Stage::PrepareContext,
        "before_model" => Stage::BeforeModel,
        "after_model" => Stage::AfterModel,
        "before_tool_batch" => Stage::BeforeToolBatch,
        "after_tool_batch" => Stage::AfterToolBatch,
        "before_finalize" => Stage::BeforeFinalize,
        _ => return None,
    })
}

fn validate_label(value: &str, message: &'static str) -> Result<(), MiddlewareError> {
    if value.is_empty() || value.len() > 256 || value.as_bytes().contains(&0) {
        return Err(MiddlewareError::stable(
            MIDDLEWARE_RESOLUTION_INVALID,
            message,
        ));
    }
    Ok(())
}
