//! Middleware port, deterministic ordering, durable invocation guards, and compaction validation.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use finstack_ai_kernel::{
    ArtifactRef, BudgetScopeId, ComponentId, ComponentInvocation, ComponentRef, Digest,
    EffectCompleted, EffectInput, EffectKind, EffectOutputKind, EffectPurpose, EffectRequested,
    EntryId, ErrorCategory, ErrorDescriptor, InteractionRequest, InvocationRecovery, Message,
    Metadata, ModelRequestId, PipelinePosition, RawJson, RecordBody, RecordEnvelope,
    RetryDirective, RetrySafety, Sensitivity, ToolCallId,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::context::{ContextItem, InvocationResumeAction};
use crate::error::{PortErrorData, PortErrorInvalid};
use crate::{ModelRequestDraft, PortFuture, PortObject, ReconcileContext, RunCallContext};

/// Stable middleware resolution error.
pub const MIDDLEWARE_RESOLUTION_INVALID: &str = "middleware_resolution_invalid";
/// Stable middleware-cycle error.
pub const MIDDLEWARE_ORDER_CYCLE: &str = "middleware_order_cycle";
/// Stable missing committed-invocation error.
pub const MIDDLEWARE_COMMIT_REQUIRED: &str = "middleware_commit_required";
/// Stable stage/outcome matrix error.
pub const MIDDLEWARE_OUTCOME_NOT_ALLOWED: &str = "middleware_outcome_not_allowed";
/// Stable compaction-integrity error.
pub const COMPACTION_RESULT_INVALID: &str = "compaction_result_invalid";
/// Stable context hard-budget error.
pub const COMPACTION_BUDGET_EXCEEDED: &str = "context_budget_exceeded";
/// Stable unauthorized compaction-child-model error.
pub const COMPACTION_MODEL_NOT_AUTHORIZED: &str = "compaction_model_not_authorized";

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
    fn validate(&self) -> Result<(), MiddlewareError> {
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

    /// Canonical raw JSON committed in `EffectInput::Middleware`.
    ///
    /// # Errors
    ///
    /// Returns a stable error when canonicalization fails.
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
    /// Convert to canonical committed middleware output.
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

/// Outstanding middleware effect supplied to `reconcile`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingMiddlewareEffect {
    /// Frozen input.
    pub input: StageInput,
    /// Committed component invocation.
    pub invocation: ComponentInvocation,
    /// Committed pipeline position.
    pub pipeline: PipelinePosition,
}

/// Middleware reconciliation result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MiddlewareReconcileResult {
    /// Normalized output is available.
    Completed(Box<StageOutcome>),
    /// The original invocation provably did not start.
    NotStarted,
    /// Reusing the same effect identity is safe.
    RetrySafe,
    /// The component cannot classify the effect.
    Unknown,
    /// A non-repeatable external action may have occurred.
    NonRepeatable,
}

/// Object-safe single-invocation middleware port.
///
/// Stages are coarse boundaries (`before_model`, `before_finalize`, …).
/// There is no per-token hook. At most one late-tier `before_model`
/// compaction owner may be active per resolved agent.
pub trait Middleware: PortObject {
    /// Immutable descriptor.
    fn descriptor(&self) -> MiddlewareDescriptor;

    /// Declared stage mask.
    fn stages(&self) -> StageMask {
        self.descriptor().stages
    }

    /// Invoke one committed stage boundary.
    ///
    /// # Arguments
    ///
    /// * `ctx` - Stage identity, locator, and cancellation.
    /// * `input` - Immutable stage payload. Return a replacement or continue.
    fn invoke(
        &self,
        ctx: MiddlewareContext,
        input: StageInput,
    ) -> PortFuture<Result<StageOutcome, MiddlewareError>>;

    /// Reconcile an outstanding committed invocation.
    fn reconcile(
        &self,
        _ctx: ReconcileContext,
        _effect: PendingMiddlewareEffect,
    ) -> PortFuture<Result<MiddlewareReconcileResult, MiddlewareError>> {
        Box::pin(async { Ok(MiddlewareReconcileResult::Unknown) })
    }
}

/// One middleware component supplied to the resolver.
pub struct MiddlewareRegistration {
    /// Ready direct handle.
    pub middleware: Arc<dyn Middleware>,
}

/// Resolved direct middleware handle and immutable descriptor.
#[derive(Clone)]
pub struct ResolvedMiddleware {
    /// Ready direct handle.
    pub middleware: Arc<dyn Middleware>,
    /// Frozen descriptor.
    pub descriptor: MiddlewareDescriptor,
    /// Registration-order tiebreaker.
    pub registration_index: u32,
}

/// Deterministically resolved middleware chain.
pub struct ResolvedMiddlewareChain {
    chain_digest: Digest,
    stages: BTreeMap<Stage, Arc<[ResolvedMiddleware]>>,
}

impl ResolvedMiddlewareChain {
    /// Resolve descriptors once and reject duplicates, missing requirements, cycles, and invalid
    /// compaction ownership before any run starts.
    ///
    /// # Errors
    ///
    /// Returns a stable construction diagnostic on any invalid graph.
    pub fn try_new(registrations: Vec<MiddlewareRegistration>) -> Result<Self, MiddlewareError> {
        if registrations.len() > finstack_ai_kernel::SEMANTIC_ARRAY_MAX_ITEMS {
            return Err(MiddlewareError::stable(
                MIDDLEWARE_RESOLUTION_INVALID,
                "middleware chain exceeds the semantic component bound",
            ));
        }
        let mut resolved = Vec::with_capacity(registrations.len());
        let mut ids = BTreeSet::new();
        let mut compactors = 0_usize;
        for (index, registration) in registrations.into_iter().enumerate() {
            let descriptor = registration.middleware.descriptor();
            descriptor.validate()?;
            if !ids.insert(descriptor.invocation.component.clone()) {
                return Err(MiddlewareError::stable(
                    MIDDLEWARE_RESOLUTION_INVALID,
                    "middleware component is registered more than once",
                ));
            }
            if matches!(descriptor.role, MiddlewareRole::ContextCompactor { .. }) {
                compactors += 1;
            }
            resolved.push(ResolvedMiddleware {
                middleware: registration.middleware,
                descriptor,
                registration_index: u32::try_from(index).map_err(|_| {
                    MiddlewareError::stable(
                        MIDDLEWARE_RESOLUTION_INVALID,
                        "middleware registration index overflowed",
                    )
                })?,
            });
        }
        if compactors > 1 {
            return Err(MiddlewareError::stable(
                MIDDLEWARE_RESOLUTION_INVALID,
                "more than one context-compaction owner is active",
            ));
        }
        for item in &resolved {
            for dependency in item
                .descriptor
                .order
                .before
                .iter()
                .chain(item.descriptor.order.after.iter())
            {
                if !ids.contains(dependency) {
                    return Err(MiddlewareError::stable(
                        MIDDLEWARE_RESOLUTION_INVALID,
                        "middleware ordering references a missing component",
                    ));
                }
            }
        }
        let descriptor_bytes = canonical_bytes(
            &resolved
                .iter()
                .map(|item| &item.descriptor)
                .collect::<Vec<_>>(),
        )?;
        let chain_digest = Digest::domain_separated("middleware-chain", 1, &descriptor_bytes)
            .map_err(|_| {
                MiddlewareError::stable(
                    MIDDLEWARE_RESOLUTION_INVALID,
                    "middleware chain digest could not be constructed",
                )
            })?;
        let mut stages = BTreeMap::new();
        for stage in [
            Stage::BeforeRun,
            Stage::PrepareContext,
            Stage::BeforeModel,
            Stage::AfterModel,
            Stage::BeforeToolBatch,
            Stage::AfterToolBatch,
            Stage::BeforeFinalize,
        ] {
            let ordered = resolve_stage(stage, &resolved)?;
            stages.insert(stage, ordered.into());
        }
        Ok(Self {
            chain_digest,
            stages,
        })
    }

    /// Frozen chain digest.
    #[must_use]
    pub const fn digest(&self) -> Digest {
        self.chain_digest
    }

    /// Resolved direct handles for one stage.
    #[must_use]
    pub fn stage(&self, stage: Stage) -> &[ResolvedMiddleware] {
        self.stages.get(&stage).map_or(&[], AsRef::as_ref)
    }
}

/// Guard proving a middleware invocation is backed by an exact committed effect record.
#[derive(Debug, Clone)]
pub struct CommittedMiddlewareCall {
    context: MiddlewareContext,
    input: StageInput,
    requested: EffectRequested,
}

impl CommittedMiddlewareCall {
    /// Validate the committed envelope against the exact component, locator, input, and cursor.
    ///
    /// # Errors
    ///
    /// Returns `middleware_commit_required` for any mismatch.
    pub fn try_new(
        envelope: &RecordEnvelope,
        context: MiddlewareContext,
        input: StageInput,
        descriptor: &MiddlewareDescriptor,
    ) -> Result<Self, MiddlewareError> {
        let RecordBody::EffectRequested(requested) = envelope.body() else {
            return Err(MiddlewareError::commit_required());
        };
        let pipeline = requested
            .pipeline()
            .ok_or_else(MiddlewareError::commit_required)?;
        let raw = input.to_raw_json()?;
        let stage = input.stage();
        let locator = &context.run.locator;
        if envelope.sequence() == 0
            || envelope.session_id() != locator.session_id
            || envelope.lane_id() != locator.lane_id
            || envelope.run_id() != Some(locator.run_id)
            || requested.effect_id() != context.run.effect_id
            || requested.kind() != EffectKind::Middleware
            || requested.component() != Some(&descriptor.invocation)
            || requested.output_contract().kind != EffectOutputKind::MiddlewareOutcome
            || requested.deadline() != context.run.deadline
            || requested.retry_safety() == RetrySafety::Unknown
            || pipeline.chain_digest() != context.chain_digest
            || pipeline.index() != context.chain_index
            || pipeline.stage() != stage_name(stage)
            || !matches!(requested.input(), EffectInput::Middleware { stage: committed_stage, input } if committed_stage.as_ref() == stage_name(stage) && input == &raw)
        {
            return Err(MiddlewareError::commit_required());
        }
        Ok(Self {
            context,
            input,
            requested: requested.clone(),
        })
    }

    /// Invoke the component and validate its normalized outcome for this exact stage/role.
    ///
    /// # Errors
    ///
    /// Returns a stable descriptor, port, stage-matrix, or compaction-integrity error.
    pub async fn invoke(
        self,
        middleware: &dyn Middleware,
    ) -> Result<StageOutcome, MiddlewareError> {
        let descriptor = middleware.descriptor();
        if self.requested.component() != Some(&descriptor.invocation) {
            return Err(MiddlewareError::commit_required());
        }
        let outcome = middleware.invoke(self.context, self.input.clone()).await?;
        validate_stage_outcome(&descriptor, &self.input, &outcome)?;
        Ok(outcome)
    }
}

/// A middleware output proven to have been committed after its exact request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedMiddlewareOutcome {
    /// Middleware component.
    pub component: ComponentId,
    /// Locked chain index.
    pub chain_index: u32,
    /// Frozen stage.
    pub stage: Stage,
    /// Normalized output.
    pub outcome: StageOutcome,
}

impl RecordedMiddlewareOutcome {
    /// Reconstruct one replay-safe outcome from request/completion envelopes.
    ///
    /// # Errors
    ///
    /// Returns a stable error for identity, sequence, contract, or payload mismatch.
    pub fn try_from_records(
        requested_envelope: &RecordEnvelope,
        completed_envelope: &RecordEnvelope,
    ) -> Result<Self, MiddlewareError> {
        let RecordBody::EffectRequested(requested) = requested_envelope.body() else {
            return Err(MiddlewareError::commit_required());
        };
        let RecordBody::EffectCompleted(completed) = completed_envelope.body() else {
            return Err(MiddlewareError::stable(
                MIDDLEWARE_OUTCOME_NOT_ALLOWED,
                "middleware completion record is missing",
            ));
        };
        let invocation = requested
            .component()
            .ok_or_else(MiddlewareError::commit_required)?;
        let pipeline = requested
            .pipeline()
            .ok_or_else(MiddlewareError::commit_required)?;
        let stage = parse_stage(pipeline.stage()).ok_or_else(|| {
            MiddlewareError::stable(
                MIDDLEWARE_OUTCOME_NOT_ALLOWED,
                "committed middleware stage is invalid",
            )
        })?;
        if requested.kind() != EffectKind::Middleware
            || requested.output_contract().kind != EffectOutputKind::MiddlewareOutcome
            || completed_envelope.sequence() <= requested_envelope.sequence()
            || completed_envelope.session_id() != requested_envelope.session_id()
            || completed_envelope.lane_id() != requested_envelope.lane_id()
            || completed_envelope.run_id() != requested_envelope.run_id()
            || completed.validate_against(requested).is_err()
        {
            return Err(MiddlewareError::stable(
                MIDDLEWARE_OUTCOME_NOT_ALLOWED,
                "middleware completion does not match its committed request",
            ));
        }
        let outcome = serde_json::from_slice(completed.output().as_bytes()).map_err(|_| {
            MiddlewareError::stable(
                MIDDLEWARE_OUTCOME_NOT_ALLOWED,
                "committed middleware outcome is invalid",
            )
        })?;
        Ok(Self {
            component: invocation.component.clone(),
            chain_index: pipeline.index(),
            stage,
            outcome,
        })
    }
}

/// Determine recovery behavior from committed request/output state.
#[must_use]
pub fn middleware_resume_action(
    requested: &EffectRequested,
    completed: Option<&EffectCompleted>,
) -> InvocationResumeAction {
    if completed.is_some_and(|value| value.validate_against(requested).is_ok()) {
        return InvocationResumeAction::UseRecorded;
    }
    match requested.component().map(|value| value.recovery) {
        Some(InvocationRecovery::RecomputeSafe) => InvocationResumeAction::Recompute,
        Some(InvocationRecovery::Reconcile) => InvocationResumeAction::Reconcile,
        Some(InvocationRecovery::NonRepeatable) | None => InvocationResumeAction::SuspendUncertain,
    }
}

/// Validate a related committed Model effect before model-assisted compaction dispatch.
///
/// # Errors
///
/// Returns `compaction_model_not_authorized` unless the child effect is related to the exact
/// committed middleware parent and freezes the requested model draft.
pub fn validate_compaction_model_effect(
    parent: &EffectRequested,
    child_envelope: &RecordEnvelope,
    request: &CompactionModelRequest,
) -> Result<(), MiddlewareError> {
    let RecordBody::EffectRequested(child) = child_envelope.body() else {
        return Err(MiddlewareError::compaction_model_not_authorized());
    };
    let raw = RawJson::parse(request.request.canonical_bytes().map_err(|_| {
        MiddlewareError::stable(
            COMPACTION_MODEL_NOT_AUTHORIZED,
            "compaction model request could not be normalized",
        )
    })?)
    .map_err(|_| {
        MiddlewareError::stable(
            COMPACTION_MODEL_NOT_AUTHORIZED,
            "compaction model request could not be normalized",
        )
    })?;
    let relation_matches = child.relation().is_some_and(|relation| {
        relation.parent_effect_id == parent.effect_id()
            && matches!(
                &relation.purpose,
                EffectPurpose::CompactionSummary {
                    middleware_component_id
                } if parent.component().is_some_and(|component| &component.component == middleware_component_id)
            )
    });
    if parent.kind() != EffectKind::Middleware
        || child.kind() != EffectKind::Model
        || child_envelope.sequence() == 0
        || !relation_matches
        || !child.component().is_some_and(|value| {
            &value.component == request.model.id()
                && request
                    .model
                    .version()
                    .is_some_and(|version| value.version == version)
        })
        || !matches!(child.input(), EffectInput::Model { request } if request == &raw)
        || child.output_contract().kind != EffectOutputKind::ModelResponse
    {
        return Err(MiddlewareError::compaction_model_not_authorized());
    }
    Ok(())
}

/// Validate the fixed stage/outcome matrix and descriptor-specific restrictions.
///
/// # Errors
///
/// Returns `middleware_outcome_not_allowed` or a precise compaction error.
pub fn validate_stage_outcome(
    descriptor: &MiddlewareDescriptor,
    input: &StageInput,
    outcome: &StageOutcome,
) -> Result<(), MiddlewareError> {
    let stage = input.stage();
    if !descriptor.stages.contains(stage) {
        return Err(MiddlewareError::outcome_not_allowed());
    }
    let allowed = match outcome {
        StageOutcome::Continue
        | StageOutcome::Fail(_)
        | StageOutcome::Suspend(_)
        | StageOutcome::RequestInteraction(_) => true,
        StageOutcome::Replace(_) => stage != Stage::BeforeFinalize,
        StageOutcome::AddInstructions(_) | StageOutcome::AddContext(_) => {
            matches!(stage, Stage::PrepareContext | Stage::BeforeModel)
        }
        StageOutcome::FilterTools(_) => {
            matches!(stage, Stage::BeforeModel | Stage::BeforeToolBatch)
        }
        StageOutcome::CompactContext(_) | StageOutcome::RequestCompactionModel(_) => {
            stage == Stage::BeforeModel
                && matches!(descriptor.role, MiddlewareRole::ContextCompactor { .. })
        }
        StageOutcome::Retry(_) => {
            matches!(
                stage,
                Stage::AfterModel | Stage::AfterToolBatch | Stage::BeforeFinalize
            )
        }
        StageOutcome::Complete(_) => {
            matches!(
                stage,
                Stage::BeforeRun
                    | Stage::AfterModel
                    | Stage::AfterToolBatch
                    | Stage::BeforeFinalize
            )
        }
    };
    if !allowed {
        return Err(MiddlewareError::outcome_not_allowed());
    }
    if matches!(descriptor.role, MiddlewareRole::PostCompactionValidator)
        && !matches!(
            outcome,
            StageOutcome::Continue
                | StageOutcome::Fail(_)
                | StageOutcome::Suspend(_)
                | StageOutcome::RequestInteraction(_)
        )
    {
        return Err(MiddlewareError::outcome_not_allowed());
    }
    match outcome {
        StageOutcome::CompactContext(result) => {
            let StageInput::BeforeModel(before_model) = input else {
                return Err(MiddlewareError::outcome_not_allowed());
            };
            validate_compaction_result(descriptor, before_model, result)
        }
        StageOutcome::RequestCompactionModel(request) => validate_compaction_model_request(request),
        _ => Ok(()),
    }
}

/// Validate protected content, tool-pair atomicity, attribution, and hard budget.
///
/// # Errors
///
/// Returns a stable compaction or context-budget error without mutating canonical history.
pub fn validate_compaction_result(
    descriptor: &MiddlewareDescriptor,
    input: &BeforeModelInput,
    result: &CompactionResult,
) -> Result<(), MiddlewareError> {
    let MiddlewareRole::ContextCompactor {
        strategy_id,
        strategy_version,
    } = &descriptor.role
    else {
        return Err(MiddlewareError::outcome_not_allowed());
    };
    validate_compaction_evidence(descriptor, input, result, strategy_id, *strategy_version)?;

    if !input.source_entries.last().is_some_and(|entry| {
        entry.protected && entry.message.role() == finstack_ai_kernel::MessageRole::User
    }) {
        return Err(MiddlewareError::compaction_invalid());
    }
    let source_by_message = input
        .source_entries
        .iter()
        .enumerate()
        .map(|(index, entry)| (*entry.message.id(), (index, entry)))
        .collect::<BTreeMap<_, _>>();
    let mut replacements = BTreeMap::new();
    let mut actual_retained = Vec::new();
    let mut last_source_index = None;
    for message in result.replacement_messages.iter() {
        let Some((source_index, source)) = source_by_message.get(message.id()) else {
            return Err(MiddlewareError::compaction_invalid());
        };
        if last_source_index.is_some_and(|last| last >= *source_index) {
            return Err(MiddlewareError::compaction_invalid());
        }
        if replacements.insert(*message.id(), message).is_some() {
            return Err(MiddlewareError::compaction_invalid());
        }
        last_source_index = Some(*source_index);
        actual_retained.push(source.entry_id);
    }
    if actual_retained.as_slice() != result.evidence.retained_entry_ids.as_ref() {
        return Err(MiddlewareError::compaction_invalid());
    }
    for entry in input.source_entries.iter().filter(|entry| entry.protected) {
        let Some(replacement) = replacements.get(entry.message.id()) else {
            return Err(MiddlewareError::compaction_invalid());
        };
        if canonical_bytes(*replacement)? != canonical_bytes(&entry.message)? {
            return Err(MiddlewareError::compaction_invalid());
        }
    }
    let source_pairs = tool_pairs(input.source_entries.iter().map(|entry| &entry.message))?;
    let replacement_pairs = tool_pairs(result.replacement_messages.iter())?;
    for (call, has_result) in source_pairs {
        let retained = replacement_pairs.get(&call).copied();
        if has_result && retained.is_some_and(|result_present| !result_present) {
            return Err(MiddlewareError::compaction_invalid());
        }
    }
    let source_sensitivity = input
        .source_entries
        .iter()
        .map(|entry| sensitivity_rank(entry.sensitivity))
        .max()
        .unwrap_or(0);
    for summary in result.derived_summaries.iter() {
        if summary.kind != crate::ContextItemKind::DerivedSummary
            || summary.authority != crate::ContextAuthority::Untrusted
            || sensitivity_rank(summary.sensitivity) < source_sensitivity
        {
            return Err(MiddlewareError::compaction_invalid());
        }
    }
    validate_checkpoint(descriptor, input, result)?;
    Ok(())
}

fn validate_compaction_evidence(
    descriptor: &MiddlewareDescriptor,
    input: &BeforeModelInput,
    result: &CompactionResult,
    strategy_id: &str,
    strategy_version: u32,
) -> Result<(), MiddlewareError> {
    if input.source_entries.len() > finstack_ai_kernel::SEMANTIC_ARRAY_MAX_ITEMS
        || result.replacement_messages.len() > finstack_ai_kernel::SEMANTIC_ARRAY_MAX_ITEMS
        || result.derived_summaries.len() > finstack_ai_kernel::SEMANTIC_ARRAY_MAX_ITEMS
    {
        return Err(MiddlewareError::compaction_invalid());
    }
    let source_digest = compaction_source_digest(&input.source_entries)?;
    let protected_ids = input
        .source_entries
        .iter()
        .filter(|entry| entry.protected)
        .map(|entry| entry.entry_id)
        .collect::<Vec<_>>();
    let protected_digest = compaction_protected_set_digest(&protected_ids)?;
    let projection_digest = compaction_projection_digest(&result.replacement_messages)?;
    let covered = input
        .source_entries
        .iter()
        .map(|entry| entry.entry_id)
        .collect::<Vec<_>>();
    if result.evidence.strategy_id.as_ref() != strategy_id
        || result.evidence.strategy_version != strategy_version
        || result.evidence.configuration_digest != descriptor.invocation.configuration_digest
        || result.evidence.model_context_profile_digest != input.model_context_profile_digest
        || result.evidence.source_digest != source_digest
        || result.evidence.protected_item_set_digest != protected_digest
        || result.evidence.projection_digest != projection_digest
        || result.evidence.covered_entry_ids.as_ref() != covered
        || result.evidence.estimated_tokens_after > input.hard_input_tokens
        || result.evidence.estimated_tokens_after > result.evidence.estimated_tokens_before
    {
        return Err(
            if result.evidence.estimated_tokens_after > input.hard_input_tokens {
                MiddlewareError::stable(
                    COMPACTION_BUDGET_EXCEEDED,
                    "compacted projection exceeds the hard model-input budget",
                )
            } else {
                MiddlewareError::compaction_invalid()
            },
        );
    }

    Ok(())
}

/// Stable middleware error.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{data}")]
pub struct MiddlewareError {
    data: PortErrorData,
}

impl From<PortErrorInvalid> for MiddlewareError {
    fn from(error: PortErrorInvalid) -> Self {
        match error {
            PortErrorInvalid::InvalidCode => Self::stable(
                MIDDLEWARE_OUTCOME_NOT_ALLOWED,
                "middleware error code is invalid",
            ),
            PortErrorInvalid::InvalidMessage => Self::stable(
                MIDDLEWARE_OUTCOME_NOT_ALLOWED,
                "middleware error message is invalid",
            ),
            PortErrorInvalid::InvalidClassification => Self::stable(
                MIDDLEWARE_OUTCOME_NOT_ALLOWED,
                "middleware error classification is invalid",
            ),
        }
    }
}

impl MiddlewareError {
    /// Construct a bounded middleware-specific error.
    ///
    /// # Errors
    ///
    /// Returns [`PortErrorInvalid`] when the code or message is invalid.
    pub fn try_new(
        code: impl AsRef<str>,
        category: ErrorCategory,
        message: impl AsRef<str>,
        metadata: Metadata,
    ) -> Result<Self, PortErrorInvalid> {
        PortErrorData::try_from_parts(code, category, false, message, metadata, 1_048_576)
            .map(|data| Self { data })
    }

    pub(crate) fn stable(code: &'static str, message: &'static str) -> Self {
        Self {
            data: PortErrorData::frozen(
                code,
                if code == COMPACTION_BUDGET_EXCEEDED {
                    ErrorCategory::Limit
                } else {
                    ErrorCategory::Middleware
                },
                false,
                message,
            ),
        }
    }

    fn commit_required() -> Self {
        Self::stable(
            MIDDLEWARE_COMMIT_REQUIRED,
            "middleware invocation is not backed by the exact committed effect",
        )
    }

    fn outcome_not_allowed() -> Self {
        Self::stable(
            MIDDLEWARE_OUTCOME_NOT_ALLOWED,
            "middleware outcome is not allowed at this stage or descriptor tier",
        )
    }

    fn compaction_invalid() -> Self {
        Self::stable(
            COMPACTION_RESULT_INVALID,
            "compaction projection or evidence violates the frozen integrity contract",
        )
    }

    fn compaction_model_not_authorized() -> Self {
        Self::stable(
            COMPACTION_MODEL_NOT_AUTHORIZED,
            "compaction child model is not backed by an authorized related effect",
        )
    }

    /// Stable error code.
    #[must_use]
    pub fn code(&self) -> &str {
        self.data.code.as_str()
    }

    /// Safe error descriptor suitable for durable failure records.
    #[must_use]
    pub fn descriptor(&self) -> ErrorDescriptor {
        ErrorDescriptor {
            code: self.data.code.clone(),
            message: Arc::clone(&self.data.message),
            category: self.data.category,
            retryable: false,
            identifiers: finstack_ai_kernel::ErrorIdentifiers::default(),
            safe_details: self.data.metadata.clone(),
        }
    }
}

fn resolve_stage(
    stage: Stage,
    all: &[ResolvedMiddleware],
) -> Result<Vec<ResolvedMiddleware>, MiddlewareError> {
    let nodes = all
        .iter()
        .filter(|item| item.descriptor.stages.contains(stage))
        .cloned()
        .collect::<Vec<_>>();
    let node_ids = nodes
        .iter()
        .map(|item| item.descriptor.invocation.component.clone())
        .collect::<BTreeSet<_>>();
    let mut incoming = node_ids
        .iter()
        .cloned()
        .map(|id| (id, 0_usize))
        .collect::<BTreeMap<_, _>>();
    let mut outgoing = BTreeMap::<ComponentId, BTreeSet<ComponentId>>::new();
    let mut add_edge = |from: &ComponentId, to: &ComponentId| {
        if from != to
            && node_ids.contains(from)
            && node_ids.contains(to)
            && outgoing.entry(from.clone()).or_default().insert(to.clone())
        {
            *incoming.entry(to.clone()).or_default() += 1;
        }
    };
    for left in &nodes {
        for right in &nodes {
            if left.descriptor.order.tier < right.descriptor.order.tier {
                add_edge(
                    &left.descriptor.invocation.component,
                    &right.descriptor.invocation.component,
                );
            }
        }
        for after in left.descriptor.order.before.iter() {
            add_edge(&left.descriptor.invocation.component, after);
        }
        for before in left.descriptor.order.after.iter() {
            add_edge(before, &left.descriptor.invocation.component);
        }
    }
    let by_id = nodes
        .iter()
        .cloned()
        .map(|item| (item.descriptor.invocation.component.clone(), item))
        .collect::<BTreeMap<_, _>>();
    let mut result = Vec::with_capacity(nodes.len());
    let mut remaining = node_ids;
    while !remaining.is_empty() {
        let next = remaining
            .iter()
            .filter(|id| incoming.get(*id).copied().unwrap_or_default() == 0)
            .min_by_key(|id| {
                let item = by_id.get(*id).expect("resolved node exists");
                (
                    item.descriptor.order.tier,
                    item.descriptor.order.priority,
                    item.registration_index,
                    (*id).clone(),
                )
            })
            .cloned()
            .ok_or_else(|| {
                MiddlewareError::stable(
                    MIDDLEWARE_ORDER_CYCLE,
                    "middleware ordering contains a cycle",
                )
            })?;
        remaining.remove(&next);
        result.push(by_id.get(&next).expect("resolved node exists").clone());
        if let Some(targets) = outgoing.get(&next) {
            for target in targets {
                if let Some(value) = incoming.get_mut(target) {
                    *value = value.saturating_sub(1);
                }
            }
        }
    }
    Ok(result)
}

fn validate_compaction_model_request(
    request: &CompactionModelRequest,
) -> Result<(), MiddlewareError> {
    if request.resume_state.as_bytes().len() > 1_048_576 {
        return Err(MiddlewareError::stable(
            COMPACTION_MODEL_NOT_AUTHORIZED,
            "compaction resume state exceeds its hard bound",
        ));
    }
    request.request.canonical_bytes().map_err(|_| {
        MiddlewareError::stable(
            COMPACTION_MODEL_NOT_AUTHORIZED,
            "compaction model request is invalid",
        )
    })?;
    Ok(())
}

fn validate_checkpoint(
    descriptor: &MiddlewareDescriptor,
    input: &BeforeModelInput,
    result: &CompactionResult,
) -> Result<(), MiddlewareError> {
    let Some(checkpoint) = &result.checkpoint else {
        return Ok(());
    };
    let Some(last_covered) = result.evidence.covered_entry_ids.last() else {
        return Err(MiddlewareError::compaction_invalid());
    };
    if checkpoint.component_id != descriptor.invocation.component
        || checkpoint.strategy_id != result.evidence.strategy_id
        || checkpoint.strategy_version != result.evidence.strategy_version
        || checkpoint.configuration_digest != descriptor.invocation.configuration_digest
        || checkpoint.model_context_profile_digest != input.model_context_profile_digest
        || checkpoint.covered_through_entry_id != *last_covered
        || checkpoint.source_digest != result.evidence.source_digest
    {
        return Err(MiddlewareError::compaction_invalid());
    }
    let summary_digest = compaction_summary_digest(&checkpoint.summary)?;
    let source_sensitivity = input
        .source_entries
        .iter()
        .map(|entry| sensitivity_rank(entry.sensitivity))
        .max()
        .unwrap_or(0);
    if checkpoint.summary_digest != summary_digest
        || result.evidence.summary_digest != Some(summary_digest)
        || sensitivity_rank(checkpoint.sensitivity) < source_sensitivity
    {
        return Err(MiddlewareError::compaction_invalid());
    }
    Ok(())
}

/// Digest canonical compaction source entries under the frozen v1 domain.
///
/// # Errors
///
/// Returns a stable compaction error when canonicalization fails.
pub fn compaction_source_digest(
    entries: &Arc<[CompactionSourceEntry]>,
) -> Result<Digest, MiddlewareError> {
    digest_value("compaction-source", entries)
}

/// Digest the ordered protected-entry set under the frozen v1 domain.
///
/// # Errors
///
/// Returns a stable compaction error when canonicalization fails.
pub fn compaction_protected_set_digest(entries: &[EntryId]) -> Result<Digest, MiddlewareError> {
    digest_value("compaction-protected-set", &entries)
}

/// Digest replacement messages under the frozen v1 projection domain.
///
/// # Errors
///
/// Returns a stable compaction error when canonicalization fails.
pub fn compaction_projection_digest(messages: &Arc<[Message]>) -> Result<Digest, MiddlewareError> {
    digest_value("compaction-projection", messages)
}

/// Digest a reusable summary under the frozen v1 summary domain.
///
/// # Errors
///
/// Returns a stable compaction error when canonicalization fails.
pub fn compaction_summary_digest(summary: &CompactedSummary) -> Result<Digest, MiddlewareError> {
    digest_value("compaction-summary", summary)
}

/// Check whether a reusable checkpoint exactly matches the current compactor and history prefix.
///
/// A mismatch is an ordinary cache miss: callers must ignore it and rebuild from canonical
/// history. Malformed canonical data returns an integrity error instead of being trusted.
///
/// # Errors
///
/// Returns a stable compaction error when source or summary digests cannot be computed.
pub fn compaction_checkpoint_compatible(
    descriptor: &MiddlewareDescriptor,
    input: &BeforeModelInput,
    checkpoint: &CompactionCheckpoint,
) -> Result<bool, MiddlewareError> {
    let MiddlewareRole::ContextCompactor {
        strategy_id,
        strategy_version,
    } = &descriptor.role
    else {
        return Ok(false);
    };
    let Some(covered_index) = input
        .source_entries
        .iter()
        .position(|entry| entry.entry_id == checkpoint.covered_through_entry_id)
    else {
        return Ok(false);
    };
    let covered: Arc<[CompactionSourceEntry]> =
        input.source_entries[..=covered_index].to_vec().into();
    let source_digest = compaction_source_digest(&covered)?;
    let summary_digest = compaction_summary_digest(&checkpoint.summary)?;
    let source_sensitivity = covered
        .iter()
        .map(|entry| sensitivity_rank(entry.sensitivity))
        .max()
        .unwrap_or(0);
    let summary_is_safe = match &checkpoint.summary {
        CompactedSummary::Inline(items) => items.iter().all(|item| {
            item.kind == crate::ContextItemKind::DerivedSummary
                && item.authority == crate::ContextAuthority::Untrusted
                && sensitivity_rank(item.sensitivity) >= source_sensitivity
        }),
        CompactedSummary::Artifact(_) => true,
    };
    Ok(checkpoint.component_id == descriptor.invocation.component
        && checkpoint.strategy_id.as_ref() == strategy_id.as_ref()
        && checkpoint.strategy_version == *strategy_version
        && checkpoint.configuration_digest == descriptor.invocation.configuration_digest
        && checkpoint.model_context_profile_digest == input.model_context_profile_digest
        && checkpoint.source_digest == source_digest
        && checkpoint.summary_digest == summary_digest
        && sensitivity_rank(checkpoint.sensitivity) >= source_sensitivity
        && summary_is_safe)
}

const fn sensitivity_rank(value: Sensitivity) -> u8 {
    match value {
        Sensitivity::Public => 0,
        Sensitivity::Internal => 1,
        Sensitivity::Confidential => 2,
        Sensitivity::Secret => 3,
        Sensitivity::Credential => 4,
    }
}

fn tool_pairs<'a>(
    messages: impl IntoIterator<Item = &'a Message>,
) -> Result<BTreeMap<ToolCallId, bool>, MiddlewareError> {
    let mut pairs = BTreeMap::<ToolCallId, bool>::new();
    for message in messages {
        let known = pairs.keys().copied().collect::<Vec<_>>();
        message
            .validate_tool_associations(Some(&known))
            .map_err(|_| MiddlewareError::compaction_invalid())?;
        for block in message.content() {
            match block {
                finstack_ai_kernel::ContentBlock::ToolCall(call) => {
                    if pairs.insert(*call.tool_call_id(), false).is_some() {
                        return Err(MiddlewareError::compaction_invalid());
                    }
                }
                finstack_ai_kernel::ContentBlock::ToolResult(result) => {
                    let Some(value) = pairs.get_mut(result.tool_call_id()) else {
                        return Err(MiddlewareError::compaction_invalid());
                    };
                    *value = true;
                }
                _ => {}
            }
        }
    }
    Ok(pairs)
}

fn canonical_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, MiddlewareError> {
    serde_json_canonicalizer::to_vec(value).map_err(|_| {
        MiddlewareError::stable(
            MIDDLEWARE_OUTCOME_NOT_ALLOWED,
            "middleware value could not be canonicalized",
        )
    })
}

fn digest_value<T: Serialize>(domain: &'static str, value: &T) -> Result<Digest, MiddlewareError> {
    Digest::domain_separated(domain, 1, &canonical_bytes(value)?).map_err(|_| {
        MiddlewareError::stable(
            COMPACTION_RESULT_INVALID,
            "compaction digest could not be constructed",
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

#[cfg(test)]
mod tests {
    use super::*;
    use finstack_ai_kernel::{
        ContentBlock, EffectId, EffectOutputContract, EffectRelation, EffectTag, EntryTag,
        EventTag, Id, IdTag, InteractionKind, InteractionTag, InvocationRecovery, LaneTag,
        MessageRole, OutputSpec, ProviderIds, RECORD_FORMAT_VERSION, RECORD_KIND_VERSION,
        RecordTag, RunTag, SessionTag, TextBlock, Timestamp, ToolCallBlock, ToolCallTag,
        ToolResultBlock, Version,
    };

    #[cfg(feature = "native-tokio")]
    use crate::{AuthorizationContext, CancellationSignal};
    use crate::{
        ContextAuthority, ContextItemKind, ContextProvenance, ModelName, ModelRequestLimits,
        ModelSettings,
    };
    #[cfg(feature = "native-tokio")]
    use finstack_ai_kernel::PrincipalRef;

    fn id<T: IdTag>(value: u64) -> Id<T> {
        let mut bytes = [0_u8; 16];
        bytes[6] = 0x70;
        bytes[8] = 0x80;
        bytes[9..].copy_from_slice(&value.to_be_bytes()[1..]);
        Id::from_bytes(bytes)
    }

    struct Stub {
        descriptor: MiddlewareDescriptor,
    }

    impl Middleware for Stub {
        fn descriptor(&self) -> MiddlewareDescriptor {
            self.descriptor.clone()
        }

        fn invoke(
            &self,
            _ctx: MiddlewareContext,
            _input: StageInput,
        ) -> PortFuture<Result<StageOutcome, MiddlewareError>> {
            Box::pin(async { Ok(StageOutcome::Continue) })
        }
    }

    fn descriptor(id: &str, before: &[&str], after: &[&str]) -> MiddlewareDescriptor {
        MiddlewareDescriptor {
            invocation: ComponentInvocation {
                component: ComponentId::parse(id).expect("component"),
                version: Version {
                    major: 1,
                    minor: 0,
                    patch: 0,
                },
                configuration_digest: Digest::raw_json(b"{}"),
                recovery: InvocationRecovery::RecomputeSafe,
            },
            stages: StageMask::from_stages([Stage::BeforeRun]),
            order: MiddlewareOrder {
                tier: OrderTier::Standard,
                priority: 0,
                before: before
                    .iter()
                    .map(|value| ComponentId::parse(*value).expect("before"))
                    .collect::<Vec<_>>()
                    .into(),
                after: after
                    .iter()
                    .map(|value| ComponentId::parse(*value).expect("after"))
                    .collect::<Vec<_>>()
                    .into(),
            },
            role: MiddlewareRole::Standard,
            metadata: Metadata::empty(),
        }
    }

    fn compactor_descriptor(id: &str) -> MiddlewareDescriptor {
        MiddlewareDescriptor {
            invocation: ComponentInvocation {
                component: ComponentId::parse(id).expect("component"),
                version: Version {
                    major: 1,
                    minor: 0,
                    patch: 0,
                },
                configuration_digest: Digest::raw_json(b"{\"window\":2}"),
                recovery: InvocationRecovery::Reconcile,
            },
            stages: StageMask::from_stages([Stage::BeforeModel]),
            order: MiddlewareOrder {
                tier: OrderTier::ContextCompaction,
                priority: 0,
                before: Arc::from([]),
                after: Arc::from([]),
            },
            role: MiddlewareRole::ContextCompactor {
                strategy_id: Arc::from("fixture.window"),
                strategy_version: 1,
            },
            metadata: Metadata::empty(),
        }
    }

    fn message(ordinal: u64, role: MessageRole, content: Vec<ContentBlock>) -> Message {
        Message::try_new(
            id(ordinal),
            role,
            content,
            Timestamp::from_unix_ms(i64::try_from(ordinal).expect("timestamp")).expect("timestamp"),
            None,
            ProviderIds::empty(),
            Metadata::empty(),
        )
        .expect("message")
    }

    fn model_draft(messages: Arc<[Message]>) -> ModelRequestDraft {
        ModelRequestDraft {
            model: ModelName::try_new("fixture-model").expect("model"),
            messages,
            tools: Arc::from([]),
            output: OutputSpec::PlainText,
            settings: ModelSettings {
                values: RawJson::parse(b"{}").expect("settings"),
            },
            limits: ModelRequestLimits {
                max_input_bytes: 1_000_000,
                max_input_tokens: 10_000,
                max_output_tokens: 1_000,
            },
        }
    }

    fn compaction_input() -> BeforeModelInput {
        let call_id = id::<ToolCallTag>(50);
        let system = message(
            10,
            MessageRole::System,
            vec![ContentBlock::Text(
                TextBlock::try_new("system policy").expect("text"),
            )],
        );
        let assistant = message(
            11,
            MessageRole::Assistant,
            vec![ContentBlock::ToolCall(
                ToolCallBlock::try_new(
                    call_id,
                    "fixture.lookup",
                    RawJson::parse(b"{}").expect("arguments"),
                )
                .expect("call"),
            )],
        );
        let tool = message(
            12,
            MessageRole::Tool,
            vec![ContentBlock::ToolResult(
                ToolResultBlock::try_new(
                    call_id,
                    vec![ContentBlock::Text(
                        TextBlock::try_new("large output").expect("text"),
                    )],
                    false,
                )
                .expect("result"),
            )],
        );
        let user = message(
            13,
            MessageRole::User,
            vec![ContentBlock::Text(
                TextBlock::try_new("current request").expect("text"),
            )],
        );
        let entries: Arc<[CompactionSourceEntry]> = Arc::from([
            CompactionSourceEntry {
                entry_id: id::<EntryTag>(20),
                message: system,
                sensitivity: Sensitivity::Internal,
                provenance_digest: Digest::raw_json(b"system"),
                protected: true,
            },
            CompactionSourceEntry {
                entry_id: id::<EntryTag>(21),
                message: assistant,
                sensitivity: Sensitivity::Internal,
                provenance_digest: Digest::raw_json(b"assistant"),
                protected: false,
            },
            CompactionSourceEntry {
                entry_id: id::<EntryTag>(22),
                message: tool,
                sensitivity: Sensitivity::Internal,
                provenance_digest: Digest::raw_json(b"tool"),
                protected: false,
            },
            CompactionSourceEntry {
                entry_id: id::<EntryTag>(23),
                message: user,
                sensitivity: Sensitivity::Internal,
                provenance_digest: Digest::raw_json(b"user"),
                protected: true,
            },
        ]);
        BeforeModelInput {
            request: model_draft(
                entries
                    .iter()
                    .map(|entry| entry.message.clone())
                    .collect::<Vec<_>>()
                    .into(),
            ),
            source_entries: entries,
            model_context_profile_digest: Digest::raw_json(b"profile"),
            hard_input_tokens: 1_000,
            checkpoint: None,
        }
    }

    fn valid_compaction(
        descriptor: &MiddlewareDescriptor,
        input: &BeforeModelInput,
    ) -> CompactionResult {
        let replacement_messages: Arc<[Message]> = Arc::from([
            input.source_entries[0].message.clone(),
            input.source_entries[3].message.clone(),
        ]);
        let summary = crate::ContextItem::try_new(
            ContextItemKind::DerivedSummary,
            vec![ContentBlock::Text(
                TextBlock::try_new("lookup completed").expect("text"),
            )],
            ContextProvenance {
                source_id: Arc::from("fixture.window"),
                source_ref: None,
                external: false,
            },
            ContextAuthority::Untrusted,
            0,
            4,
            Sensitivity::Internal,
            false,
        )
        .expect("summary");
        let derived_summaries: Arc<[crate::ContextItem]> = Arc::from([summary]);
        let checkpoint_summary = CompactedSummary::Inline(Arc::clone(&derived_summaries));
        let summary_digest =
            compaction_summary_digest(&checkpoint_summary).expect("summary digest");
        CompactionResult {
            replacement_messages: Arc::clone(&replacement_messages),
            derived_summaries,
            evidence: CompactionEvidence {
                strategy_id: Arc::from("fixture.window"),
                strategy_version: 1,
                configuration_digest: descriptor.invocation.configuration_digest,
                model_context_profile_digest: input.model_context_profile_digest,
                source_digest: compaction_source_digest(&input.source_entries).expect("source"),
                protected_item_set_digest: compaction_protected_set_digest(&[
                    input.source_entries[0].entry_id,
                    input.source_entries[3].entry_id,
                ])
                .expect("protected"),
                covered_entry_ids: input
                    .source_entries
                    .iter()
                    .map(|entry| entry.entry_id)
                    .collect::<Vec<_>>()
                    .into(),
                retained_entry_ids: Arc::from([
                    input.source_entries[0].entry_id,
                    input.source_entries[3].entry_id,
                ]),
                projection_digest: compaction_projection_digest(&replacement_messages)
                    .expect("projection"),
                estimated_tokens_before: 800,
                estimated_tokens_after: 200,
                summary_digest: Some(summary_digest),
                cache_impact: PromptCacheImpact::StablePrefixPreserved,
            },
            checkpoint: Some(CompactionCheckpoint {
                component_id: descriptor.invocation.component.clone(),
                strategy_id: Arc::from("fixture.window"),
                strategy_version: 1,
                configuration_digest: descriptor.invocation.configuration_digest,
                model_context_profile_digest: input.model_context_profile_digest,
                covered_through_entry_id: input.source_entries[3].entry_id,
                source_digest: compaction_source_digest(&input.source_entries).expect("source"),
                summary: checkpoint_summary,
                summary_digest,
                sensitivity: Sensitivity::Internal,
            }),
        }
    }

    #[test]
    fn cycles_fail_during_resolution() {
        let left: Arc<dyn Middleware> = Arc::new(Stub {
            descriptor: descriptor("fixture.left", &["fixture.right"], &[]),
        });
        let right: Arc<dyn Middleware> = Arc::new(Stub {
            descriptor: descriptor("fixture.right", &["fixture.left"], &[]),
        });
        let error = ResolvedMiddlewareChain::try_new(vec![
            MiddlewareRegistration { middleware: left },
            MiddlewareRegistration { middleware: right },
        ])
        .err()
        .expect("cycle");
        assert_eq!(error.code(), MIDDLEWARE_ORDER_CYCLE);
    }

    #[test]
    fn missing_requirements_duplicate_compactors_and_post_compactor_mutation_fail_resolution() {
        let missing: Arc<dyn Middleware> = Arc::new(Stub {
            descriptor: descriptor("fixture.missing", &["fixture.absent"], &[]),
        });
        assert_eq!(
            ResolvedMiddlewareChain::try_new(vec![MiddlewareRegistration {
                middleware: missing,
            }])
            .err()
            .expect("missing")
            .code(),
            MIDDLEWARE_RESOLUTION_INVALID
        );

        let first: Arc<dyn Middleware> = Arc::new(Stub {
            descriptor: compactor_descriptor("fixture.compactor-one"),
        });
        let second: Arc<dyn Middleware> = Arc::new(Stub {
            descriptor: compactor_descriptor("fixture.compactor-two"),
        });
        assert_eq!(
            ResolvedMiddlewareChain::try_new(vec![
                MiddlewareRegistration { middleware: first },
                MiddlewareRegistration { middleware: second },
            ])
            .err()
            .expect("duplicate compactor")
            .code(),
            MIDDLEWARE_RESOLUTION_INVALID
        );

        let compactor: Arc<dyn Middleware> = Arc::new(Stub {
            descriptor: compactor_descriptor("fixture.compactor"),
        });
        let mut mutator_descriptor = descriptor("fixture.mutator", &[], &["fixture.compactor"]);
        mutator_descriptor.stages = StageMask::from_stages([Stage::BeforeModel]);
        mutator_descriptor.order.tier = OrderTier::ContextMutation;
        let mutator: Arc<dyn Middleware> = Arc::new(Stub {
            descriptor: mutator_descriptor,
        });
        assert_eq!(
            ResolvedMiddlewareChain::try_new(vec![
                MiddlewareRegistration {
                    middleware: compactor,
                },
                MiddlewareRegistration {
                    middleware: mutator,
                },
            ])
            .err()
            .expect("post-compactor mutation")
            .code(),
            MIDDLEWARE_ORDER_CYCLE
        );
    }

    #[test]
    fn before_finalize_accepts_interaction_but_rejects_replacement() {
        let descriptor = MiddlewareDescriptor {
            stages: StageMask::from_stages([Stage::BeforeFinalize]),
            ..descriptor("fixture.finalize", &[], &[])
        };
        let input = StageInput::BeforeFinalize {
            candidate: RawJson::parse(b"{\"answer\":42}").expect("candidate"),
        };
        assert_eq!(
            validate_stage_outcome(
                &descriptor,
                &input,
                &StageOutcome::Replace(RawJson::parse(b"{}").expect("replacement")),
            )
            .expect_err("replacement")
            .code(),
            MIDDLEWARE_OUTCOME_NOT_ALLOWED
        );
        let interaction = InteractionRequest::try_new(
            1,
            id::<InteractionTag>(70),
            id::<EffectTag>(71),
            InteractionKind::Approval,
            vec![ContentBlock::Text(
                TextBlock::try_new("approve completion").expect("prompt"),
            )],
            RawJson::parse(
                br#"{"additionalProperties":false,"properties":{"approved":{"type":"boolean"}},"required":["approved"],"type":"object"}"#,
            )
            .expect("schema"),
            ComponentRef::new(
                descriptor.invocation.component.clone(),
                Some(descriptor.invocation.version),
            ),
            descriptor.invocation.version,
            None,
            None,
            false,
            Metadata::empty(),
        )
        .expect("interaction");
        validate_stage_outcome(
            &descriptor,
            &input,
            &StageOutcome::RequestInteraction(Box::new(interaction)),
        )
        .expect("interaction allowed");
    }

    #[test]
    fn compaction_preserves_protected_bytes_tool_pair_atomicity_and_canonical_history() {
        let descriptor = compactor_descriptor("fixture.compactor");
        let input = compaction_input();
        let canonical_before = input.source_entries.clone();
        let valid = valid_compaction(&descriptor, &input);
        validate_compaction_result(&descriptor, &input, &valid).expect("valid compaction");
        assert!(
            compaction_checkpoint_compatible(
                &descriptor,
                &input,
                valid.checkpoint.as_ref().expect("checkpoint")
            )
            .expect("checkpoint compatibility")
        );
        assert_eq!(input.source_entries, canonical_before);

        let mut stale_checkpoint = valid.checkpoint.clone().expect("checkpoint");
        stale_checkpoint.configuration_digest = Digest::raw_json(b"stale-config");
        assert!(
            !compaction_checkpoint_compatible(&descriptor, &input, &stale_checkpoint)
                .expect("stale checkpoint is an ordinary cache miss")
        );

        let mut missing_user = valid.clone();
        missing_user.replacement_messages = Arc::from([input.source_entries[0].message.clone()]);
        missing_user.evidence.retained_entry_ids = Arc::from([input.source_entries[0].entry_id]);
        missing_user.evidence.projection_digest =
            compaction_projection_digest(&missing_user.replacement_messages).expect("digest");
        assert_eq!(
            validate_compaction_result(&descriptor, &input, &missing_user)
                .expect_err("protected user")
                .code(),
            COMPACTION_RESULT_INVALID
        );

        let mut reordered = valid.clone();
        reordered.replacement_messages = Arc::from([
            input.source_entries[3].message.clone(),
            input.source_entries[0].message.clone(),
        ]);
        reordered.evidence.retained_entry_ids = Arc::from([
            input.source_entries[3].entry_id,
            input.source_entries[0].entry_id,
        ]);
        reordered.evidence.projection_digest =
            compaction_projection_digest(&reordered.replacement_messages).expect("digest");
        assert_eq!(
            validate_compaction_result(&descriptor, &input, &reordered)
                .expect_err("source reorder")
                .code(),
            COMPACTION_RESULT_INVALID
        );

        let mut split_pair = valid;
        split_pair.replacement_messages = Arc::from([
            input.source_entries[0].message.clone(),
            input.source_entries[1].message.clone(),
            input.source_entries[3].message.clone(),
        ]);
        split_pair.evidence.retained_entry_ids = Arc::from([
            input.source_entries[0].entry_id,
            input.source_entries[1].entry_id,
            input.source_entries[3].entry_id,
        ]);
        split_pair.evidence.projection_digest =
            compaction_projection_digest(&split_pair.replacement_messages).expect("digest");
        assert_eq!(
            validate_compaction_result(&descriptor, &input, &split_pair)
                .expect_err("split pair")
                .code(),
            COMPACTION_RESULT_INVALID
        );
    }

    #[cfg(feature = "native-tokio")]
    fn run(effect_id: EffectId) -> RunCallContext {
        RunCallContext {
            locator: finstack_ai_kernel::OperationLocator::try_new(
                "tenant-a",
                id::<SessionTag>(1),
                id::<LaneTag>(2),
                id::<RunTag>(3),
            )
            .expect("locator"),
            authorization: AuthorizationContext {
                principal: PrincipalRef::try_new("issuer", "subject", Some("tenant-a"))
                    .expect("principal"),
                authentication_method: Arc::from("fixture"),
                assurance_level: Arc::from("high"),
                roles: Arc::from([]),
                permitted_scopes: Arc::from([Arc::from("tenant-a")]),
                safe_claims: Metadata::empty(),
                policy_version: Arc::from("v1"),
                decision_id: Arc::from("decision-1"),
            },
            effect_id,
            attempt: 1,
            deadline: None,
            budget_scope_id: None,
            cancellation: CancellationSignal::new(),
        }
    }

    fn envelope(sequence: u64, body: RecordBody) -> RecordEnvelope {
        let events = (0..body
            .derived_event_count(RECORD_KIND_VERSION)
            .expect("events"))
            .map(|offset| id::<EventTag>(100 + sequence + u64::try_from(offset).expect("offset")))
            .collect();
        RecordEnvelope::try_new(
            RECORD_FORMAT_VERSION,
            RECORD_KIND_VERSION,
            id::<RecordTag>(10 + sequence),
            id::<SessionTag>(1),
            id::<LaneTag>(2),
            Some(id::<RunTag>(3)),
            sequence,
            Timestamp::from_unix_ms(1_000).expect("timestamp"),
            None,
            Digest::raw_json(b"{}"),
            None,
            Digest::raw_json(b"{}"),
            events,
            body,
        )
        .expect("envelope")
    }

    fn middleware_effect(
        effect_id: EffectId,
        descriptor: &MiddlewareDescriptor,
        input: &StageInput,
    ) -> EffectRequested {
        EffectRequested::try_new(
            effect_id,
            EffectKind::Middleware,
            None,
            Some(descriptor.invocation.clone()),
            Some(
                PipelinePosition::try_new(
                    Digest::raw_json(b"middleware-chain"),
                    stage_name(input.stage()),
                    0,
                )
                .expect("pipeline"),
            ),
            EffectOutputContract {
                kind: EffectOutputKind::MiddlewareOutcome,
                schema_version: 1,
                schema_digest: Digest::raw_json(b"middleware-outcome-v1"),
            },
            EffectInput::Middleware {
                stage: Arc::from(stage_name(input.stage())),
                input: input.to_raw_json().expect("input"),
            },
            RetrySafety::SafeToRetry,
            None,
        )
        .expect("effect")
    }

    #[cfg(feature = "native-tokio")]
    #[tokio::test]
    async fn committed_middleware_cursor_is_reused_and_recorded_before_application() {
        let descriptor = descriptor("fixture.committed", &[], &[]);
        let input = StageInput::BeforeRun {
            value: RawJson::parse(b"{}").expect("input"),
        };
        let effect_id = id::<EffectTag>(4);
        let requested = middleware_effect(effect_id, &descriptor, &input);
        let requested_envelope = envelope(1, RecordBody::EffectRequested(requested.clone()));
        let middleware = Stub {
            descriptor: descriptor.clone(),
        };
        let context = MiddlewareContext {
            run: run(effect_id),
            chain_digest: Digest::raw_json(b"middleware-chain"),
            chain_index: 0,
            compaction_resume: None,
        };
        let outcome =
            CommittedMiddlewareCall::try_new(&requested_envelope, context, input, &descriptor)
                .expect("committed guard")
                .invoke(&middleware)
                .await
                .expect("invoke");
        let completed = EffectCompleted::try_new(
            effect_id,
            requested.output_contract().clone(),
            outcome.to_raw_json().expect("outcome"),
            None,
            Vec::new(),
            ProviderIds::empty(),
            None::<&str>,
            None,
        )
        .expect("completion");
        let completed_envelope = envelope(2, RecordBody::EffectCompleted(completed.clone()));
        let recorded =
            RecordedMiddlewareOutcome::try_from_records(&requested_envelope, &completed_envelope)
                .expect("recorded");
        assert_eq!(recorded.stage, Stage::BeforeRun);
        assert_eq!(recorded.outcome, StageOutcome::Continue);
        assert_eq!(
            middleware_resume_action(&requested, Some(&completed)),
            InvocationResumeAction::UseRecorded
        );
    }

    #[test]
    fn compaction_child_model_requires_exact_committed_relation_and_input() {
        let descriptor = compactor_descriptor("fixture.compactor");
        let input = StageInput::BeforeModel(Box::new(compaction_input()));
        let parent_id = id::<EffectTag>(80);
        let parent = middleware_effect(parent_id, &descriptor, &input);
        let model_component = ComponentRef::new(
            ComponentId::parse("fixture.summary-model").expect("model component"),
            Some(Version {
                major: 1,
                minor: 0,
                patch: 0,
            }),
        );
        let request = CompactionModelRequest {
            model: model_component.clone(),
            request: model_draft(Arc::from([])),
            budget_scope_id: id(81),
            source_sensitivity: Sensitivity::Internal,
            residency_policy_digest: Digest::raw_json(b"residency"),
            resume_state: RawJson::parse(b"{}").expect("resume"),
        };
        let child = EffectRequested::try_new(
            id::<EffectTag>(82),
            EffectKind::Model,
            Some(EffectRelation {
                parent_effect_id: parent_id,
                purpose: EffectPurpose::CompactionSummary {
                    middleware_component_id: descriptor.invocation.component.clone(),
                },
            }),
            Some(ComponentInvocation {
                component: model_component.id().clone(),
                version: model_component.version().expect("version"),
                configuration_digest: Digest::raw_json(b"model-config"),
                recovery: InvocationRecovery::Reconcile,
            }),
            None,
            EffectOutputContract {
                kind: EffectOutputKind::ModelResponse,
                schema_version: 1,
                schema_digest: Digest::raw_json(b"model-response-v1"),
            },
            EffectInput::Model {
                request: RawJson::parse(request.request.canonical_bytes().expect("request"))
                    .expect("raw"),
            },
            RetrySafety::IdempotentWithKey,
            None,
        )
        .expect("child");
        let child_envelope = envelope(2, RecordBody::EffectRequested(child.clone()));
        validate_compaction_model_effect(&parent, &child_envelope, &request)
            .expect("related child");

        let wrong_relation = EffectRequested::try_new(
            id::<EffectTag>(83),
            child.kind(),
            Some(EffectRelation {
                parent_effect_id: id::<EffectTag>(84),
                purpose: EffectPurpose::CompactionSummary {
                    middleware_component_id: descriptor.invocation.component.clone(),
                },
            }),
            child.component().cloned(),
            child.pipeline().cloned(),
            child.output_contract().clone(),
            child.input().clone(),
            child.retry_safety(),
            child.deadline(),
        )
        .expect("wrong relation fixture");
        assert_eq!(
            validate_compaction_model_effect(
                &parent,
                &envelope(3, RecordBody::EffectRequested(wrong_relation)),
                &request,
            )
            .expect_err("wrong parent relation")
            .code(),
            COMPACTION_MODEL_NOT_AUTHORIZED
        );

        let mut wrong = request;
        wrong.resume_state = RawJson::parse(b"{\"different\":true}").expect("state");
        // Resume state belongs to the parent cursor and does not change child provider input.
        validate_compaction_model_effect(&parent, &child_envelope, &wrong)
            .expect("opaque parent resume state");
    }
}
