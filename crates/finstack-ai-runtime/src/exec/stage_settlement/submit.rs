use std::sync::Arc;

use finstack_ai_kernel::{
    AllocatedIds, AppendBatchTag, EventTag, KernelError, KernelInput, RecordTag,
    ReducerStageOutcome, StageCursor, StageSettled, TransitionEnv,
};

use crate::coordinator::CommitCoordinator;
use crate::run_types::RunHandleError;
use crate::settlement::{SettlementSources, stage_allocation};
use crate::{Clock, CommitOutcome, RandomSource};

use super::{MIDDLEWARE_STAGE_PAYLOAD_INVALID, stage_error};

/// `decide_limit`'s fixed record requirement (`decide.rs:684`).
const LIMIT_CROSSING_RECORDS: usize = 2;

/// `decide_limit`'s fixed event requirement (`decide.rs:684`).
const LIMIT_CROSSING_EVENTS: usize = 2;

pub(super) async fn submit_settled(
    coordinator: &mut CommitCoordinator,
    env: TransitionEnv,
    settled: StageSettled,
) -> Result<CommitOutcome, RunHandleError> {
    coordinator
        .submit(env, KernelInput::StageSettled(settled))
        .await
        .map_err(RunHandleError::Coordinator)
}

/// Submit a folded outcome with the id bag the kernel actually demands for it.
///
/// # Why this is a two-shot probe and not one table lookup
///
/// [`stage_allocation`] mirrors `stage_id_requirements` (`decide.rs:1101-1183`)
/// and nothing else. That table is unreachable whenever `decide_limit`
/// (`decide.rs:469-709`) returns a decision, in which case the kernel demands a
/// fixed `IdRequirements::new(2, 2, 0, 0, 0, 0)` (`decide.rs:684`) regardless of
/// stage or outcome. The crossing test is
/// `deadline_crossing.or(first_limit_crossing(accepted, &usage))`
/// (`decide.rs:680`), and `decide_limit` increments usage **from the submitted
/// outcome itself** (`decide.rs:547-609`) before testing — so a fold is
/// perfectly capable of *causing* the interception it then has to satisfy: a
/// `ContextPrepared` fold that grows `context_bytes` past `max_context_bytes`,
/// a `ContextPrepared` at all when `max_turns` is already reached, or a
/// middleware `Retry` that pushes `usage.retries` past `max_retries`.
///
/// The two requirements cannot be satisfied at once. `validate_allocated_ids`
/// (`allocated_ids.rs:97-107`) rejects `actual < needed` **and**
/// `actual > needed`: allocation is an exact match, so a superset bag fails
/// with `UnusedAllocatedIds` instead of being tolerated. The caller must
/// therefore know *which* path the kernel will take before it allocates.
///
/// Rather than hand-copy `decide_limit`'s predicate — the same
/// copy-the-kernel's-table drift that made `StageIds::for_outcome` wrong — this
/// asks the kernel. `CommitCoordinator::classify` is the pure `Kernel::decide`
/// with no commit, so the stage-table bag is offered first and, if and only if
/// it is rejected on id cardinality, the fixed limit-crossing bag is offered
/// instead. No stage tuple in `stage_id_requirements` is `(2, 2, …)`, so a
/// limit crossing always produces a cardinality mismatch on the first probe and
/// the fallback is always reached when it is needed. Any other classification
/// error is left to `submit`, which surfaces the kernel's own diagnosis.
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
    let settled = StageSettled { cursor, outcome };
    let input = KernelInput::StageSettled(settled.clone());
    let env = TransitionEnv { now, ids };
    let env = match coordinator.classify(&env, input) {
        Err(KernelError::AllocatedIdsExhausted { .. } | KernelError::UnusedAllocatedIds { .. }) => {
            TransitionEnv {
                now,
                ids: limit_crossing_allocation(sources)?,
            }
        }
        Ok(_) | Err(_) => env,
    };
    submit_settled(coordinator, env, settled).await
}

/// Re-classify [`stage_allocation`]'s rejection of a **folded** outcome as a
/// middleware failure rather than a worker fault.
///
/// `stage_allocation` reports every admissibility rejection as
/// [`RunHandleError::ToolSettlement`] — it was written for the tool-settlement
/// path, where that classification is right. Both worker loops' `result_fault_code`
/// (`task.rs:1173-1190`, `host_task.rs:1389-1406`) list `ToolSettlement` among
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
    let mut records = Vec::with_capacity(LIMIT_CROSSING_RECORDS);
    for _ in 0..LIMIT_CROSSING_RECORDS {
        records.push(sources.generate::<RecordTag>()?);
    }
    let mut events = Vec::with_capacity(LIMIT_CROSSING_EVENTS);
    for _ in 0..LIMIT_CROSSING_EVENTS {
        events.push(sources.generate::<EventTag>()?);
    }
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
