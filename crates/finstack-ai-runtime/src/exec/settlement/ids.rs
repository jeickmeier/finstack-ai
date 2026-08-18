use finstack_ai_kernel::{
    AllocatedIds, AppendBatchId, AppendBatchTag, CancellationRequestTag, EffectTag, EventId,
    EventTag, Id, InteractionTag, KernelError, KernelInput, MessageId, MessageTag, ModelRequestTag,
    RecordId, RecordTag, ToolBatchTag, ToolCallTag, TransitionEnv, TurnTag,
};

use crate::coordinator::CommitCoordinator;
use crate::run_types::RunHandleError;
use crate::{Clock, RandomSource};

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

pub(super) fn allocate_for_runtime_input<C: Clock, R: RandomSource>(
    coordinator: &CommitCoordinator,
    now: finstack_ai_kernel::Timestamp,
    input: &KernelInput,
    sources: &SettlementSources<C, R>,
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
                _ => {
                    return Err(RunHandleError::CancellationSettlement {
                        code: "runtime_input_id_kind_unknown",
                    });
                }
            },
            Err(_) => {
                return Err(RunHandleError::CancellationSettlement {
                    code: "runtime_input_rejected",
                });
            }
        }
    }
    Err(RunHandleError::CancellationSettlement {
        code: "runtime_input_allocation_exhausted",
    })
}

pub(super) async fn submit_resume_input<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    input: KernelInput,
    sources: &SettlementSources<C, R>,
) -> Result<(), RunHandleError> {
    let now = sources.now()?;
    let ids = allocate_resume_input(coordinator, now, &input, sources)?;
    let outcome = coordinator
        .submit(TransitionEnv { now, ids }, input)
        .await
        .map_err(RunHandleError::Coordinator)?;
    if let Some(fault) = outcome.fault {
        return Err(RunHandleError::Faulted {
            code: fault.code.into(),
        });
    }
    Ok(())
}

fn allocate_resume_input<C: Clock, R: RandomSource>(
    coordinator: &CommitCoordinator,
    now: finstack_ai_kernel::Timestamp,
    input: &KernelInput,
    sources: &SettlementSources<C, R>,
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
                _ => {
                    return Err(RunHandleError::ModelSettlement {
                        code: "runtime_input_id_kind_unknown",
                    });
                }
            },
            Err(error) => {
                return Err(RunHandleError::ModelSettlement { code: error.code() });
            }
        }
    }
    Err(RunHandleError::ModelSettlement {
        code: "runtime_input_allocation_exhausted",
    })
}
