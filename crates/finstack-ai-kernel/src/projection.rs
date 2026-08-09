//! Exact typed schema-1 nested fingerprint projections.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::content::{
    BlobRef, ContentBlock, MediaRef, OpaquePayload, ToolCallBlock, ToolResultBlock,
};
use crate::digest::Digest;
use crate::effects::{EffectCompleted, EffectDeferred, EffectFailed, EffectOutputContract};
use crate::error::{ErrorCategory, ErrorCode, ErrorDescriptor, ErrorIdentifiers};
use crate::ids::{
    AppendBatchId, ArtifactId, BudgetReservationId, BudgetScopeId, CancellationRequestId, EffectId,
    EventId, InteractionId, LaneId, LimitKey, MessageId, ModelRequestId, RecordId, RunId,
    SessionId, ToolBatchId, ToolCallId, TurnId,
};
use crate::message::{Message, MessageRole, ModelRef, ProviderIds, ThinkingLevel};
use crate::raw_json::{Metadata, RawJson};
use crate::refs::{ArtifactRef, CostAmount, ExternalHandleRef, Usage};
use crate::time::Timestamp;

#[derive(Serialize)]
pub(crate) struct EffectCompletedProjection<'a> {
    effect_id: EffectId,
    output_contract: &'a EffectOutputContract,
    output: &'a RawJson,
    output_digest: Digest,
    usage: Option<UsageProjection<'a>>,
    usage_digest: Option<Digest>,
    artifacts: Vec<ArtifactProjection<'a>>,
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
            artifacts: value
                .artifacts()
                .iter()
                .map(ArtifactProjection::from)
                .collect(),
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
    content: Vec<ContentProjection<'a>>,
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
            content: value
                .content()
                .iter()
                .map(ContentProjection::from)
                .collect(),
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
    },
    ToolResult {
        tool_call_id: ToolCallId,
        content: Vec<ContentProjection<'a>>,
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
    }
}

fn tool_result_projection(block: &ToolResultBlock) -> ContentProjection<'_> {
    ContentProjection::ToolResult {
        tool_call_id: *block.tool_call_id(),
        content: block
            .content()
            .iter()
            .map(ContentProjection::from)
            .collect(),
        is_error: block.is_error(),
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
