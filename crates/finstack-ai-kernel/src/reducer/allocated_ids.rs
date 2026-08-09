//! Canonical preallocated-ID requirement validation.

use crate::AllocatedIds;

use super::KernelError;

#[derive(Clone, Copy)]
pub(super) struct IdRequirements {
    records: usize,
    events: usize,
    effects: usize,
    turns: usize,
    model_requests: usize,
    messages: usize,
}

impl IdRequirements {
    pub(super) const fn new(
        records: usize,
        events: usize,
        effects: usize,
        turns: usize,
        model_requests: usize,
        messages: usize,
    ) -> Self {
        Self {
            records,
            events,
            effects,
            turns,
            model_requests,
            messages,
        }
    }
}

pub(super) fn validate_allocated_ids(
    ids: &AllocatedIds,
    required: IdRequirements,
) -> Result<(), KernelError> {
    let queues = [
        ("record_ids", ids.record_ids().len(), required.records),
        ("event_ids", ids.event_ids().len(), required.events),
        ("effect_ids", ids.effect_ids().len(), required.effects),
        ("turn_ids", ids.turn_ids().len(), required.turns),
        (
            "model_request_ids",
            ids.model_request_ids().len(),
            required.model_requests,
        ),
        ("message_ids", ids.message_ids().len(), required.messages),
        ("interaction_ids", ids.interaction_ids().len(), 0),
        ("tool_batch_ids", ids.tool_batch_ids().len(), 0),
        ("tool_call_ids", ids.tool_call_ids().len(), 0),
        (
            "cancellation_request_ids",
            ids.cancellation_request_ids().len(),
            0,
        ),
    ];
    for (kind, actual, needed) in queues {
        if actual < needed {
            return Err(KernelError::AllocatedIdsExhausted { kind });
        }
    }
    for (kind, actual, needed) in queues {
        if actual > needed {
            return Err(KernelError::UnusedAllocatedIds { kind });
        }
    }
    Ok(())
}
