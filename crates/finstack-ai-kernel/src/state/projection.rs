//! Exact typed nested fingerprint projections with recursive explicit nulls.

use core::marker::PhantomData;
use std::collections::BTreeMap;

use serde::Serialize;
use serde::ser::SerializeSeq;

use crate::content::{
    BlobRef, ContentBlock, MediaRef, OpaquePayload, ToolCallBlock, ToolResultBlock,
};
use crate::conversation::{Message, MessageRole, ModelRef, ProviderIds, ThinkingLevel};
use crate::effects::{
    ComponentInvocation, EffectCompleted, EffectDeferred, EffectFailed, EffectOutputContract,
    RetrySafety,
};
use crate::primitives::Digest;
use crate::primitives::Timestamp;
use crate::primitives::{
    AppendBatchId, ArtifactId, BudgetReservationId, BudgetScopeId, CancellationRequestId, EffectId,
    EventId, InteractionId, LaneId, LimitKey, MessageId, ModelRequestId, RecordId, RunId,
    SessionId, ToolBatchId, ToolCallId, ToolId, TurnId,
};
use crate::primitives::{ArtifactRef, CostAmount, ExternalHandleRef, Usage};
use crate::primitives::{ErrorCategory, ErrorCode, ErrorDescriptor, ErrorIdentifiers};
use crate::primitives::{Metadata, RawJson};
use crate::records::tools::{
    AssignedToolCall, SyntheticToolClosure, ToolBatchOutcome, ToolCallPlan, ToolExecutionMode,
    ToolFailurePolicy, ValidatedToolCall,
};

/// Serializes a borrowed slice through a per-element projection, lazily.
///
/// Collecting projections into a `Vec` allocated once per collection per
/// digest; every message in every fingerprint and state hash paid it. The
/// emitted sequence is identical either way.
pub(crate) struct ProjectedSeq<'a, T, P> {
    items: &'a [T],
    _marker: PhantomData<fn() -> P>,
}

impl<'a, T, P> ProjectedSeq<'a, T, P> {
    pub(crate) const fn new(items: &'a [T]) -> Self {
        Self {
            items,
            _marker: PhantomData,
        }
    }
}

impl<'a, T, P> Serialize for ProjectedSeq<'a, T, P>
where
    P: From<&'a T> + Serialize,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut sequence = serializer.serialize_seq(Some(self.items.len()))?;
        for item in self.items {
            sequence.serialize_element(&P::from(item))?;
        }
        sequence.end()
    }
}

/// Lazily projected content-block sequence.
pub(crate) type ContentSeq<'a> = ProjectedSeq<'a, ContentBlock, ContentProjection<'a>>;
/// Lazily projected artifact sequence.
pub(crate) type ArtifactSeq<'a> = ProjectedSeq<'a, ArtifactRef, ArtifactProjection<'a>>;
/// Lazily projected message sequence.
pub(crate) type MessageSeq<'a> = ProjectedSeq<'a, Message, MessageProjection<'a>>;

#[derive(Serialize)]
pub(crate) struct EffectCompletedProjection<'a> {
    effect_id: EffectId,
    output_contract: &'a EffectOutputContract,
    output: &'a RawJson,
    output_digest: Digest,
    usage: Option<UsageProjection<'a>>,
    usage_digest: Option<Digest>,
    artifacts: ArtifactSeq<'a>,
    provider_ids: ProviderIdsProjection<'a>,
    completion_id: Option<&'a str>,
    reservation_id: Option<BudgetReservationId>,
}

impl<'a> From<&'a EffectCompleted> for EffectCompletedProjection<'a> {
    fn from(value: &'a EffectCompleted) -> Self {
        Self {
            effect_id: value.effect_id(),
            output_contract: value.output_contract(),
            output: value.output(),
            output_digest: value.output_digest(),
            usage: value.usage().map(UsageProjection::from),
            usage_digest: value.usage_digest(),
            artifacts: ArtifactSeq::new(value.artifacts()),
            provider_ids: ProviderIdsProjection::from(value.provider_ids()),
            completion_id: value.completion_id(),
            reservation_id: value.reservation_id(),
        }
    }
}

#[derive(Serialize)]
pub(crate) struct EffectFailedProjection<'a> {
    effect_id: EffectId,
    output_contract: &'a EffectOutputContract,
    error: ErrorProjection<'a>,
    usage: Option<UsageProjection<'a>>,
    usage_digest: Option<Digest>,
    completion_id: Option<&'a str>,
}

impl<'a> From<&'a EffectFailed> for EffectFailedProjection<'a> {
    fn from(value: &'a EffectFailed) -> Self {
        Self {
            effect_id: value.effect_id(),
            output_contract: value.output_contract(),
            error: ErrorProjection::from(value.error()),
            usage: value.usage().map(UsageProjection::from),
            usage_digest: value.usage_digest(),
            completion_id: value.completion_id(),
        }
    }
}

#[derive(Serialize)]
pub(crate) struct EffectDeferredProjection<'a> {
    effect_id: EffectId,
    handle: &'a ExternalHandleRef,
    reconciliation: crate::effects::ReconciliationPolicy,
    next_poll_at: Option<Timestamp>,
    expires_at: Option<Timestamp>,
    output_contract: &'a EffectOutputContract,
}

impl<'a> From<&'a EffectDeferred> for EffectDeferredProjection<'a> {
    fn from(value: &'a EffectDeferred) -> Self {
        Self {
            effect_id: value.effect_id,
            handle: &value.handle,
            reconciliation: value.reconciliation,
            next_poll_at: value.next_poll_at,
            expires_at: value.expires_at,
            output_contract: &value.output_contract,
        }
    }
}

#[derive(Serialize)]
pub(crate) struct MessageProjection<'a> {
    id: MessageId,
    role: MessageRole,
    content: ContentSeq<'a>,
    created_at: Timestamp,
    model: Option<ModelRefProjection<'a>>,
    provider_ids: ProviderIdsProjection<'a>,
    metadata: &'a Metadata,
}

impl<'a> From<&'a Message> for MessageProjection<'a> {
    fn from(value: &'a Message) -> Self {
        Self {
            id: *value.id(),
            role: value.role(),
            content: ContentSeq::new(value.content()),
            created_at: value.created_at(),
            model: value.model().map(ModelRefProjection::from),
            provider_ids: ProviderIdsProjection::from(value.provider_ids()),
            metadata: value.metadata(),
        }
    }
}

#[derive(Serialize)]
pub(crate) struct ModelRefProjection<'a> {
    provider: &'a str,
    model: &'a str,
    thinking_level: Option<ThinkingLevel>,
    context_length: Option<u64>,
    fast: Option<bool>,
}

impl<'a> From<&'a ModelRef> for ModelRefProjection<'a> {
    fn from(value: &'a ModelRef) -> Self {
        Self {
            provider: value.provider(),
            model: value.model(),
            thinking_level: value.thinking_level(),
            context_length: value.context_length(),
            fast: value.fast(),
        }
    }
}

#[derive(Serialize)]
pub(crate) struct ProviderIdsProjection<'a> {
    #[serde(rename = "request_id")]
    request: Option<&'a str>,
    #[serde(rename = "response_id")]
    response: Option<&'a str>,
    #[serde(rename = "continuation_id")]
    continuation: Option<&'a str>,
}

impl<'a> From<&'a ProviderIds> for ProviderIdsProjection<'a> {
    fn from(value: &'a ProviderIds) -> Self {
        Self {
            request: value.request_id(),
            response: value.response_id(),
            continuation: value.continuation_id(),
        }
    }
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum ContentProjection<'a> {
    Text {
        text: &'a str,
    },
    Json {
        value: &'a RawJson,
    },
    Image {
        blob: BlobProjection<'a>,
    },
    Audio {
        blob: BlobProjection<'a>,
    },
    File {
        blob: BlobProjection<'a>,
    },
    ToolCall {
        tool_call_id: ToolCallId,
        tool_name: &'a str,
        arguments: &'a RawJson,
        provider_call_id: Option<&'a str>,
    },
    ToolResult {
        tool_call_id: ToolCallId,
        content: ContentSeq<'a>,
        is_error: bool,
    },
    Opaque {
        media_type: &'a str,
        payload: &'a OpaquePayload,
    },
}

impl<'a> From<&'a ContentBlock> for ContentProjection<'a> {
    fn from(value: &'a ContentBlock) -> Self {
        match value {
            ContentBlock::Text(block) => Self::Text { text: block.text() },
            ContentBlock::Json(block) => Self::Json {
                value: block.value(),
            },
            ContentBlock::Image(media) => Self::Image {
                blob: media_projection(media),
            },
            ContentBlock::Audio(media) => Self::Audio {
                blob: media_projection(media),
            },
            ContentBlock::File(media) => Self::File {
                blob: media_projection(media),
            },
            ContentBlock::ToolCall(block) => tool_call_projection(block),
            ContentBlock::ToolResult(block) => tool_result_projection(block),
            ContentBlock::Opaque(block) => Self::Opaque {
                media_type: block.media_type(),
                payload: block.payload(),
            },
        }
    }
}

fn media_projection(media: &MediaRef) -> BlobProjection<'_> {
    BlobProjection::from(media.blob())
}

fn tool_call_projection(block: &ToolCallBlock) -> ContentProjection<'_> {
    ContentProjection::ToolCall {
        tool_call_id: *block.tool_call_id(),
        tool_name: block.tool_name(),
        arguments: block.arguments(),
        provider_call_id: block.provider_call_id(),
    }
}

fn tool_result_projection(block: &ToolResultBlock) -> ContentProjection<'_> {
    let ToolResultProjection {
        tool_call_id,
        content,
        is_error,
    } = ToolResultProjection::from(block);
    ContentProjection::ToolResult {
        tool_call_id,
        content,
        is_error,
    }
}

/// Shared tool-result projection for fingerprints and state hashes.
#[derive(Serialize)]
pub(crate) struct ToolResultProjection<'a> {
    pub(crate) tool_call_id: ToolCallId,
    pub(crate) content: ContentSeq<'a>,
    pub(crate) is_error: bool,
}

impl<'a> From<&'a ToolResultBlock> for ToolResultProjection<'a> {
    fn from(value: &'a ToolResultBlock) -> Self {
        Self {
            tool_call_id: *value.tool_call_id(),
            content: ContentSeq::new(value.content()),
            is_error: value.is_error(),
        }
    }
}

/// Shared assigned-call projection for fingerprints and state hashes.
#[derive(Serialize)]
pub(crate) struct AssignedToolCallProjection<'a> {
    pub(crate) source_index: u32,
    pub(crate) group_index: u32,
    pub(crate) effect_id: EffectId,
    pub(crate) plan: ToolCallPlanProjection<'a>,
}

impl<'a> From<&'a AssignedToolCall> for AssignedToolCallProjection<'a> {
    fn from(value: &'a AssignedToolCall) -> Self {
        Self {
            source_index: value.source_index,
            group_index: value.group_index,
            effect_id: value.effect_id,
            plan: ToolCallPlanProjection::from(&value.plan),
        }
    }
}

/// Shared tool-plan projection for fingerprints and state hashes.
#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ToolCallPlanProjection<'a> {
    Execute(ValidatedToolCallProjection<'a>),
    SyntheticClosure(Box<SyntheticToolClosureProjection<'a>>),
}

impl<'a> From<&'a ToolCallPlan> for ToolCallPlanProjection<'a> {
    fn from(value: &'a ToolCallPlan) -> Self {
        match value {
            ToolCallPlan::Execute(call) => Self::Execute(ValidatedToolCallProjection::from(call)),
            ToolCallPlan::SyntheticClosure(closure) => {
                Self::SyntheticClosure(Box::new(SyntheticToolClosureProjection::from(closure)))
            }
        }
    }
}

#[derive(Serialize)]
pub(crate) struct ValidatedToolCallProjection<'a> {
    pub(crate) call: &'a ToolCallBlock,
    pub(crate) tool_id: ToolId,
    pub(crate) component: Option<&'a ComponentInvocation>,
    pub(crate) output_contract: &'a EffectOutputContract,
    pub(crate) retry_safety: RetrySafety,
    pub(crate) deadline: Option<Timestamp>,
    pub(crate) execution: ToolExecutionMode,
    pub(crate) failure_policy: ToolFailurePolicy,
}

impl<'a> From<&'a ValidatedToolCall> for ValidatedToolCallProjection<'a> {
    fn from(value: &'a ValidatedToolCall) -> Self {
        Self {
            call: &value.call,
            tool_id: value.tool_id.clone(),
            component: value.component.as_ref(),
            output_contract: &value.output_contract,
            retry_safety: value.retry_safety,
            deadline: value.deadline,
            execution: value.execution,
            failure_policy: value.failure_policy,
        }
    }
}

#[derive(Serialize)]
pub(crate) struct SyntheticToolClosureProjection<'a> {
    pub(crate) call: &'a ToolCallBlock,
    pub(crate) execution: ToolExecutionMode,
    pub(crate) failure_policy: ToolFailurePolicy,
    pub(crate) error: ErrorProjection<'a>,
}

impl<'a> From<&'a SyntheticToolClosure> for SyntheticToolClosureProjection<'a> {
    fn from(value: &'a SyntheticToolClosure) -> Self {
        Self {
            call: &value.call,
            execution: value.execution,
            failure_policy: value.failure_policy,
            error: ErrorProjection::from(&value.error),
        }
    }
}

/// Shared batch-outcome projection for fingerprints and state hashes.
#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ToolBatchOutcomeProjection<'a> {
    ContinueModel,
    Finalize,
    Failed { error: Box<ErrorProjection<'a>> },
}

impl<'a> From<&'a ToolBatchOutcome> for ToolBatchOutcomeProjection<'a> {
    fn from(value: &'a ToolBatchOutcome) -> Self {
        match value {
            ToolBatchOutcome::ContinueModel => Self::ContinueModel,
            ToolBatchOutcome::Finalize => Self::Finalize,
            ToolBatchOutcome::Failed { error } => Self::Failed {
                error: Box::new(ErrorProjection::from(error)),
            },
        }
    }
}

#[derive(Serialize)]
pub(crate) struct BlobProjection<'a> {
    id: &'a str,
    media_type: &'a str,
    length: u64,
    digest: Option<Digest>,
    name: Option<&'a str>,
}

impl<'a> From<&'a BlobRef> for BlobProjection<'a> {
    fn from(value: &'a BlobRef) -> Self {
        Self {
            id: value.id(),
            media_type: value.media_type(),
            length: value.length(),
            digest: value.digest().copied(),
            name: value.name(),
        }
    }
}

#[derive(Serialize)]
pub(crate) struct ErrorProjection<'a> {
    code: &'a ErrorCode,
    message: &'a str,
    category: ErrorCategory,
    retryable: bool,
    identifiers: ErrorIdentifiersProjection,
    safe_details: &'a Metadata,
}

impl<'a> From<&'a ErrorDescriptor> for ErrorProjection<'a> {
    fn from(value: &'a ErrorDescriptor) -> Self {
        Self {
            code: &value.code,
            message: &value.message,
            category: value.category,
            retryable: value.retryable,
            identifiers: ErrorIdentifiersProjection::from(&value.identifiers),
            safe_details: &value.safe_details,
        }
    }
}

#[derive(Serialize)]
pub(crate) struct ErrorIdentifiersProjection {
    #[serde(rename = "session_id")]
    session: Option<SessionId>,
    #[serde(rename = "lane_id")]
    lane: Option<LaneId>,
    #[serde(rename = "run_id")]
    run: Option<RunId>,
    #[serde(rename = "turn_id")]
    turn: Option<TurnId>,
    #[serde(rename = "message_id")]
    message: Option<MessageId>,
    #[serde(rename = "model_request_id")]
    model_request: Option<ModelRequestId>,
    #[serde(rename = "tool_batch_id")]
    tool_batch: Option<ToolBatchId>,
    #[serde(rename = "tool_call_id")]
    tool_call: Option<ToolCallId>,
    #[serde(rename = "effect_id")]
    effect: Option<EffectId>,
    #[serde(rename = "interaction_id")]
    interaction: Option<InteractionId>,
    #[serde(rename = "event_id")]
    event: Option<EventId>,
    #[serde(rename = "budget_scope_id")]
    budget_scope: Option<BudgetScopeId>,
    #[serde(rename = "budget_reservation_id")]
    budget_reservation: Option<BudgetReservationId>,
    #[serde(rename = "cancellation_request_id")]
    cancellation_request: Option<CancellationRequestId>,
    #[serde(rename = "record_id")]
    record: Option<RecordId>,
    #[serde(rename = "append_batch_id")]
    append_batch: Option<AppendBatchId>,
    #[serde(rename = "artifact_id")]
    artifact: Option<ArtifactId>,
}

impl From<&ErrorIdentifiers> for ErrorIdentifiersProjection {
    fn from(value: &ErrorIdentifiers) -> Self {
        Self {
            session: value.session_id,
            lane: value.lane_id,
            run: value.run_id,
            turn: value.turn_id,
            message: value.message_id,
            model_request: value.model_request_id,
            tool_batch: value.tool_batch_id,
            tool_call: value.tool_call_id,
            effect: value.effect_id,
            interaction: value.interaction_id,
            event: value.event_id,
            budget_scope: value.budget_scope_id,
            budget_reservation: value.budget_reservation_id,
            cancellation_request: value.cancellation_request_id,
            record: value.record_id,
            append_batch: value.append_batch_id,
            artifact: value.artifact_id,
        }
    }
}

#[derive(Serialize)]
pub(crate) struct UsageProjection<'a> {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    total_tokens: Option<u64>,
    cost: Option<&'a CostAmount>,
    extension_counters: &'a BTreeMap<LimitKey, u64>,
}

impl<'a> From<&'a Usage> for UsageProjection<'a> {
    fn from(value: &'a Usage) -> Self {
        Self {
            input_tokens: value.input_tokens(),
            output_tokens: value.output_tokens(),
            total_tokens: value.total_tokens(),
            cost: value.cost(),
            extension_counters: value.extension_counters(),
        }
    }
}

#[derive(Serialize)]
pub(crate) struct ArtifactProjection<'a> {
    id: ArtifactId,
    kind: &'a str,
    blob: BlobProjection<'a>,
    content_digest: Digest,
    scope_digest: Digest,
    metadata: &'a Metadata,
}

impl<'a> From<&'a ArtifactRef> for ArtifactProjection<'a> {
    fn from(value: &'a ArtifactRef) -> Self {
        Self {
            id: value.id(),
            kind: value.kind(),
            blob: BlobProjection::from(value.blob()),
            content_digest: value.content_digest(),
            scope_digest: value.scope_digest(),
            metadata: value.metadata(),
        }
    }
}
