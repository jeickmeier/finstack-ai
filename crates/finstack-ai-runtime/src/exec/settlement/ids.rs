use std::future::Future;

use finstack_ai_kernel::{
    AllocatedIds, AppendBatchId, AppendBatchTag, CancellationRequestTag, EffectTag, EventId,
    EventTag, Id, InteractionTag, KernelError, KernelInput, MessageId, MessageTag, ModelRequestTag,
    RecordId, RecordTag, ToolBatchTag, ToolCallTag, TransitionEnv, TurnTag,
};

use crate::coordinator::CommitCoordinator;
use crate::ids::{Clock, RandomSource};
use crate::run_types::RunHandleError;

use super::SettlementSources;

#[derive(Default)]
struct RuntimeIdAllocation {
    records: Vec<RecordId>,
    events: Vec<EventId>,
    effects: Vec<Id<EffectTag>>,
    interactions: Vec<Id<InteractionTag>>,
    messages: Vec<MessageId>,
    turns: Vec<Id<TurnTag>>,
    model_requests: Vec<Id<ModelRequestTag>>,
    tool_batches: Vec<Id<ToolBatchTag>>,
    tool_calls: Vec<Id<ToolCallTag>>,
    cancellations: Vec<Id<CancellationRequestTag>>,
}

impl RuntimeIdAllocation {
    fn freeze(&self, append_batch_id: AppendBatchId) -> Result<AllocatedIds, RunHandleError> {
        AllocatedIds::try_new(
            self.records.clone(),
            self.events.clone(),
            self.effects.clone(),
            self.interactions.clone(),
            self.messages.clone(),
            self.turns.clone(),
            self.model_requests.clone(),
            self.tool_batches.clone(),
            self.tool_calls.clone(),
            vec![append_batch_id],
            self.cancellations.clone(),
        )
        .map_err(|_| RunHandleError::CancellationSettlement {
            code: "runtime_input_ids_invalid",
        })
    }
}

/// Submit a runtime-authored input, growing the id bag until the kernel
/// classifies it. `error` wraps the allocator's own stable codes; `rejected`
/// maps any other kernel rejection.
pub(super) async fn submit_runtime_input<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    input: KernelInput,
    sources: &SettlementSources<C, R>,
    error: fn(&'static str) -> RunHandleError,
    rejected: fn(KernelError) -> RunHandleError,
) -> Result<(), RunHandleError> {
    let now = sources.now()?;
    let ids = allocate_runtime_input(coordinator, now, &input, sources, error, rejected)?;
    super::committed(coordinator.submit(TransitionEnv { now, ids }, input).await)
}

/// [`submit_runtime_input`] with resume-path error classification.
pub(super) fn submit_resume_input<'a, C: Clock, R: RandomSource>(
    coordinator: &'a mut CommitCoordinator,
    input: KernelInput,
    sources: &'a SettlementSources<C, R>,
) -> impl Future<Output = Result<(), RunHandleError>> + 'a {
    submit_runtime_input(
        coordinator,
        input,
        sources,
        |code| RunHandleError::ModelSettlement { code },
        |error| RunHandleError::ModelSettlement { code: error.code() },
    )
}

pub(super) fn allocate_runtime_input<C: Clock, R: RandomSource>(
    coordinator: &CommitCoordinator,
    now: finstack_ai_kernel::Timestamp,
    input: &KernelInput,
    sources: &SettlementSources<C, R>,
    error: fn(&'static str) -> RunHandleError,
    rejected: fn(KernelError) -> RunHandleError,
) -> Result<AllocatedIds, RunHandleError> {
    let append_batch_id = sources.generate::<AppendBatchTag>()?;
    let mut allocation = RuntimeIdAllocation::default();
    for _ in 0..finstack_ai_kernel::SEMANTIC_ARRAY_MAX_ITEMS {
        let ids = allocation.freeze(append_batch_id)?;
        match coordinator.classify(
            &TransitionEnv {
                now,
                ids: ids.clone(),
            },
            input.clone(),
        ) {
            Ok(_) => return Ok(ids),
            Err(KernelError::AllocatedIdsExhausted { kind }) => match kind {
                "record_ids" => allocation.records.push(sources.generate::<RecordTag>()?),
                "event_ids" => allocation.events.push(sources.generate::<EventTag>()?),
                "effect_ids" => allocation.effects.push(sources.generate::<EffectTag>()?),
                "interaction_ids" => allocation
                    .interactions
                    .push(sources.generate::<InteractionTag>()?),
                "message_ids" => allocation.messages.push(sources.generate::<MessageTag>()?),
                "turn_ids" => allocation.turns.push(sources.generate::<TurnTag>()?),
                "model_request_ids" => allocation
                    .model_requests
                    .push(sources.generate::<ModelRequestTag>()?),
                "tool_batch_ids" => allocation
                    .tool_batches
                    .push(sources.generate::<ToolBatchTag>()?),
                "tool_call_ids" => allocation
                    .tool_calls
                    .push(sources.generate::<ToolCallTag>()?),
                "cancellation_request_ids" => allocation
                    .cancellations
                    .push(sources.generate::<CancellationRequestTag>()?),
                _ => return Err(error("runtime_input_id_kind_unknown")),
            },
            Err(other) => return Err(rejected(other)),
        }
    }
    Err(error("runtime_input_allocation_exhausted"))
}
