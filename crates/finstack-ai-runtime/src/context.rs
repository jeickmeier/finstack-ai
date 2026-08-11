//! Context-provider port, committed invocation guard, and deterministic assembly.

use std::collections::BTreeSet;
use std::sync::Arc;

use finstack_ai_kernel::{
    CapabilityId, ComponentId, ComponentInvocation, ContentBlock, Digest, EffectCompleted,
    EffectInput, EffectKind, EffectOutputKind, EffectRequested, ErrorCategory, ErrorCode,
    ErrorDescriptor, InvocationRecovery, LaneId, Message, Metadata, PipelinePosition, RawJson,
    RecordBody, RecordEnvelope, RetrySafety, RunId, SEMANTIC_ARRAY_MAX_ITEMS, Sensitivity,
    SessionId,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{PortFuture, PortObject, ReconcileContext, RunCallContext};

/// Stable invalid-context-configuration code.
pub const CONTEXT_CONFIGURATION_INVALID: &str = "context_configuration_invalid";
/// Stable missing-commit-boundary code.
pub const CONTEXT_COMMIT_REQUIRED: &str = "context_commit_required";
/// Stable invalid-contribution code.
pub const CONTEXT_CONTRIBUTION_INVALID: &str = "context_contribution_invalid";
/// Stable context-budget exhaustion code.
pub const CONTEXT_BUDGET_EXCEEDED: &str = "context_budget_exceeded";
/// Stable unresolved non-repeatable context-invocation code.
pub const CONTEXT_RECOVERY_UNCERTAIN: &str = "context_recovery_uncertain";

const CONTEXT_STAGE: &str = "prepare_context";

/// Deterministic policy for a context-budget overrun.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextOverflowPolicy {
    /// Reject the contribution without changing it.
    Reject,
    /// Retain the highest-priority whole items and report every omitted item.
    TruncateWithDiagnostic,
}

/// Explicit item, token, and byte budget supplied to one context request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextBudget {
    /// Maximum accepted item count.
    pub max_items: usize,
    /// Maximum accepted estimated input tokens.
    pub max_tokens: u64,
    /// Maximum accepted canonical item bytes.
    pub max_bytes: u64,
    /// Deterministic overrun behavior.
    pub overflow: ContextOverflowPolicy,
}

impl ContextBudget {
    fn validate(self) -> Result<Self, ContextError> {
        if self.max_items > SEMANTIC_ARRAY_MAX_ITEMS {
            return Err(ContextError::stable(
                CONTEXT_CONFIGURATION_INVALID,
                "context item budget exceeds the semantic maximum",
            ));
        }
        Ok(self)
    }
}

/// Authority assigned to a provider contribution after descriptor policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextAuthority {
    /// Delimited data that cannot grant instruction authority.
    Untrusted,
    /// Application instruction explicitly authorized in the resolved descriptor.
    TrustedApplication,
}

/// Normalized context-item class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextItemKind {
    /// Application instruction, subject to descriptor authorization.
    Instruction,
    /// Quoted external content.
    QuotedSource,
    /// Reference to external content.
    Reference,
    /// Non-authoritative metadata.
    Metadata,
    /// Hidden application-owned context.
    HiddenApplicationContext,
    /// Derived, untrusted compaction summary.
    DerivedSummary,
}

/// Mandatory provenance for one context item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextProvenance {
    /// Stable source identifier safe to expose in diagnostics.
    pub source_id: Arc<str>,
    /// Optional source locator that contains no bearer credentials.
    pub source_ref: Option<Arc<str>>,
    /// Whether the content came from retrieval or another external source.
    pub external: bool,
}

impl ContextProvenance {
    fn validate(&self) -> Result<(), ContextError> {
        validate_label(&self.source_id, "context provenance source")?;
        if let Some(value) = &self.source_ref {
            validate_text(value, "context provenance reference")?;
        }
        Ok(())
    }
}

/// One typed, attributed, budgeted context item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextItem {
    /// Item class.
    pub kind: ContextItemKind,
    /// Normalized content blocks.
    pub content: Arc<[ContentBlock]>,
    /// Mandatory attribution.
    pub provenance: ContextProvenance,
    /// Authority after runtime normalization.
    pub authority: ContextAuthority,
    /// Higher values sort before lower values within one provider.
    pub priority: i32,
    /// Provider estimate used for deterministic budget accounting.
    pub estimated_tokens: u64,
    /// Canonical byte count of `content`.
    pub bytes: u64,
    /// Data sensitivity carried into model and observer policy.
    pub sensitivity: Sensitivity,
    /// Whether compaction is forbidden from removing this item.
    pub protected: bool,
}

impl ContextItem {
    /// Construct an item and compute its exact canonical content byte count.
    ///
    /// # Errors
    ///
    /// Returns a stable contribution error for empty, invalid, or non-encodable content.
    #[allow(clippy::too_many_arguments)]
    pub fn try_new(
        kind: ContextItemKind,
        content: Vec<ContentBlock>,
        provenance: ContextProvenance,
        authority: ContextAuthority,
        priority: i32,
        estimated_tokens: u64,
        sensitivity: Sensitivity,
        protected: bool,
    ) -> Result<Self, ContextError> {
        let shared: Arc<[ContentBlock]> = content.into();
        let bytes = u64::try_from(canonical_bytes(&shared)?.len()).map_err(|_| {
            ContextError::stable(
                CONTEXT_CONTRIBUTION_INVALID,
                "context item byte count overflowed",
            )
        })?;
        let item = Self {
            kind,
            content: shared,
            provenance,
            authority,
            priority,
            estimated_tokens,
            bytes,
            sensitivity,
            protected,
        };
        item.validate()?;
        Ok(item)
    }

    fn validate(&self) -> Result<(), ContextError> {
        self.provenance.validate()?;
        if self.content.is_empty() {
            return Err(ContextError::stable(
                CONTEXT_CONTRIBUTION_INVALID,
                "context item content is empty",
            ));
        }
        let bytes = canonical_bytes(&self.content)?;
        if u64::try_from(bytes.len()).ok() != Some(self.bytes) {
            return Err(ContextError::stable(
                CONTEXT_CONTRIBUTION_INVALID,
                "context item byte estimate does not match canonical content",
            ));
        }
        if self.kind == ContextItemKind::DerivedSummary
            && self.authority != ContextAuthority::Untrusted
        {
            return Err(ContextError::stable(
                CONTEXT_CONTRIBUTION_INVALID,
                "derived context cannot acquire instruction authority",
            ));
        }
        Ok(())
    }
}

/// Bounded request passed to one context provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextRequest {
    /// Session identity repeated in the committed provider input.
    pub session_id: SessionId,
    /// Lane identity repeated in the committed provider input.
    pub lane_id: LaneId,
    /// Run identity repeated in the committed provider input.
    pub run_id: RunId,
    /// Current user input blocks.
    pub user_input: Arc<[ContentBlock]>,
    /// Canonical recent history supplied by the runtime.
    pub recent_history: Arc<[Message]>,
    /// Provider-local budget.
    pub budget: ContextBudget,
    /// Active capabilities in locked activation order.
    pub active_capabilities: Arc<[CapabilityId]>,
}

impl ContextRequest {
    /// Canonical JSON bytes committed as `EffectInput::Context`.
    ///
    /// # Errors
    ///
    /// Returns a stable configuration error when the request is invalid or cannot be encoded.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, ContextError> {
        self.budget.validate()?;
        if self.user_input.len() > SEMANTIC_ARRAY_MAX_ITEMS
            || self.recent_history.len() > SEMANTIC_ARRAY_MAX_ITEMS
            || self.active_capabilities.len() > SEMANTIC_ARRAY_MAX_ITEMS
        {
            return Err(ContextError::stable(
                CONTEXT_CONFIGURATION_INVALID,
                "context request exceeds a semantic array bound",
            ));
        }
        canonical_bytes(self)
    }

    /// Canonical raw JSON committed as the effect input.
    ///
    /// # Errors
    ///
    /// Returns a stable configuration error for invalid or non-encodable input.
    pub fn to_raw_json(&self) -> Result<RawJson, ContextError> {
        RawJson::parse(self.canonical_bytes()?).map_err(|_| {
            ContextError::stable(
                CONTEXT_CONFIGURATION_INVALID,
                "context request could not be normalized",
            )
        })
    }
}

/// Normalized provider output before committed assembly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextContribution {
    /// Source-ordered provider items.
    pub items: Arc<[ContextItem]>,
    /// Sum of item token estimates.
    pub estimated_tokens: u64,
    /// Sum of item canonical byte counts.
    pub bytes: u64,
    /// Optional bounded cache identity.
    pub cache_key: Option<Arc<str>>,
}

impl ContextContribution {
    /// Construct a contribution and compute exact aggregate item totals.
    ///
    /// # Errors
    ///
    /// Returns a stable contribution error for invalid items or checked-sum overflow.
    pub fn try_new(
        items: Vec<ContextItem>,
        cache_key: Option<impl AsRef<str>>,
    ) -> Result<Self, ContextError> {
        let mut tokens = 0_u64;
        let mut bytes = 0_u64;
        for item in &items {
            item.validate()?;
            tokens = tokens.checked_add(item.estimated_tokens).ok_or_else(|| {
                ContextError::stable(
                    CONTEXT_CONTRIBUTION_INVALID,
                    "context token estimate overflowed",
                )
            })?;
            bytes = bytes.checked_add(item.bytes).ok_or_else(|| {
                ContextError::stable(
                    CONTEXT_CONTRIBUTION_INVALID,
                    "context byte estimate overflowed",
                )
            })?;
        }
        let contribution = Self {
            items: items.into(),
            estimated_tokens: tokens,
            bytes,
            cache_key: cache_key.map(|value| Arc::from(value.as_ref())),
        };
        contribution.validate()?;
        Ok(contribution)
    }

    fn validate(&self) -> Result<(), ContextError> {
        if self.items.len() > SEMANTIC_ARRAY_MAX_ITEMS {
            return Err(ContextError::stable(
                CONTEXT_CONTRIBUTION_INVALID,
                "context contribution contains too many items",
            ));
        }
        if let Some(value) = &self.cache_key {
            validate_label(value, "context cache key")?;
        }
        let mut tokens = 0_u64;
        let mut bytes = 0_u64;
        for item in self.items.iter() {
            item.validate()?;
            tokens = tokens.checked_add(item.estimated_tokens).ok_or_else(|| {
                ContextError::stable(
                    CONTEXT_CONTRIBUTION_INVALID,
                    "context token estimate overflowed",
                )
            })?;
            bytes = bytes.checked_add(item.bytes).ok_or_else(|| {
                ContextError::stable(
                    CONTEXT_CONTRIBUTION_INVALID,
                    "context byte estimate overflowed",
                )
            })?;
        }
        if tokens != self.estimated_tokens || bytes != self.bytes {
            return Err(ContextError::stable(
                CONTEXT_CONTRIBUTION_INVALID,
                "context contribution totals do not match its items",
            ));
        }
        Ok(())
    }

    /// Convert the normalized contribution into canonical effect output.
    ///
    /// # Errors
    ///
    /// Returns a stable contribution error for invalid or non-encodable output.
    pub fn to_raw_json(&self) -> Result<RawJson, ContextError> {
        self.validate()?;
        RawJson::parse(canonical_bytes(self)?).map_err(|_| {
            ContextError::stable(
                CONTEXT_CONTRIBUTION_INVALID,
                "context contribution could not be normalized",
            )
        })
    }
}

/// Immutable context-provider descriptor locked at agent resolution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextProviderDescriptor {
    /// Exact component/version/configuration and recovery class.
    pub invocation: ComponentInvocation,
    /// Whether this provider may contribute trusted application instructions.
    pub trusted_application_instructions: bool,
    /// Non-secret descriptor metadata.
    #[serde(default)]
    pub metadata: Metadata,
}

/// Context-specific committed call context.
#[derive(Debug, Clone)]
pub struct ContextCallContext {
    /// Shared identity, authority, deadline, budget, and cancellation context.
    pub run: RunCallContext,
    /// Locked provider order in the resolved chain.
    pub provider_index: u32,
    /// Locked chain digest.
    pub chain_digest: Digest,
}

/// Outstanding context effect supplied to `reconcile`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingContextEffect {
    /// Frozen request.
    pub request: ContextRequest,
    /// Committed component invocation.
    pub invocation: ComponentInvocation,
    /// Committed pipeline position.
    pub pipeline: PipelinePosition,
}

/// Context reconciliation result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextReconcileResult {
    /// Normalized output is available.
    Completed(ContextContribution),
    /// The original call provably did not start.
    NotStarted,
    /// Reusing the same effect identity is safe.
    RetrySafe,
    /// The provider cannot classify the effect.
    Unknown,
    /// A non-repeatable external action may have occurred.
    NonRepeatable,
}

/// Object-safe context-provider port.
pub trait ContextProvider: PortObject {
    /// Immutable descriptor.
    fn descriptor(&self) -> ContextProviderDescriptor;

    /// Collect one committed bounded contribution.
    fn collect(
        &self,
        ctx: ContextCallContext,
        request: ContextRequest,
    ) -> PortFuture<Result<ContextContribution, ContextError>>;

    /// Reconcile an outstanding committed invocation.
    fn reconcile(
        &self,
        _ctx: ReconcileContext,
        _effect: PendingContextEffect,
    ) -> PortFuture<Result<ContextReconcileResult, ContextError>> {
        Box::pin(async { Ok(ContextReconcileResult::Unknown) })
    }
}

/// Guard proving a context invocation is backed by a committed effect record.
#[derive(Debug, Clone)]
pub struct CommittedContextCall {
    context: ContextCallContext,
    request: ContextRequest,
    requested: EffectRequested,
    sequence: u64,
}

impl CommittedContextCall {
    /// Validate a committed envelope against the exact provider, request, locator, and cursor.
    ///
    /// # Errors
    ///
    /// Returns `context_commit_required` when any committed identity or digest differs.
    pub fn try_new(
        envelope: &RecordEnvelope,
        context: ContextCallContext,
        request: ContextRequest,
        descriptor: &ContextProviderDescriptor,
    ) -> Result<Self, ContextError> {
        let RecordBody::EffectRequested(requested) = envelope.body() else {
            return Err(ContextError::commit_required());
        };
        let raw = request.to_raw_json()?;
        let expected_pipeline = requested
            .pipeline()
            .ok_or_else(ContextError::commit_required)?;
        let locator = &context.run.locator;
        if envelope.sequence() == 0
            || envelope.session_id() != locator.session_id
            || envelope.lane_id() != locator.lane_id
            || envelope.run_id() != Some(locator.run_id)
            || request.session_id != locator.session_id
            || request.lane_id != locator.lane_id
            || request.run_id != locator.run_id
            || requested.effect_id() != context.run.effect_id
            || requested.kind() != EffectKind::Context
            || requested.component() != Some(&descriptor.invocation)
            || expected_pipeline.chain_digest() != context.chain_digest
            || expected_pipeline.stage() != CONTEXT_STAGE
            || expected_pipeline.index() != context.provider_index
            || requested.deadline() != context.run.deadline
            || requested.retry_safety() == RetrySafety::Unknown
            || requested.output_contract().kind != EffectOutputKind::ContextContribution
            || !matches!(requested.input(), EffectInput::Context { request } if request == &raw)
        {
            return Err(ContextError::commit_required());
        }
        Ok(Self {
            context,
            request,
            requested: requested.clone(),
            sequence: envelope.sequence(),
        })
    }

    /// Invoke the exact provider only after the committed guard has been constructed.
    ///
    /// # Errors
    ///
    /// Returns a stable error when the descriptor drifts, the provider fails, or output is invalid.
    pub async fn invoke(
        self,
        provider: &dyn ContextProvider,
    ) -> Result<ContextContribution, ContextError> {
        let descriptor = provider.descriptor();
        if self.requested.component() != Some(&descriptor.invocation) {
            return Err(ContextError::commit_required());
        }
        let mut contribution = provider.collect(self.context, self.request.clone()).await?;
        contribution.validate()?;
        normalize_instruction_authority(
            &mut contribution,
            descriptor.trusted_application_instructions,
        );
        contribution.validate()?;
        if self.request.budget.overflow == ContextOverflowPolicy::Reject
            && !contribution_fits_budget(&contribution, self.request.budget)
        {
            return Err(ContextError::stable(
                CONTEXT_BUDGET_EXCEEDED,
                "context provider contribution exceeds its explicit budget",
            ));
        }
        Ok(contribution)
    }

    /// Store-assigned request sequence.
    #[must_use]
    pub const fn sequence(&self) -> u64 {
        self.sequence
    }
}

/// One diagnostic describing explicit deterministic context truncation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextTruncationDiagnostic {
    /// Provider component.
    pub component: ComponentId,
    /// Number of whole items omitted.
    pub dropped_items: u32,
    /// Omitted estimated tokens.
    pub dropped_tokens: u64,
    /// Omitted canonical bytes.
    pub dropped_bytes: u64,
}

/// A context output proven to have been committed after its request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedContextContribution {
    /// Provider component.
    pub component: ComponentId,
    /// Locked provider index.
    pub provider_index: u32,
    /// Normalized contribution.
    pub contribution: ContextContribution,
}

impl RecordedContextContribution {
    /// Reconstruct a replay-safe contribution from request and completion envelopes.
    ///
    /// # Errors
    ///
    /// Returns a stable contribution error for identity, order, or payload mismatch.
    pub fn try_from_records(
        requested_envelope: &RecordEnvelope,
        completed_envelope: &RecordEnvelope,
    ) -> Result<Self, ContextError> {
        let RecordBody::EffectRequested(requested) = requested_envelope.body() else {
            return Err(ContextError::commit_required());
        };
        let RecordBody::EffectCompleted(completed) = completed_envelope.body() else {
            return Err(ContextError::stable(
                CONTEXT_CONTRIBUTION_INVALID,
                "context completion record is missing",
            ));
        };
        let component = requested
            .component()
            .ok_or_else(ContextError::commit_required)?
            .component
            .clone();
        let pipeline = requested
            .pipeline()
            .ok_or_else(ContextError::commit_required)?;
        if requested.kind() != EffectKind::Context
            || requested.output_contract().kind != EffectOutputKind::ContextContribution
            || pipeline.stage() != CONTEXT_STAGE
            || completed_envelope.sequence() <= requested_envelope.sequence()
            || completed_envelope.session_id() != requested_envelope.session_id()
            || completed_envelope.lane_id() != requested_envelope.lane_id()
            || completed_envelope.run_id() != requested_envelope.run_id()
            || completed.validate_against(requested).is_err()
        {
            return Err(ContextError::stable(
                CONTEXT_CONTRIBUTION_INVALID,
                "context completion does not match its committed request",
            ));
        }
        let contribution: ContextContribution =
            serde_json::from_slice(completed.output().as_bytes()).map_err(|_| {
                ContextError::stable(
                    CONTEXT_CONTRIBUTION_INVALID,
                    "committed context contribution is invalid",
                )
            })?;
        contribution.validate()?;
        Ok(Self {
            component,
            provider_index: pipeline.index(),
            contribution,
        })
    }
}

/// Replay decision for a committed but unsettled component invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvocationResumeAction {
    /// Use the already committed output.
    UseRecorded,
    /// Rerun with the same effect identity.
    Recompute,
    /// Call the component reconciliation hook.
    Reconcile,
    /// Suspend because repeating the invocation could duplicate a non-repeatable effect.
    SuspendUncertain,
}

/// Determine recovery behavior from committed request/output state.
#[must_use]
pub fn context_resume_action(
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

/// Fully ordered provider-context projection and explicit truncation diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssembledContext {
    /// Trusted provider reminders before untrusted external items.
    pub items: Arc<[ContextItem]>,
    /// Explicit overrun diagnostics; never silently omitted.
    pub diagnostics: Arc<[ContextTruncationDiagnostic]>,
    /// Accepted estimated tokens.
    pub estimated_tokens: u64,
    /// Accepted canonical bytes.
    pub bytes: u64,
}

/// Locked capability instructions used during final context projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityContext {
    /// Capability identity in locked activation order.
    pub capability_id: CapabilityId,
    /// Trusted capability instruction items in declared order.
    pub instructions: Arc<[ContextItem]>,
}

/// Source group for one final model-visible projection item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextProjectionSource {
    /// Resolved system policy.
    System,
    /// Resolved developer/application instruction.
    Developer,
    /// Active capability instruction.
    Capability,
    /// Authorized provider application reminder.
    ProviderReminder,
    /// Delimited external provider context.
    ExternalContext,
    /// Canonical conversation history.
    History,
    /// Current user request, exactly once and last.
    CurrentUser,
}

/// Message or context item in the fixed-authority model projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextProjectionItem {
    /// Canonical message group.
    Message {
        /// Fixed source group.
        source: ContextProjectionSource,
        /// Immutable message.
        message: Message,
    },
    /// Provider/capability context item.
    Context {
        /// Fixed source group.
        source: ContextProjectionSource,
        /// Immutable normalized item.
        item: ContextItem,
    },
}

/// Inputs for fixed authority and ordering assembly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextProjectionInput {
    /// Resolved system messages in declared order.
    pub system: Arc<[Message]>,
    /// Resolved developer/application messages in declared order.
    pub developer: Arc<[Message]>,
    /// Active capabilities in locked activation order.
    pub capabilities: Arc<[CapabilityContext]>,
    /// Already budgeted and deterministically ordered provider context.
    pub providers: AssembledContext,
    /// Canonical conversation history in parent-chain order.
    pub history: Arc<[Message]>,
    /// Current user request.
    pub current_user: Message,
}

/// Assemble the fixed authority groups ending with exactly one current user request.
///
/// # Errors
///
/// Returns a stable configuration error for role drift, duplicate capability identity, invalid
/// capability instructions, or a duplicated current-user message.
pub fn assemble_context_projection(
    input: ContextProjectionInput,
) -> Result<Arc<[ContextProjectionItem]>, ContextError> {
    if input
        .system
        .iter()
        .any(|message| message.role() != finstack_ai_kernel::MessageRole::System)
        || input
            .developer
            .iter()
            .any(|message| message.role() != finstack_ai_kernel::MessageRole::Developer)
        || input.current_user.role() != finstack_ai_kernel::MessageRole::User
        || input
            .history
            .iter()
            .any(|message| message.id() == input.current_user.id())
    {
        return Err(ContextError::stable(
            CONTEXT_CONFIGURATION_INVALID,
            "context authority group contains an invalid role or duplicate current user",
        ));
    }
    let mut capability_ids = BTreeSet::new();
    for capability in input.capabilities.iter() {
        if !capability_ids.insert(capability.capability_id.clone())
            || capability.instructions.iter().any(|item| {
                item.kind != ContextItemKind::Instruction
                    || item.authority != ContextAuthority::TrustedApplication
                    || item.provenance.external
            })
        {
            return Err(ContextError::stable(
                CONTEXT_CONFIGURATION_INVALID,
                "capability context is duplicate or not trusted resolved instruction data",
            ));
        }
    }
    let capacity = input
        .system
        .len()
        .saturating_add(input.developer.len())
        .saturating_add(
            input
                .capabilities
                .iter()
                .map(|capability| capability.instructions.len())
                .sum::<usize>(),
        )
        .saturating_add(input.providers.items.len())
        .saturating_add(input.history.len())
        .saturating_add(1);
    if capacity > SEMANTIC_ARRAY_MAX_ITEMS {
        return Err(ContextError::stable(
            CONTEXT_CONFIGURATION_INVALID,
            "assembled context projection exceeds the semantic item bound",
        ));
    }
    let mut projection = Vec::with_capacity(capacity);
    projection.extend(
        input
            .system
            .iter()
            .cloned()
            .map(|message| ContextProjectionItem::Message {
                source: ContextProjectionSource::System,
                message,
            }),
    );
    projection.extend(input.developer.iter().cloned().map(|message| {
        ContextProjectionItem::Message {
            source: ContextProjectionSource::Developer,
            message,
        }
    }));
    for capability in input.capabilities.iter() {
        projection.extend(capability.instructions.iter().cloned().map(|item| {
            ContextProjectionItem::Context {
                source: ContextProjectionSource::Capability,
                item,
            }
        }));
    }
    projection.extend(input.providers.items.iter().cloned().map(|item| {
        let source = if item.kind == ContextItemKind::Instruction
            && item.authority == ContextAuthority::TrustedApplication
        {
            ContextProjectionSource::ProviderReminder
        } else {
            ContextProjectionSource::ExternalContext
        };
        ContextProjectionItem::Context { source, item }
    }));
    projection.extend(input.history.iter().cloned().map(|message| {
        ContextProjectionItem::Message {
            source: ContextProjectionSource::History,
            message,
        }
    }));
    projection.push(ContextProjectionItem::Message {
        source: ContextProjectionSource::CurrentUser,
        message: input.current_user,
    });
    Ok(projection.into())
}

/// Deterministically assemble only committed provider outputs.
///
/// Provider index, authority group, descending item priority, and original item order are the
/// complete ordering keys. Completion order and hash-map iteration never participate.
///
/// # Errors
///
/// Returns a stable budget or contribution error on duplicates, gaps, invalid items, or overrun.
pub fn assemble_context(
    mut recorded: Vec<RecordedContextContribution>,
    budget: ContextBudget,
) -> Result<AssembledContext, ContextError> {
    budget.validate()?;
    if recorded.len() > SEMANTIC_ARRAY_MAX_ITEMS {
        return Err(ContextError::stable(
            CONTEXT_CONFIGURATION_INVALID,
            "recorded context provider chain exceeds the semantic item bound",
        ));
    }
    recorded.sort_by_key(|item| item.provider_index);
    let mut components = BTreeSet::new();
    for (expected_index, item) in recorded.iter().enumerate() {
        if usize::try_from(item.provider_index).ok() != Some(expected_index)
            || !components.insert(item.component.clone())
        {
            return Err(ContextError::stable(
                CONTEXT_CONFIGURATION_INVALID,
                "recorded context provider order has a duplicate, gap, or unstable identity",
            ));
        }
    }

    let mut indexed = recorded
        .into_iter()
        .flat_map(|provider| {
            provider
                .contribution
                .items
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .into_iter()
                .enumerate()
                .map(move |(source_index, item)| {
                    (
                        provider.provider_index,
                        source_index,
                        provider.component.clone(),
                        item,
                    )
                })
        })
        .collect::<Vec<_>>();
    indexed.sort_by_key(|(provider_index, source_index, _, item)| {
        let authority_group = u8::from(
            !(item.kind == ContextItemKind::Instruction
                && item.authority == ContextAuthority::TrustedApplication),
        );
        (
            authority_group,
            *provider_index,
            core::cmp::Reverse(item.priority),
            *source_index,
        )
    });

    let mut accepted = Vec::new();
    let mut diagnostics = Vec::new();
    let mut tokens = 0_u64;
    let mut bytes = 0_u64;
    for (_, _, component, item) in indexed {
        item.validate()?;
        let next_items = accepted.len().saturating_add(1);
        let next_tokens = tokens.checked_add(item.estimated_tokens);
        let next_bytes = bytes.checked_add(item.bytes);
        let fits = next_items <= budget.max_items
            && next_tokens.is_some_and(|value| value <= budget.max_tokens)
            && next_bytes.is_some_and(|value| value <= budget.max_bytes);
        if fits {
            tokens += item.estimated_tokens;
            bytes += item.bytes;
            accepted.push(item);
            continue;
        }
        if budget.overflow == ContextOverflowPolicy::Reject {
            return Err(ContextError::stable(
                CONTEXT_BUDGET_EXCEEDED,
                "assembled context exceeds its explicit budget",
            ));
        }
        if let Some(existing) = diagnostics
            .iter_mut()
            .find(|diagnostic: &&mut ContextTruncationDiagnostic| diagnostic.component == component)
        {
            existing.dropped_items = existing.dropped_items.saturating_add(1);
            existing.dropped_tokens = existing
                .dropped_tokens
                .saturating_add(item.estimated_tokens);
            existing.dropped_bytes = existing.dropped_bytes.saturating_add(item.bytes);
        } else {
            diagnostics.push(ContextTruncationDiagnostic {
                component,
                dropped_items: 1,
                dropped_tokens: item.estimated_tokens,
                dropped_bytes: item.bytes,
            });
        }
    }
    Ok(AssembledContext {
        items: accepted.into(),
        diagnostics: diagnostics.into(),
        estimated_tokens: tokens,
        bytes,
    })
}

/// Stable context-port error.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{code}: {message}")]
pub struct ContextError {
    code: ErrorCode,
    category: ErrorCategory,
    message: Arc<str>,
    metadata: Metadata,
}

impl ContextError {
    /// Construct a bounded provider-specific context error.
    ///
    /// # Errors
    ///
    /// Returns `context_contribution_invalid` when the code or message is invalid.
    pub fn try_new(
        code: impl AsRef<str>,
        category: ErrorCategory,
        message: impl AsRef<str>,
        metadata: Metadata,
    ) -> Result<Self, Self> {
        let code = ErrorCode::new(code).map_err(|_| {
            Self::stable(
                CONTEXT_CONTRIBUTION_INVALID,
                "context error code is invalid",
            )
        })?;
        let message = message.as_ref();
        if message.is_empty() || message.len() > 1_048_576 || message.as_bytes().contains(&0) {
            return Err(Self::stable(
                CONTEXT_CONTRIBUTION_INVALID,
                "context error message is invalid",
            ));
        }
        Ok(Self {
            code,
            category,
            message: Arc::from(message),
            metadata,
        })
    }

    fn stable(code: &'static str, message: &'static str) -> Self {
        Self {
            code: ErrorCode::new(code).expect("frozen context error code is valid"),
            category: if code == CONTEXT_BUDGET_EXCEEDED {
                ErrorCategory::Limit
            } else {
                ErrorCategory::Validation
            },
            message: Arc::from(message),
            metadata: Metadata::empty(),
        }
    }

    fn commit_required() -> Self {
        Self::stable(
            CONTEXT_COMMIT_REQUIRED,
            "context invocation is not backed by the exact committed effect",
        )
    }

    /// Stable error code.
    #[must_use]
    pub fn code(&self) -> &str {
        self.code.as_str()
    }

    /// Safe error descriptor suitable for durable failure records.
    #[must_use]
    pub fn descriptor(&self) -> ErrorDescriptor {
        ErrorDescriptor {
            code: self.code.clone(),
            message: Arc::clone(&self.message),
            category: self.category,
            retryable: false,
            identifiers: finstack_ai_kernel::ErrorIdentifiers::default(),
            safe_details: self.metadata.clone(),
        }
    }
}

fn normalize_instruction_authority(
    contribution: &mut ContextContribution,
    trusted_instructions: bool,
) {
    let items = contribution
        .items
        .iter()
        .cloned()
        .map(|mut item| {
            if item.kind == ContextItemKind::Instruction {
                if trusted_instructions && !item.provenance.external {
                    item.authority = ContextAuthority::TrustedApplication;
                } else {
                    item.kind = ContextItemKind::QuotedSource;
                    item.authority = ContextAuthority::Untrusted;
                }
            }
            item
        })
        .collect::<Vec<_>>();
    contribution.items = items.into();
}

fn contribution_fits_budget(contribution: &ContextContribution, budget: ContextBudget) -> bool {
    contribution.items.len() <= budget.max_items
        && contribution.estimated_tokens <= budget.max_tokens
        && contribution.bytes <= budget.max_bytes
}

fn canonical_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, ContextError> {
    serde_json_canonicalizer::to_vec(value).map_err(|_| {
        ContextError::stable(
            CONTEXT_CONTRIBUTION_INVALID,
            "context value could not be canonicalized",
        )
    })
}

fn validate_label(value: &str, field: &'static str) -> Result<(), ContextError> {
    if value.is_empty() || value.len() > 256 || value.as_bytes().contains(&0) {
        return Err(ContextError::stable(CONTEXT_CONTRIBUTION_INVALID, field));
    }
    Ok(())
}

fn validate_text(value: &str, field: &'static str) -> Result<(), ContextError> {
    if value.is_empty() || value.len() > 1_048_576 || value.as_bytes().contains(&0) {
        return Err(ContextError::stable(CONTEXT_CONTRIBUTION_INVALID, field));
    }
    Ok(())
}

#[cfg(all(test, feature = "native-tokio"))]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use finstack_ai_kernel::{
        EffectOutputContract, EventTag, Id, IdTag, LaneTag, MessageRole, PrincipalRef, ProviderIds,
        RECORD_FORMAT_VERSION, RECORD_KIND_VERSION, RecordTag, RunTag, SessionTag, TextBlock,
        Timestamp, Version,
    };

    use crate::{AuthorizationContext, CancellationSignal};

    fn id<T: IdTag>(value: u64) -> Id<T> {
        let mut bytes = [0_u8; 16];
        bytes[6] = 0x70;
        bytes[8] = 0x80;
        bytes[9..].copy_from_slice(&value.to_be_bytes()[1..]);
        Id::from_bytes(bytes)
    }

    fn item(priority: i32, source: &str, authority: ContextAuthority) -> ContextItem {
        ContextItem::try_new(
            ContextItemKind::QuotedSource,
            vec![ContentBlock::Text(
                TextBlock::try_new(source).expect("text"),
            )],
            ContextProvenance {
                source_id: Arc::from(source),
                source_ref: None,
                external: true,
            },
            authority,
            priority,
            1,
            Sensitivity::Internal,
            false,
        )
        .expect("item")
    }

    fn message(ordinal: u64, role: MessageRole, text: &str) -> Message {
        Message::try_new(
            id(ordinal),
            role,
            vec![ContentBlock::Text(TextBlock::try_new(text).expect("text"))],
            Timestamp::from_unix_ms(i64::try_from(ordinal).expect("timestamp")).expect("timestamp"),
            None,
            ProviderIds::empty(),
            Metadata::empty(),
        )
        .expect("message")
    }

    #[test]
    fn explicit_truncation_is_deterministic_and_diagnostic() {
        let contribution = ContextContribution::try_new(
            vec![
                item(1, "low", ContextAuthority::Untrusted),
                item(10, "high", ContextAuthority::Untrusted),
            ],
            None::<&str>,
        )
        .expect("contribution");
        let result = assemble_context(
            vec![RecordedContextContribution {
                component: ComponentId::parse("fixture.context").expect("component"),
                provider_index: 0,
                contribution,
            }],
            ContextBudget {
                max_items: 1,
                max_tokens: 2,
                max_bytes: u64::MAX,
                overflow: ContextOverflowPolicy::TruncateWithDiagnostic,
            },
        )
        .expect("assembly");
        assert_eq!(result.items.len(), 1);
        assert_eq!(result.items[0].priority, 10);
        assert_eq!(result.diagnostics[0].dropped_items, 1);
    }

    #[test]
    fn provider_finish_order_cannot_change_assembly_and_chain_gaps_fail() {
        let first = RecordedContextContribution {
            component: ComponentId::parse("fixture.context-first").expect("component"),
            provider_index: 0,
            contribution: ContextContribution::try_new(
                vec![item(1, "first", ContextAuthority::Untrusted)],
                None::<&str>,
            )
            .expect("contribution"),
        };
        let second = RecordedContextContribution {
            component: ComponentId::parse("fixture.context-second").expect("component"),
            provider_index: 1,
            contribution: ContextContribution::try_new(
                vec![item(100, "second", ContextAuthority::Untrusted)],
                None::<&str>,
            )
            .expect("contribution"),
        };
        let budget = ContextBudget {
            max_items: 8,
            max_tokens: 128,
            max_bytes: u64::MAX,
            overflow: ContextOverflowPolicy::Reject,
        };
        let completion_order = assemble_context(vec![second.clone(), first.clone()], budget)
            .expect("completion order");
        let declaration_order =
            assemble_context(vec![first.clone(), second.clone()], budget).expect("declaration");
        assert_eq!(completion_order, declaration_order);
        assert_eq!(
            completion_order.items[0].provenance.source_id.as_ref(),
            "first"
        );

        let mut gap = second;
        gap.provider_index = 2;
        assert_eq!(
            assemble_context(vec![first, gap], budget)
                .expect_err("chain gap")
                .code(),
            CONTEXT_CONFIGURATION_INVALID
        );
    }

    #[test]
    fn authority_projection_has_exact_fixed_groups_and_current_user_once_last() {
        let capability = ContextItem::try_new(
            ContextItemKind::Instruction,
            vec![ContentBlock::Text(
                TextBlock::try_new("capability").expect("text"),
            )],
            ContextProvenance {
                source_id: Arc::from("capability"),
                source_ref: None,
                external: false,
            },
            ContextAuthority::TrustedApplication,
            0,
            1,
            Sensitivity::Internal,
            true,
        )
        .expect("capability");
        let mut reminder = item(0, "reminder", ContextAuthority::TrustedApplication);
        reminder.kind = ContextItemKind::Instruction;
        reminder.provenance.external = false;
        let providers = AssembledContext {
            items: Arc::from([reminder, item(0, "external", ContextAuthority::Untrusted)]),
            diagnostics: Arc::from([]),
            estimated_tokens: 2,
            bytes: 0,
        };
        let projection = assemble_context_projection(ContextProjectionInput {
            system: Arc::from([message(1, MessageRole::System, "system")]),
            developer: Arc::from([message(2, MessageRole::Developer, "developer")]),
            capabilities: Arc::from([CapabilityContext {
                capability_id: CapabilityId::parse("fixture.capability").expect("capability id"),
                instructions: Arc::from([capability]),
            }]),
            providers,
            history: Arc::from([message(3, MessageRole::Assistant, "history")]),
            current_user: message(4, MessageRole::User, "current"),
        })
        .expect("projection");
        let sources = projection
            .iter()
            .map(|item| match item {
                ContextProjectionItem::Message { source, .. }
                | ContextProjectionItem::Context { source, .. } => *source,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            sources,
            vec![
                ContextProjectionSource::System,
                ContextProjectionSource::Developer,
                ContextProjectionSource::Capability,
                ContextProjectionSource::ProviderReminder,
                ContextProjectionSource::ExternalContext,
                ContextProjectionSource::History,
                ContextProjectionSource::CurrentUser,
            ]
        );
    }

    #[cfg(feature = "native-tokio")]
    struct FixtureProvider {
        descriptor: ContextProviderDescriptor,
        calls: Arc<AtomicUsize>,
    }

    #[cfg(feature = "native-tokio")]
    impl ContextProvider for FixtureProvider {
        fn descriptor(&self) -> ContextProviderDescriptor {
            self.descriptor.clone()
        }

        fn collect(
            &self,
            _ctx: ContextCallContext,
            _request: ContextRequest,
        ) -> PortFuture<Result<ContextContribution, ContextError>> {
            self.calls.fetch_add(1, Ordering::AcqRel);
            let contribution = ContextContribution::try_new(
                vec![
                    ContextItem::try_new(
                        ContextItemKind::Instruction,
                        vec![ContentBlock::Text(
                            TextBlock::try_new("retrieved system override").expect("text"),
                        )],
                        ContextProvenance {
                            source_id: Arc::from("fixture-source"),
                            source_ref: None,
                            external: true,
                        },
                        ContextAuthority::TrustedApplication,
                        0,
                        4,
                        Sensitivity::Internal,
                        false,
                    )
                    .expect("item"),
                ],
                None::<&str>,
            )
            .expect("contribution");
            Box::pin(async move { Ok(contribution) })
        }
    }

    #[cfg(feature = "native-tokio")]
    fn descriptor(recovery: InvocationRecovery) -> ContextProviderDescriptor {
        ContextProviderDescriptor {
            invocation: ComponentInvocation {
                component: ComponentId::parse("fixture.context").expect("component"),
                version: Version {
                    major: 1,
                    minor: 0,
                    patch: 0,
                },
                configuration_digest: Digest::raw_json(b"{}"),
                recovery,
            },
            trusted_application_instructions: false,
            metadata: Metadata::empty(),
        }
    }

    #[cfg(feature = "native-tokio")]
    fn request() -> ContextRequest {
        ContextRequest {
            session_id: id::<SessionTag>(1),
            lane_id: id::<LaneTag>(2),
            run_id: id::<RunTag>(3),
            user_input: Arc::from([]),
            recent_history: Arc::from([]),
            budget: ContextBudget {
                max_items: 8,
                max_tokens: 128,
                max_bytes: 16_384,
                overflow: ContextOverflowPolicy::Reject,
            },
            active_capabilities: Arc::from([]),
        }
    }

    #[cfg(feature = "native-tokio")]
    fn run(effect_id: finstack_ai_kernel::EffectId) -> RunCallContext {
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

    #[cfg(feature = "native-tokio")]
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

    #[cfg(feature = "native-tokio")]
    fn effect_request(
        effect_id: finstack_ai_kernel::EffectId,
        descriptor: &ContextProviderDescriptor,
        request: &ContextRequest,
    ) -> EffectRequested {
        EffectRequested::try_new(
            effect_id,
            EffectKind::Context,
            None,
            Some(descriptor.invocation.clone()),
            Some(
                PipelinePosition::try_new(Digest::raw_json(b"chain"), CONTEXT_STAGE, 0)
                    .expect("pipeline"),
            ),
            EffectOutputContract {
                kind: EffectOutputKind::ContextContribution,
                schema_version: 1,
                schema_digest: Digest::raw_json(b"context-contribution-v1"),
            },
            EffectInput::Context {
                request: request.to_raw_json().expect("request"),
            },
            RetrySafety::SafeToRetry,
            None,
        )
        .expect("effect")
    }

    #[cfg(feature = "native-tokio")]
    #[tokio::test]
    async fn committed_guard_precedes_provider_io_and_untrusted_instruction_is_downgraded() {
        let descriptor = descriptor(InvocationRecovery::RecomputeSafe);
        let request = request();
        let effect_id = id::<finstack_ai_kernel::EffectTag>(4);
        let requested = effect_request(effect_id, &descriptor, &request);
        let committed = envelope(1, RecordBody::EffectRequested(requested.clone()));
        let calls = Arc::new(AtomicUsize::new(0));
        let provider = FixtureProvider {
            descriptor: descriptor.clone(),
            calls: Arc::clone(&calls),
        };
        let context = ContextCallContext {
            run: run(effect_id),
            provider_index: 0,
            chain_digest: Digest::raw_json(b"chain"),
        };
        let wrong = ContextCallContext {
            run: run(id::<finstack_ai_kernel::EffectTag>(99)),
            ..context.clone()
        };
        assert_eq!(
            CommittedContextCall::try_new(&committed, wrong, request.clone(), &descriptor)
                .expect_err("mismatch")
                .code(),
            CONTEXT_COMMIT_REQUIRED
        );
        assert_eq!(calls.load(Ordering::Acquire), 0);

        let contribution =
            CommittedContextCall::try_new(&committed, context, request.clone(), &descriptor)
                .expect("guard")
                .invoke(&provider)
                .await
                .expect("invoke");
        assert_eq!(calls.load(Ordering::Acquire), 1);
        assert_eq!(contribution.items[0].kind, ContextItemKind::QuotedSource);
        assert_eq!(contribution.items[0].authority, ContextAuthority::Untrusted);

        let completed = EffectCompleted::try_new(
            effect_id,
            requested.output_contract().clone(),
            contribution.to_raw_json().expect("output"),
            None,
            Vec::new(),
            finstack_ai_kernel::ProviderIds::empty(),
            None::<&str>,
            None,
        )
        .expect("completion");
        let completed_envelope = envelope(2, RecordBody::EffectCompleted(completed.clone()));
        let recorded =
            RecordedContextContribution::try_from_records(&committed, &completed_envelope)
                .expect("recorded");
        assert_eq!(recorded.provider_index, 0);
        assert_eq!(
            context_resume_action(&requested, Some(&completed)),
            InvocationResumeAction::UseRecorded
        );
        assert_eq!(
            context_resume_action(&requested, None),
            InvocationResumeAction::Recompute
        );
    }
}
