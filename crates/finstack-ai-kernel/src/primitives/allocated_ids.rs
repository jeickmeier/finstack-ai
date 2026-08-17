//! Preallocated identifier bags for a single transition.

use serde::de;
use serde::{Deserialize, Serialize};

use crate::primitives::{
    AppendBatchId, CancellationRequestId, EffectId, EventId, InteractionId, MessageId,
    ModelRequestId, RecordId, ToolBatchId, ToolCallId, TurnId,
};
use crate::primitives::{BoundedVec, SEMANTIC_ARRAY_MAX_ITEMS};

use super::refs_error::RefsError;

/// Runtime-owned preallocated `UUIDv7` bags for a transition.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
#[allow(clippy::struct_field_names)] // Frozen contract fields intentionally end in `_ids`.
pub struct AllocatedIds {
    /// Record ids.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) record_ids: Vec<RecordId>,
    /// Event ids.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) event_ids: Vec<EventId>,
    /// Effect ids.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) effect_ids: Vec<EffectId>,
    /// Interaction ids.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) interaction_ids: Vec<InteractionId>,
    /// Message ids.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) message_ids: Vec<MessageId>,
    /// Turn ids.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) turn_ids: Vec<TurnId>,
    /// Model request ids.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) model_request_ids: Vec<ModelRequestId>,
    /// Tool batch ids.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) tool_batch_ids: Vec<ToolBatchId>,
    /// Tool call ids.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) tool_call_ids: Vec<ToolCallId>,
    /// Append batch ids.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) append_batch_ids: Vec<AppendBatchId>,
    /// Cancellation request ids.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) cancellation_request_ids: Vec<CancellationRequestId>,
}

impl AllocatedIds {
    /// Construct bounded runtime-owned ID bags.
    ///
    /// # Arguments
    ///
    /// * `record_ids` - Preallocated record identities for this transition.
    /// * `event_ids` - Preallocated derived-event identities.
    /// * `effect_ids` - Preallocated effect identities.
    /// * `interaction_ids` - Preallocated interaction identities.
    /// * `message_ids` - Preallocated message identities.
    /// * `turn_ids` - Preallocated turn identities.
    /// * `model_request_ids` - Preallocated model-request identities.
    /// * `tool_batch_ids` - Preallocated tool-batch identities.
    /// * `tool_call_ids` - Preallocated tool-call identities.
    /// * `append_batch_ids` - Preallocated append-batch identities.
    /// * `cancellation_request_ids` - Preallocated cancellation-request identities.
    ///
    /// Each bag must stay within [`crate::SEMANTIC_ARRAY_MAX_ITEMS`]. Unused
    /// families may be empty.
    ///
    /// # Errors
    ///
    /// Returns [`RefsError::TooManyItems`] when any bag exceeds the v1 array ceiling.
    ///
    /// # Examples
    ///
    /// ```
    /// use finstack_ai_kernel::{AllocatedIds, RecordId};
    ///
    /// let ids = AllocatedIds::try_new(
    ///     vec![RecordId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("id")],
    ///     vec![],
    ///     vec![],
    ///     vec![],
    ///     vec![],
    ///     vec![],
    ///     vec![],
    ///     vec![],
    ///     vec![],
    ///     vec![],
    ///     vec![],
    /// )
    /// .expect("ids");
    /// assert_eq!(ids.record_ids().len(), 1);
    /// ```
    #[allow(clippy::too_many_arguments)]
    pub fn try_new(
        record_ids: Vec<RecordId>,
        event_ids: Vec<EventId>,
        effect_ids: Vec<EffectId>,
        interaction_ids: Vec<InteractionId>,
        message_ids: Vec<MessageId>,
        turn_ids: Vec<TurnId>,
        model_request_ids: Vec<ModelRequestId>,
        tool_batch_ids: Vec<ToolBatchId>,
        tool_call_ids: Vec<ToolCallId>,
        append_batch_ids: Vec<AppendBatchId>,
        cancellation_request_ids: Vec<CancellationRequestId>,
    ) -> Result<Self, RefsError> {
        for (field, len) in [
            ("record_ids", record_ids.len()),
            ("event_ids", event_ids.len()),
            ("effect_ids", effect_ids.len()),
            ("interaction_ids", interaction_ids.len()),
            ("message_ids", message_ids.len()),
            ("turn_ids", turn_ids.len()),
            ("model_request_ids", model_request_ids.len()),
            ("tool_batch_ids", tool_batch_ids.len()),
            ("tool_call_ids", tool_call_ids.len()),
            ("append_batch_ids", append_batch_ids.len()),
            ("cancellation_request_ids", cancellation_request_ids.len()),
        ] {
            if len > SEMANTIC_ARRAY_MAX_ITEMS {
                return Err(RefsError::TooManyItems {
                    field,
                    len,
                    max: SEMANTIC_ARRAY_MAX_ITEMS,
                });
            }
        }
        Ok(Self {
            record_ids,
            event_ids,
            effect_ids,
            interaction_ids,
            message_ids,
            turn_ids,
            model_request_ids,
            tool_batch_ids,
            tool_call_ids,
            append_batch_ids,
            cancellation_request_ids,
        })
    }

    /// Record ids.
    #[must_use]
    pub fn record_ids(&self) -> &[RecordId] {
        &self.record_ids
    }

    /// Event ids.
    #[must_use]
    pub fn event_ids(&self) -> &[EventId] {
        &self.event_ids
    }

    /// Effect ids.
    #[must_use]
    pub fn effect_ids(&self) -> &[EffectId] {
        &self.effect_ids
    }

    /// Interaction ids.
    #[must_use]
    pub fn interaction_ids(&self) -> &[InteractionId] {
        &self.interaction_ids
    }

    /// Message ids.
    #[must_use]
    pub fn message_ids(&self) -> &[MessageId] {
        &self.message_ids
    }

    /// Turn ids.
    #[must_use]
    pub fn turn_ids(&self) -> &[TurnId] {
        &self.turn_ids
    }

    /// Model request ids.
    #[must_use]
    pub fn model_request_ids(&self) -> &[ModelRequestId] {
        &self.model_request_ids
    }

    /// Tool batch ids.
    #[must_use]
    pub fn tool_batch_ids(&self) -> &[ToolBatchId] {
        &self.tool_batch_ids
    }

    /// Tool call ids.
    #[must_use]
    pub fn tool_call_ids(&self) -> &[ToolCallId] {
        &self.tool_call_ids
    }

    /// Append batch ids.
    #[must_use]
    pub fn append_batch_ids(&self) -> &[AppendBatchId] {
        &self.append_batch_ids
    }

    /// Cancellation request ids.
    #[must_use]
    pub fn cancellation_request_ids(&self) -> &[CancellationRequestId] {
        &self.cancellation_request_ids
    }
}

impl<'de> Deserialize<'de> for AllocatedIds {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        #[allow(clippy::struct_field_names)] // Mirrors the frozen `AllocatedIds` contract.
        struct Wire {
            #[serde(default)]
            record_ids: BoundedVec<RecordId, SEMANTIC_ARRAY_MAX_ITEMS>,
            #[serde(default)]
            event_ids: BoundedVec<EventId, SEMANTIC_ARRAY_MAX_ITEMS>,
            #[serde(default)]
            effect_ids: BoundedVec<EffectId, SEMANTIC_ARRAY_MAX_ITEMS>,
            #[serde(default)]
            interaction_ids: BoundedVec<InteractionId, SEMANTIC_ARRAY_MAX_ITEMS>,
            #[serde(default)]
            message_ids: BoundedVec<MessageId, SEMANTIC_ARRAY_MAX_ITEMS>,
            #[serde(default)]
            turn_ids: BoundedVec<TurnId, SEMANTIC_ARRAY_MAX_ITEMS>,
            #[serde(default)]
            model_request_ids: BoundedVec<ModelRequestId, SEMANTIC_ARRAY_MAX_ITEMS>,
            #[serde(default)]
            tool_batch_ids: BoundedVec<ToolBatchId, SEMANTIC_ARRAY_MAX_ITEMS>,
            #[serde(default)]
            tool_call_ids: BoundedVec<ToolCallId, SEMANTIC_ARRAY_MAX_ITEMS>,
            #[serde(default)]
            append_batch_ids: BoundedVec<AppendBatchId, SEMANTIC_ARRAY_MAX_ITEMS>,
            #[serde(default)]
            cancellation_request_ids: BoundedVec<CancellationRequestId, SEMANTIC_ARRAY_MAX_ITEMS>,
        }

        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(
            wire.record_ids.into_inner(),
            wire.event_ids.into_inner(),
            wire.effect_ids.into_inner(),
            wire.interaction_ids.into_inner(),
            wire.message_ids.into_inner(),
            wire.turn_ids.into_inner(),
            wire.model_request_ids.into_inner(),
            wire.tool_batch_ids.into_inner(),
            wire.tool_call_ids.into_inner(),
            wire.append_batch_ids.into_inner(),
            wire.cancellation_request_ids.into_inner(),
        )
        .map_err(de::Error::custom)
    }
}
