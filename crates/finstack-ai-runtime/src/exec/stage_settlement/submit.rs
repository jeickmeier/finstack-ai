use std::sync::Arc;

use finstack_ai_kernel::{
    AllocatedIds, AppendBatchTag, EventTag, KernelError, KernelInput, RecordTag,
    ReducerStageOutcome, StageCursor, StageSettled, TransitionEnv,
};

use crate::commit::CommitOutcome;
use crate::coordinator::CommitCoordinator;
use crate::ids::{Clock, RandomSource};
use crate::run_types::RunHandleError;
use crate::settlement::{SettlementSources, stage_allocation};

use super::{MIDDLEWARE_STAGE_PAYLOAD_INVALID, stage_error};

/// `decide_limit`'s fixed record requirement (`decide.rs:684`).
const LIMIT_CROSSING_RECORDS: usize = 2;

/// `decide_limit`'s fixed event requirement (`decide.rs:684`).
const LIMIT_CROSSING_EVENTS: usize = 2;

/// Submit a stage settlement, swapping in the limit-crossing id bag when
/// `decide_limit` intercepts.
///
/// Identity/passthrough and folded submits share this choke point. The facade
/// (and [`stage_allocation`]) mint bags for `stage_id_requirements`. That table
/// is unreachable whenever `decide_limit` returns a decision, in which case
/// the kernel demands a fixed `IdRequirements::new(2, 2, 0, 0, 0, 0)`
/// regardless of stage or outcome. `validate_allocated_ids` rejects both
/// under- and over-allocation, so a short operational deadline that expires
/// between `AcceptRun` and the next stage cannot reuse the facade bag.
///
/// Rather than copy `decide_limit`'s predicate, this asks the kernel.
/// `CommitCoordinator::classify` is pure `Kernel::decide` with no commit: the
/// caller's bag is offered first and, if and only if it is rejected on id
/// cardinality, the fixed limit-crossing bag is offered instead. Any other
/// classification error is left to `submit`.
///
/// # Errors
///
/// Forwards the coordinator's error, or `middleware_stage_payload_invalid`
/// when the fallback bag cannot be constructed.
pub(super) async fn submit_settled<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    sources: &SettlementSources<C, R>,
    env: TransitionEnv,
    settled: StageSettled,
) -> Result<CommitOutcome, RunHandleError> {
    let env = match coordinator.classify(&env, KernelInput::StageSettled(settled.clone())) {
        Err(KernelError::AllocatedIdsExhausted { .. } | KernelError::UnusedAllocatedIds { .. }) => {
            TransitionEnv {
                now: env.now,
                ids: limit_crossing_allocation(sources)?,
            }
        }
        Ok(_) | Err(_) => env,
    };
    coordinator
        .submit(env, KernelInput::StageSettled(settled))
        .await
        .map_err(RunHandleError::Coordinator)
}

/// Submit a folded outcome with the id bag the kernel actually demands for it.
///
/// [`stage_allocation`] mirrors `stage_id_requirements` and nothing else. A
/// fold can itself *cause* a `decide_limit` interception (a `ContextPrepared`
/// that grows past `max_context_bytes` or `max_turns`, a `Retry` past
/// `max_retries`). [`submit_settled`] probes the kernel and substitutes the
/// limit-crossing bag when that happens.
///
/// # Errors
///
/// Forwards `stage_allocation`'s admissibility rejection **re-classified** by
/// [`folded_allocation_error`], the coordinator's error, or
/// `middleware_stage_payload_invalid` when an id bag is invalid.
pub(crate) async fn submit_folded<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    sources: &SettlementSources<C, R>,
    now: finstack_ai_kernel::Timestamp,
    cursor: StageCursor,
    outcome: ReducerStageOutcome,
) -> Result<CommitOutcome, RunHandleError> {
    let ids = stage_allocation(coordinator.state(), cursor, &outcome, sources)
        .map_err(folded_allocation_error)?;
    submit_settled(
        coordinator,
        sources,
        TransitionEnv { now, ids },
        StageSettled { cursor, outcome },
    )
    .await
}

/// Re-classify [`stage_allocation`]'s rejection of a **folded** outcome as a
/// middleware failure rather than a worker fault.
///
/// `stage_allocation` reports every admissibility rejection as
/// [`RunHandleError::ToolSettlement`] — it was written for the tool-settlement
/// path, where that classification is right. Both worker loops' `result_fault_code`
/// (`run_types.rs` `result_fault_code`) list `ToolSettlement` among
/// the variants that **tear down the command intake and set
/// `RunStatus::Faulted`**. For a folded outcome that is the wrong blast radius:
/// a middleware fold the kernel will not admit must fail the *run* that
/// submitted it and leave the worker healthy, which is precisely why
/// [`RunHandleError::Middleware`] exists (`run_types.rs:237-246`).
///
/// This was unreachable before `BeforeModel` folded: [`apply_fold`] could only
/// produce `Fail`, `Retry` at `BeforeFinalize`, and `ContextPrepared` at
/// `PrepareContext`, none of which hits a guarded arm of `stage_allocation`.
/// [`apply_model_draft`] makes it reachable, because
/// `ModelRequestPrepared` has one: a request whose `output_contract.kind` is not
/// `EffectOutputKind::ModelResponse` is rejected with
/// `stage_allocation_model_request_contract_mismatch` (`settlement.rs:573-577`).
///
/// The stable code is preserved verbatim, so the diagnosis does not change —
/// only the classification does. Non-`ToolSettlement` errors pass through
/// untouched.
pub(super) fn folded_allocation_error(error: RunHandleError) -> RunHandleError {
    match error {
        RunHandleError::ToolSettlement { code } => RunHandleError::Middleware {
            code: Arc::from(code),
        },
        other => other,
    }
}

/// The fixed id bag `decide_limit` demands when it intercepts an input
/// (`decide.rs:684`): one `LimitReached` record and one `RunFailed` record,
/// with one derived event each.
pub(super) fn limit_crossing_allocation<C: Clock, R: RandomSource>(
    sources: &SettlementSources<C, R>,
) -> Result<AllocatedIds, RunHandleError> {
    let records = (0..LIMIT_CROSSING_RECORDS)
        .map(|_| sources.generate::<RecordTag>())
        .collect::<Result<Vec<_>, _>>()?;
    let events = (0..LIMIT_CROSSING_EVENTS)
        .map(|_| sources.generate::<EventTag>())
        .collect::<Result<Vec<_>, _>>()?;
    AllocatedIds::try_new(
        records,
        events,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        vec![sources.generate::<AppendBatchTag>()?],
        Vec::new(),
    )
    .map_err(|_| stage_error(MIDDLEWARE_STAGE_PAYLOAD_INVALID))
}
