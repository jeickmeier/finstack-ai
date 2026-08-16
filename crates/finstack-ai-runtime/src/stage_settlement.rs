//! Aggregate middleware fold at the stage-settlement choke point.
//!
//! Every facade stage settlement funnels through `RunHandle::submit` into one
//! `coordinator.submit(env, input)` call in a worker command loop. Intercepting
//! the [`KernelInput::StageSettled`] branch there is the single point at which
//! the six facade-authored stages can be routed through their middleware chain,
//! with no change to the facade's stage sequence and no kernel change: the `N`
//! [`crate::middleware::StageOutcome`]s a chain produces are folded into exactly
//! ONE [`ReducerStageOutcome`] per `(cycle, stage)` cursor, which is what
//! `KernelInput::StageSettled` already carries.
//!
//! # Governing invariant: passthrough when the chain is empty
//!
//! When no driver is installed, or the driver has no component registered for
//! the cursor's stage, or the chain's fold is the identity, the facade's `env`
//! and `input` are submitted **byte for byte unchanged** — the facade's own
//! pre-minted record/event/effect ids included. The feature is opt-in per
//! agent: an agent with no middleware must produce a journal identical to the
//! one it produced before this module existed.
//!
//! # Stage coverage
//!
//! [`settle_facade_stage`] folds `BeforeRun`, `PrepareContext`, `AfterModel`,
//! `AfterToolBatch`, and `BeforeFinalize`. `BeforeModel` is deliberately
//! passthrough here: its [`StageInput`] is the typed
//! `StageInput::BeforeModel(Box<BeforeModelInput>)`, whose assembly needs the
//! run's `LockedModelContextProfile` and is owned by a later task.
//! `BeforeToolBatch` never reaches this choke point at all — the facade never
//! settles it; `settlement::prepare_tool_batch_if_ready` does.

use std::sync::Arc;

use finstack_ai_kernel::{
    AllocatedIds, AppendBatchTag, EventTag, KernelError, KernelInput, KernelState, Message,
    MessageRole, MessageTag, Metadata, ProviderIds, RawJson, RecordTag, ReducerStageOutcome,
    SEMANTIC_ARRAY_MAX_ITEMS, Stage, StageCursor, StageSettled, TransitionEnv,
};

use crate::context::ContextItem;
use crate::coordinator::CommitCoordinator;
use crate::middleware::{MiddlewareError, StageInput};
use crate::middleware_driver::{
    MIDDLEWARE_STAGE_BOUNDS_EXCEEDED, MIDDLEWARE_STAGE_UNLANDABLE, MiddlewareStageContext,
    StageDriver, StageFold, StageTerminal, derived_stage_effect_id,
};
use crate::run_types::RunHandleError;
use crate::settlement::{SettlementSources, stage_allocation};
use crate::{Clock, CommitOutcome, RandomSource, RunCallContext};

/// The run has no dispatch identity (locator/authorization) for a stage
/// invocation. Only reachable before `AcceptRun` commits, which no stage
/// settlement can precede.
const MIDDLEWARE_STAGE_IDENTITY_MISSING: &str = "middleware_stage_identity_missing";

/// The kernel state carries no payload the cursor's [`StageInput`] can be built
/// from (an `AfterModel` cursor with no messages, a `BeforeFinalize` cursor with
/// no terminal candidate, a stage this choke point does not fold).
const MIDDLEWARE_STAGE_INPUT_INVALID: &str = "middleware_stage_input_invalid";

/// A folded stage payload could not be canonicalized, parsed back, or rebuilt
/// into a valid kernel [`Message`]/[`AllocatedIds`].
const MIDDLEWARE_STAGE_PAYLOAD_INVALID: &str = "middleware_stage_payload_invalid";

/// `decide_limit`'s fixed record requirement (`decide.rs:684`).
const LIMIT_CROSSING_RECORDS: usize = 2;

/// `decide_limit`'s fixed event requirement (`decide.rs:684`).
const LIMIT_CROSSING_EVENTS: usize = 2;

/// Build the run's stage driver from the chain the facade installed.
///
/// Called once per run in the spawn path, where both the installed chain and
/// the run-scoped cancellation signal are in scope, and handed to the worker.
/// `None` means no chain was installed at all. A chain that *is* installed but
/// registers nothing for a stage still produces a driver: the facade installs
/// its chain unconditionally (`agent.rs:613`), so `middleware_chain()` is
/// always `Some` in a facade-started run, possibly wrapping an empty chain.
/// [`StageDriver::is_active`] — not this `Option` — is the passthrough gate.
pub(crate) fn stage_driver(
    coordinator: &CommitCoordinator,
    cancellation: &crate::CancellationSignal,
) -> Option<StageDriver> {
    coordinator
        .middleware_chain()
        .map(|chain| StageDriver::new(Arc::clone(chain), cancellation.child()))
}

fn stage_error(code: &'static str) -> RunHandleError {
    RunHandleError::Middleware {
        code: Arc::from(code),
    }
}

fn middleware_error(error: &MiddlewareError) -> RunHandleError {
    RunHandleError::Middleware {
        code: Arc::from(error.code()),
    }
}

/// Whether [`settle_facade_stage`] folds a chain at `stage`.
///
/// `BeforeModel` and `BeforeToolBatch` are excluded — see the module docs.
const fn folds_at(stage: Stage) -> bool {
    matches!(
        stage,
        Stage::BeforeRun
            | Stage::PrepareContext
            | Stage::AfterModel
            | Stage::AfterToolBatch
            | Stage::BeforeFinalize
    )
}

/// Submit one worker command, routing `StageSettled` through the stage driver.
///
/// The single entry point the three worker command loops call in place of a
/// bare `coordinator.submit`. Every other [`KernelInput`] is forwarded
/// untouched.
///
/// # Errors
///
/// Forwards the coordinator's error, or a stable
/// [`RunHandleError::Middleware`] when the chain, the fold, or the folded
/// allocation could not produce a valid settlement.
pub(crate) async fn submit_command<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    driver: Option<&StageDriver>,
    sources: &SettlementSources<C, R>,
    env: TransitionEnv,
    input: KernelInput,
) -> Result<CommitOutcome, RunHandleError> {
    match input {
        KernelInput::StageSettled(settled) => {
            settle_facade_stage(coordinator, driver, sources, env, settled).await
        }
        other => coordinator
            .submit(env, other)
            .await
            .map_err(RunHandleError::Coordinator),
    }
}

/// Fold the stage's middleware chain into the facade's base outcome and submit
/// the result.
///
/// # Passthrough
///
/// Returns `coordinator.submit(env, KernelInput::StageSettled(settled))`
/// unchanged when `driver` is `None`, when the driver has no component
/// registered at `settled.cursor.stage`, when this choke point does not fold
/// that stage, or when the chain's fold is the identity. In all four cases the
/// facade's `env` — its `now` and its pre-minted id bags — is reused verbatim.
///
/// # Re-allocation
///
/// Once the fold is non-identity the submitted outcome is no longer the one the
/// facade allocated for, so the ids the facade minted (`agent.rs`'s `StageIds`)
/// are **discarded** and a fresh bag is taken from `sources`, which shares the
/// run's clock and random source. See [`submit_folded`] for why that bag is not
/// simply [`stage_allocation`]'s answer. The facade's `env.now` is kept: the
/// chain running does not advance the run's semantic transition instant, and
/// reusing it keeps a folded submission's deadline evaluation identical to the
/// passthrough it replaced.
///
/// # Errors
///
/// Forwards the coordinator's error, or a stable
/// [`RunHandleError::Middleware`] when a component fails, when the aggregate
/// fold has no kernel landing at this cursor, or when the folded payload cannot
/// be rebuilt.
pub(crate) async fn settle_facade_stage<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    driver: Option<&StageDriver>,
    sources: &SettlementSources<C, R>,
    env: TransitionEnv,
    settled: StageSettled,
) -> Result<CommitOutcome, RunHandleError> {
    let cursor = settled.cursor;
    let Some(driver) =
        driver.filter(|driver| folds_at(cursor.stage) && driver.is_active(cursor.stage))
    else {
        return submit_settled(coordinator, env, settled).await;
    };
    let input = stage_input(coordinator.state(), cursor.stage, &settled.outcome)?;
    let fold = run_stage_chain(coordinator, Some(driver), cursor, input).await?;
    if fold.is_identity() {
        return submit_settled(coordinator, env, settled).await;
    }
    let outcome = apply_fold(&fold, cursor, settled.outcome, sources)?;
    submit_folded(coordinator, sources, env.now, cursor, outcome).await
}

async fn submit_settled(
    coordinator: &mut CommitCoordinator,
    env: TransitionEnv,
    settled: StageSettled,
) -> Result<CommitOutcome, RunHandleError> {
    coordinator
        .submit(env, KernelInput::StageSettled(settled))
        .await
        .map_err(RunHandleError::Coordinator)
}

/// Run one stage's ordered chain and fold its outcomes into a [`StageFold`].
///
/// Read-only in the coordinator: it needs `state()` for the dispatch seed and
/// nothing else. Returns [`StageFold::default`] — the identity — whenever the
/// driver is absent or has no component at `cursor.stage`, so a caller can use
/// the returned fold uniformly without re-testing the passthrough condition.
///
/// # Errors
///
/// Returns a stable [`RunHandleError::Middleware`] carrying the component's own
/// code, the fold's `middleware_stage_unlandable` /
/// `middleware_stage_bounds_exceeded`, or
/// `middleware_stage_identity_missing` when the run has no dispatch identity.
pub(crate) async fn run_stage_chain(
    coordinator: &CommitCoordinator,
    driver: Option<&StageDriver>,
    cursor: StageCursor,
    input: StageInput,
) -> Result<StageFold, RunHandleError> {
    let Some(driver) = driver.filter(|driver| driver.is_active(cursor.stage)) else {
        return Ok(StageFold::default());
    };
    debug_assert_eq!(
        cursor.stage,
        input.stage(),
        "the cursor and the stage input must describe the same stage"
    );
    let seed = coordinator
        .stage_dispatch_seed()
        .ok_or_else(|| stage_error(MIDDLEWARE_STAGE_IDENTITY_MISSING))?;
    let run = RunCallContext {
        effect_id: derived_stage_effect_id(&seed.locator, cursor.cycle, cursor.stage),
        locator: seed.locator,
        authorization: seed.authorization,
        attempt: seed.attempt,
        deadline: seed.deadline,
        budget_scope_id: seed.budget_scope_id,
        cancellation: driver.cancellation().child(),
    };
    let ctx = MiddlewareStageContext::new(run, driver.chain().digest(), cursor);
    let outcomes = driver
        .run_stage(&ctx, input)
        .await
        .map_err(|error| middleware_error(&error))?;
    StageFold::accumulate(cursor.stage, &outcomes).map_err(|error| middleware_error(&error))
}

/// Build the [`StageInput`] one stage's chain observes.
///
/// Every payload is JCS-canonical JSON over live `coordinator.state()`, not a
/// recovered copy:
///
/// - `BeforeRun` — the run's message array (empty at that cursor).
/// - `PrepareContext` — the base outcome's own prepared message array, i.e.
///   exactly what would land if no component contributed.
/// - `AfterModel` — the single most recent message, which at that cursor is the
///   assistant message the model just produced.
/// - `AfterToolBatch` — the trailing run of `MessageRole::Tool` messages, i.e.
///   the results the batch just appended.
/// - `BeforeFinalize` — the terminal candidate, live and O(1) here.
fn stage_input(
    state: &KernelState,
    cursor_stage: Stage,
    outcome: &ReducerStageOutcome,
) -> Result<StageInput, RunHandleError> {
    match cursor_stage {
        Stage::BeforeRun => Ok(StageInput::BeforeRun {
            value: canonical_messages(state.messages.as_slice())?,
        }),
        Stage::PrepareContext => {
            let value = match outcome {
                ReducerStageOutcome::ContextPrepared { messages } => canonical_messages(messages)?,
                _ => canonical_messages(state.messages.as_slice())?,
            };
            Ok(StageInput::PrepareContext { value })
        }
        Stage::AfterModel => {
            let message = state
                .messages
                .last()
                .ok_or_else(|| stage_error(MIDDLEWARE_STAGE_INPUT_INVALID))?;
            Ok(StageInput::AfterModel {
                value: canonical_message(message)?,
            })
        }
        Stage::AfterToolBatch => Ok(StageInput::AfterToolBatch {
            value: canonical_messages(trailing_role_run(
                state.messages.as_slice(),
                MessageRole::Tool,
            ))?,
        }),
        Stage::BeforeFinalize => Ok(StageInput::BeforeFinalize {
            candidate: canonical_terminal_candidate(state)?,
        }),
        Stage::BeforeModel | Stage::BeforeToolBatch => {
            Err(stage_error(MIDDLEWARE_STAGE_INPUT_INVALID))
        }
    }
}

/// The maximal trailing slice of `messages` whose role is `role`.
fn trailing_role_run(messages: &[Message], role: MessageRole) -> &[Message] {
    let start = messages
        .iter()
        .rposition(|message| message.role() != role)
        .map_or(0, |index| index + 1);
    &messages[start..]
}

/// Apply an aggregate fold to the facade's base outcome.
///
/// A [`StageTerminal`] wins outright: it replaces the base outcome whatever the
/// base was, which is what makes a middleware `Retry` at `BeforeFinalize`
/// supersede the facade's own structured-output `Retry` at the same cursor.
/// Otherwise the only non-terminal fold with a kernel landing at the stages
/// this choke point folds is `ContextPrepared` at `PrepareContext`; a
/// non-identity fold anywhere else has nowhere to land and is rejected rather
/// than silently dropped.
///
/// # Errors
///
/// Returns [`MIDDLEWARE_STAGE_UNLANDABLE`] for a non-terminal fold with no
/// landing at `cursor`, or the payload errors of [`apply_context_prepared`].
fn apply_fold<C: Clock, R: RandomSource>(
    fold: &StageFold,
    cursor: StageCursor,
    base: ReducerStageOutcome,
    sources: &SettlementSources<C, R>,
) -> Result<ReducerStageOutcome, RunHandleError> {
    if let Some(terminal) = fold.terminal.as_ref() {
        return Ok(match terminal {
            StageTerminal::Fail(descriptor) => {
                ReducerStageOutcome::Fail(descriptor.as_ref().clone())
            }
            StageTerminal::Retry(directive) => ReducerStageOutcome::Retry(directive.clone()),
        });
    }
    match base {
        ReducerStageOutcome::ContextPrepared { messages }
            if cursor.stage == Stage::PrepareContext =>
        {
            Ok(ReducerStageOutcome::ContextPrepared {
                messages: apply_context_prepared(fold, &messages, sources)?,
            })
        }
        other if fold.is_identity() => Ok(other),
        _ => Err(stage_error(MIDDLEWARE_STAGE_UNLANDABLE)),
    }
}

/// Rebuild a `ContextPrepared` message array from an aggregate fold.
///
/// # Precedence: replacement re-bases, additive contributions apply on top
///
/// `StageFold` aggregates each outcome kind separately and keeps no ordering
/// *between* kinds, so a positional "last writer across kinds wins" rule is not
/// representable. This applier therefore defines precedence at the field level:
/// `fold.replacement` substitutes the **base payload** the stage was going to
/// land, and `fold.instructions`/`fold.context` are appended to whatever base
/// survives — the replacement when there is one, the facade's messages when
/// there is not. Both orders are stable and documented: replacement first, then
/// instructions (as `MessageRole::System`), then context (as
/// `MessageRole::User`), each group in chain order.
///
/// The alternative — letting a `Replace` from one component discard an
/// `AddContext` from another — was rejected: silently dropping a component's
/// contribution is exactly the failure mode
/// [`MIDDLEWARE_STAGE_UNLANDABLE`] exists to prevent, and a chain whose
/// components disagree that badly is a configuration error the driver cannot
/// resolve by picking a winner.
///
/// Ids for the appended messages come from `sources`, not from `AllocatedIds`:
/// the kernel requires **zero** message ids for `ContextPrepared`
/// (`decide.rs:1131-1133`, tuple `(2, 0, 0, 1, 0, 0)`), exactly as the facade's
/// own `Agent::context_messages` mints them outside the id bag.
///
/// # Errors
///
/// Returns [`MIDDLEWARE_STAGE_BOUNDS_EXCEEDED`] when the rebuilt array would
/// exceed the kernel's `SEMANTIC_ARRAY_MAX_ITEMS` bound (`decide.rs:1195-1202`
/// rejects it), or `middleware_stage_payload_invalid` when the replacement
/// payload is not a message array or an item cannot become a valid message.
pub(crate) fn apply_context_prepared<C: Clock, R: RandomSource>(
    fold: &StageFold,
    base: &[Message],
    sources: &SettlementSources<C, R>,
) -> Result<Arc<[Message]>, RunHandleError> {
    let mut messages = match fold.replacement.as_ref() {
        Some(replacement) => parse_messages(replacement)?,
        None => base.to_vec(),
    };
    for item in &fold.instructions {
        messages.push(message_from_item(item, MessageRole::System, sources)?);
    }
    for item in &fold.context {
        messages.push(message_from_item(item, MessageRole::User, sources)?);
    }
    if messages.len() > SEMANTIC_ARRAY_MAX_ITEMS {
        return Err(stage_error(MIDDLEWARE_STAGE_BOUNDS_EXCEEDED));
    }
    Ok(messages.into())
}

fn message_from_item<C: Clock, R: RandomSource>(
    item: &ContextItem,
    role: MessageRole,
    sources: &SettlementSources<C, R>,
) -> Result<Message, RunHandleError> {
    Message::try_new(
        sources.generate::<MessageTag>()?,
        role,
        item.content.to_vec(),
        sources.now()?,
        None,
        ProviderIds::empty(),
        Metadata::empty(),
    )
    .map_err(|_| stage_error(MIDDLEWARE_STAGE_PAYLOAD_INVALID))
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
/// Forwards `stage_allocation`'s admissibility rejection, the coordinator's
/// error, or `middleware_stage_payload_invalid` when an id bag is invalid.
async fn submit_folded<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    sources: &SettlementSources<C, R>,
    now: finstack_ai_kernel::Timestamp,
    cursor: StageCursor,
    outcome: ReducerStageOutcome,
) -> Result<CommitOutcome, RunHandleError> {
    let ids = stage_allocation(coordinator.state(), cursor, &outcome, sources)?;
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

/// The fixed id bag `decide_limit` demands when it intercepts an input
/// (`decide.rs:684`): one `LimitReached` record and one `RunFailed` record,
/// with one derived event each.
fn limit_crossing_allocation<C: Clock, R: RandomSource>(
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

// --- codec helpers, on live kernel state rather than a recovered copy -------

/// JCS-canonical bytes for an ordered message array.
fn canonical_messages(messages: &[Message]) -> Result<RawJson, RunHandleError> {
    canonical(&messages)
}

/// JCS-canonical bytes for one message.
fn canonical_message(message: &Message) -> Result<RawJson, RunHandleError> {
    canonical(message)
}

/// Parse a `Replace` payload back into an ordered message array.
fn parse_messages(value: &RawJson) -> Result<Vec<Message>, RunHandleError> {
    serde_json::from_slice(value.as_bytes())
        .map_err(|_| stage_error(MIDDLEWARE_STAGE_PAYLOAD_INVALID))
}

/// JCS-canonical bytes for the terminal candidate gated by `BeforeFinalize`,
/// read live rather than through a `CommitCoordinator::recover`.
fn canonical_terminal_candidate(state: &KernelState) -> Result<RawJson, RunHandleError> {
    let candidate = state
        .terminal_candidate
        .as_ref()
        .ok_or_else(|| stage_error(MIDDLEWARE_STAGE_INPUT_INVALID))?;
    canonical(candidate)
}

fn canonical<T: serde::Serialize>(value: &T) -> Result<RawJson, RunHandleError> {
    let bytes = serde_json_canonicalizer::to_vec(value)
        .map_err(|_| stage_error(MIDDLEWARE_STAGE_PAYLOAD_INVALID))?;
    RawJson::parse(&bytes).map_err(|_| stage_error(MIDDLEWARE_STAGE_PAYLOAD_INVALID))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::future::Future;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use std::task::{Context, Poll, Waker};

    use finstack_ai_kernel::{
        AcceptRun, AllocatedIds, AppendBatchId, AppendRequest, BudgetPropagation,
        CancellationPropagation, CommittedBatch, ContentBlock, DeadlinePropagation, Digest,
        ErrorCategory, ErrorDescriptor, Id, IdTag, KernelInput, LaneTag, Message, MessageRole,
        Metadata, PrincipalPropagation, PrincipalRef, ProviderIds, RawJson, RecordEnvelope,
        ReducerStageOutcome, RunAccepted, RunLimits, RunPropagationPolicy, RunRelation,
        RunSecurityContext, Sensitivity, SessionTag, Stage, StageCursor, StageSettled, TextBlock,
        Timestamp, TransitionEnv, Version,
    };

    use super::*;
    use crate::context::{ContextAuthority, ContextItem, ContextItemKind, ContextProvenance};
    use crate::middleware::{
        MiddlewareDescriptor, MiddlewareOrder, MiddlewareRegistration, MiddlewareRole, OrderTier,
        ResolvedMiddlewareChain, StageMask, StageOutcome,
    };
    use crate::middleware_driver::StageDriver;
    use crate::{
        CancellationSignal, CommitCoordinator, ExternalClock, IdGenerationError, JournalStore,
        LoadRequest, LoadedSession, PortFuture, RandomSource, SnapshotReceipt, SnapshotRequest,
        StoreError, StoreHealth,
    };

    fn block_on<T>(future: impl Future<Output = T>) -> T {
        let mut context = Context::from_waker(Waker::noop());
        let mut future = std::pin::pin!(future);
        loop {
            match future.as_mut().poll(&mut context) {
                Poll::Ready(value) => return value,
                Poll::Pending => std::thread::yield_now(),
            }
        }
    }

    fn id<T: IdTag>(ordinal: u64) -> Id<T> {
        let mut bytes = [0_u8; 16];
        bytes[6] = 0x70;
        bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        Id::from_bytes(bytes)
    }

    fn timestamp(ms: i64) -> Timestamp {
        Timestamp::from_unix_ms(ms).expect("timestamp")
    }

    #[expect(clippy::too_many_arguments, reason = "mirrors AllocatedIds' own bags")]
    fn env(
        now: i64,
        records: &[u64],
        events: &[u64],
        effects: &[u64],
        turns: &[u64],
        model_requests: &[u64],
        messages: &[u64],
        append_batch: u64,
    ) -> TransitionEnv {
        TransitionEnv {
            now: timestamp(now),
            ids: AllocatedIds::try_new(
                records.iter().copied().map(id).collect(),
                events.iter().copied().map(id).collect(),
                effects.iter().copied().map(id).collect(),
                Vec::new(),
                messages.iter().copied().map(id).collect(),
                turns.iter().copied().map(id).collect(),
                model_requests.iter().copied().map(id).collect(),
                Vec::new(),
                Vec::new(),
                vec![id(append_batch)],
                Vec::new(),
            )
            .expect("allocated ids"),
        }
    }

    fn acceptance(limits: RunLimits) -> RunAccepted {
        let run_id = id(3);
        RunAccepted::try_new(
            run_id,
            RunRelation::root(run_id).expect("relation"),
            RunSecurityContext::try_new(
                "tenant-a",
                PrincipalRef::try_new("issuer-a", "subject-a", Some("tenant-a"))
                    .expect("principal"),
                "oidc",
                "high",
                "policy-v1",
                "decision-v1",
                None,
            )
            .expect("security"),
            None,
            limits,
            RunPropagationPolicy {
                cancellation: CancellationPropagation::Cascade,
                deadline: DeadlinePropagation::MinimumOfParentAndChild,
                budget: BudgetPropagation::SharedScope,
                principal: PrincipalPropagation::Inherit,
            },
            Digest::raw_json(br#"{"agent":"fixture"}"#),
            None,
        )
        .expect("acceptance")
    }

    fn accept_input(limits: RunLimits) -> KernelInput {
        KernelInput::AcceptRun(AcceptRun {
            session_id: id::<SessionTag>(1),
            lane_id: id::<LaneTag>(2),
            accepted: acceptance(limits),
        })
    }

    fn user_message(ordinal: u64, text: &str) -> Message {
        Message::try_new(
            id(ordinal),
            MessageRole::User,
            vec![ContentBlock::Text(TextBlock::try_new(text).expect("text"))],
            timestamp(900),
            None,
            ProviderIds::empty(),
            Metadata::empty(),
        )
        .expect("message")
    }

    fn message_text(message: &Message) -> String {
        message
            .content()
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text(text) => Some(text.text().to_owned()),
                _ => None,
            })
            .collect()
    }

    fn item(text: &str) -> ContextItem {
        ContextItem::try_new(
            ContextItemKind::Instruction,
            vec![ContentBlock::Text(TextBlock::try_new(text).expect("text"))],
            ContextProvenance {
                source_id: Arc::from("fixture.source"),
                source_ref: None,
                external: false,
            },
            ContextAuthority::TrustedApplication,
            0,
            4,
            Sensitivity::Internal,
            false,
        )
        .expect("item")
    }

    fn descriptor(component: &str, stage: Stage) -> MiddlewareDescriptor {
        MiddlewareDescriptor {
            invocation: finstack_ai_kernel::ComponentInvocation {
                component: finstack_ai_kernel::ComponentId::parse(component).expect("component"),
                version: Version {
                    major: 1,
                    minor: 0,
                    patch: 0,
                },
                configuration_digest: Digest::raw_json(b"{}"),
                recovery: finstack_ai_kernel::InvocationRecovery::RecomputeSafe,
            },
            stages: StageMask::from_stages([stage]),
            order: MiddlewareOrder {
                tier: OrderTier::Standard,
                priority: 0,
                before: Arc::from([]),
                after: Arc::from([]),
            },
            role: MiddlewareRole::Standard,
            metadata: Metadata::empty(),
        }
    }

    /// A component that always returns one fixed outcome at one stage.
    struct Fixed {
        descriptor: MiddlewareDescriptor,
        outcome: StageOutcome,
    }

    impl crate::middleware::Middleware for Fixed {
        fn descriptor(&self) -> MiddlewareDescriptor {
            self.descriptor.clone()
        }

        fn invoke(
            &self,
            _ctx: crate::middleware::MiddlewareContext,
            _input: crate::middleware::StageInput,
        ) -> PortFuture<Result<StageOutcome, crate::middleware::MiddlewareError>> {
            let outcome = self.outcome.clone();
            Box::pin(async move { Ok(outcome) })
        }
    }

    fn driver_for(component: &str, stage: Stage, outcome: StageOutcome) -> StageDriver {
        let middleware: Arc<dyn crate::middleware::Middleware> = Arc::new(Fixed {
            descriptor: descriptor(component, stage),
            outcome,
        });
        StageDriver::new(
            Arc::new(
                ResolvedMiddlewareChain::try_new(vec![MiddlewareRegistration { middleware }])
                    .expect("chain"),
            ),
            CancellationSignal::new(),
        )
    }

    /// Deterministic, collision-free random source.
    #[derive(Default)]
    struct CountingRandom(AtomicU64);

    impl RandomSource for CountingRandom {
        fn fill_bytes(&self, buf: &mut [u8]) -> Result<(), IdGenerationError> {
            let counter = self.0.fetch_add(1, Ordering::Relaxed);
            let bytes = counter.to_be_bytes();
            for (index, slot) in buf.iter_mut().enumerate() {
                *slot = bytes[index % bytes.len()];
            }
            Ok(())
        }
    }

    fn test_sources() -> SettlementSources<ExternalClock, CountingRandom> {
        SettlementSources::try_new(
            ExternalClock::new(timestamp(1_000)),
            CountingRandom::default(),
        )
        .expect("sources")
    }

    struct MemoryStore {
        inner: Mutex<MemoryInner>,
    }

    struct MemoryInner {
        batches: Vec<CommittedBatch>,
        requests: BTreeMap<AppendBatchId, AppendRequest>,
    }

    impl MemoryStore {
        fn new() -> Self {
            Self {
                inner: Mutex::new(MemoryInner {
                    batches: Vec::new(),
                    requests: BTreeMap::new(),
                }),
            }
        }
    }

    fn commit_request(request: &AppendRequest) -> CommittedBatch {
        let records = request
            .records()
            .iter()
            .enumerate()
            .map(|(offset, draft)| {
                let sequence = request.expected_sequence() + u64::try_from(offset).expect("offset");
                RecordEnvelope::try_new(
                    draft.format_version(),
                    draft.kind_version(),
                    draft.record_id(),
                    draft.session_id(),
                    draft.lane_id(),
                    draft.run_id(),
                    sequence,
                    draft.timestamp(),
                    None,
                    Digest::raw_json(format!("payload-{sequence}").as_bytes()),
                    None,
                    Digest::raw_json(format!("checksum-{sequence}").as_bytes()),
                    draft.derived_event_ids().to_vec(),
                    draft.body().clone(),
                )
                .expect("envelope")
            })
            .collect::<Vec<_>>();
        CommittedBatch::try_new(
            request.batch_id(),
            request.expected_sequence(),
            request.expected_sequence() + u64::try_from(records.len()).expect("count") - 1,
            records,
        )
        .expect("committed batch")
    }

    impl JournalStore for MemoryStore {
        fn append(&self, request: AppendRequest) -> PortFuture<Result<CommittedBatch, StoreError>> {
            let mut inner = self.inner.lock().expect("lock");
            if let Some(existing) = inner.requests.get(&request.batch_id()) {
                if existing != &request {
                    return Box::pin(async {
                        Err(StoreError::Corruption {
                            reason_code: "batch_reuse",
                        })
                    });
                }
                let committed = inner
                    .batches
                    .iter()
                    .find(|batch| batch.batch_id == request.batch_id())
                    .cloned()
                    .expect("indexed batch");
                return Box::pin(async move { Ok(committed) });
            }
            let committed = commit_request(&request);
            inner.requests.insert(request.batch_id(), request);
            inner.batches.push(committed.clone());
            Box::pin(async move { Ok(committed) })
        }

        fn load(&self, request: LoadRequest) -> PortFuture<Result<LoadedSession, StoreError>> {
            let inner = self.inner.lock().expect("lock");
            let batches = inner.batches.clone();
            let head_sequence = batches.last().map_or(0, |batch| batch.last_sequence);
            Box::pin(async move {
                Ok(LoadedSession {
                    session_id: request.session_id,
                    head_sequence,
                    head_checksum: batches
                        .last()
                        .and_then(|batch| batch.records.last().map(RecordEnvelope::checksum)),
                    metadata: Metadata::empty(),
                    committed_batches: batches.into(),
                    snapshot: None,
                    accelerated: None,
                })
            })
        }

        fn write_snapshot(
            &self,
            _request: SnapshotRequest,
        ) -> PortFuture<Result<SnapshotReceipt, StoreError>> {
            Box::pin(async {
                Err(StoreError::Unavailable {
                    reason_code: "not_used",
                })
            })
        }

        fn health(&self) -> PortFuture<Result<StoreHealth, StoreError>> {
            Box::pin(async {
                Ok(StoreHealth {
                    ready: true,
                    durable: false,
                    detail: Arc::from("test"),
                })
            })
        }
    }

    /// An accepted, `BeforeRun`-phase coordinator over an in-memory journal.
    fn accepted_coordinator(limits: RunLimits) -> CommitCoordinator {
        let mut coordinator = CommitCoordinator::new(Arc::new(MemoryStore::new()));
        block_on(coordinator.submit(
            env(1_000, &[1], &[1], &[], &[], &[], &[], 101),
            accept_input(limits),
        ))
        .expect("accept");
        coordinator
    }

    /// Drive `coordinator` through `BeforeRun` so the next cursor is
    /// `PrepareContext`.
    fn drive_to_prepare_context(coordinator: &mut CommitCoordinator) {
        block_on(coordinator.submit(
            env(1_100, &[2], &[], &[], &[], &[], &[], 102),
            KernelInput::StageSettled(StageSettled {
                cursor: StageCursor {
                    cycle: 0,
                    stage: Stage::BeforeRun,
                },
                outcome: ReducerStageOutcome::Continue,
            }),
        ))
        .expect("before run");
    }

    fn committed_record_ids(outcome: &crate::CommitOutcome) -> Vec<finstack_ai_kernel::RecordId> {
        outcome
            .committed
            .as_ref()
            .expect("committed batch")
            .records
            .iter()
            .map(RecordEnvelope::record_id)
            .collect()
    }

    fn before_run_settled() -> StageSettled {
        StageSettled {
            cursor: StageCursor {
                cycle: 0,
                stage: Stage::BeforeRun,
            },
            outcome: ReducerStageOutcome::Continue,
        }
    }

    // ---- passthrough invariant -------------------------------------------

    #[test]
    fn no_chain_installed_submits_the_facade_input_unchanged() {
        let mut coordinator = accepted_coordinator(RunLimits::empty());
        let sources = test_sources();
        let facade_env = env(1_100, &[2], &[], &[], &[], &[], &[], 102);

        let outcome = block_on(settle_facade_stage(
            &mut coordinator,
            None,
            &sources,
            facade_env,
            before_run_settled(),
        ))
        .expect("passthrough");

        assert_eq!(
            committed_record_ids(&outcome),
            vec![id(2)],
            "passthrough must commit the facade's own pre-minted record id"
        );
    }

    #[test]
    fn inactive_stage_submits_the_facade_input_unchanged() {
        // The facade installs the chain unconditionally, so `middleware_chain()`
        // is always Some in a facade-started run. Passthrough must key off
        // `is_active(stage)`, not off the Option.
        let mut coordinator = accepted_coordinator(RunLimits::empty());
        let sources = test_sources();
        let driver = driver_for(
            "fixture.prepare-only",
            Stage::PrepareContext,
            StageOutcome::AddContext(Arc::from([item("never-runs")])),
        );
        assert!(!driver.is_active(Stage::BeforeRun));

        let outcome = block_on(settle_facade_stage(
            &mut coordinator,
            Some(&driver),
            &sources,
            env(1_100, &[2], &[], &[], &[], &[], &[], 102),
            before_run_settled(),
        ))
        .expect("passthrough");

        assert_eq!(
            committed_record_ids(&outcome),
            vec![id(2)],
            "an inactive stage must reuse the facade's env byte for byte"
        );
    }

    #[test]
    fn identity_fold_submits_the_facade_input_unchanged() {
        let mut coordinator = accepted_coordinator(RunLimits::empty());
        let sources = test_sources();
        let driver = driver_for("fixture.passive", Stage::BeforeRun, StageOutcome::Continue);
        assert!(driver.is_active(Stage::BeforeRun));

        let outcome = block_on(settle_facade_stage(
            &mut coordinator,
            Some(&driver),
            &sources,
            env(1_100, &[2], &[], &[], &[], &[], &[], 102),
            before_run_settled(),
        ))
        .expect("identity fold");

        assert_eq!(
            committed_record_ids(&outcome),
            vec![id(2)],
            "an all-Continue chain must not perturb the facade's submission"
        );
    }

    #[test]
    fn before_model_is_passthrough_until_the_typed_stage_input_lands() {
        // Task 8 owns `StageInput::BeforeModel(Box<BeforeModelInput>)`. Until it
        // lands, BeforeModel is a documented passthrough at this choke point.
        let mut coordinator = accepted_coordinator(RunLimits::empty());
        drive_to_prepare_context(&mut coordinator);
        block_on(coordinator.submit(
            env(1_200, &[3, 4], &[], &[], &[101], &[], &[], 103),
            KernelInput::StageSettled(StageSettled {
                cursor: StageCursor {
                    cycle: 0,
                    stage: Stage::PrepareContext,
                },
                outcome: ReducerStageOutcome::ContextPrepared {
                    messages: Arc::from([user_message(4, "hi")]),
                },
            }),
        ))
        .expect("context");
        let sources = test_sources();
        let driver = driver_for(
            "fixture.model",
            Stage::BeforeModel,
            StageOutcome::AddContext(Arc::from([item("would-be-injected")])),
        );

        let outcome = block_on(settle_facade_stage(
            &mut coordinator,
            Some(&driver),
            &sources,
            env(1_300, &[5, 6], &[2], &[103], &[], &[102], &[], 104),
            StageSettled {
                cursor: StageCursor {
                    cycle: 0,
                    stage: Stage::BeforeModel,
                },
                outcome: ReducerStageOutcome::ModelRequestPrepared {
                    request: RawJson::parse(br#"{"messages":[]}"#).expect("request"),
                    component: None,
                    output_contract: finstack_ai_kernel::EffectOutputContract {
                        kind: finstack_ai_kernel::EffectOutputKind::ModelResponse,
                        schema_version: 1,
                        schema_digest: Digest::raw_json(br#"{"type":"model_response"}"#),
                    },
                    retry_safety: finstack_ai_kernel::RetrySafety::SafeToRetry,
                    deadline: None,
                },
            },
        ))
        .expect("passthrough");

        assert_eq!(
            committed_record_ids(&outcome),
            vec![id(5), id(6)],
            "BeforeModel must still reuse the facade's env until Task 8"
        );
    }

    // ---- PrepareContext fold ---------------------------------------------

    #[test]
    fn prepare_context_middleware_adds_messages_to_the_committed_outcome() {
        let mut coordinator = accepted_coordinator(RunLimits::empty());
        drive_to_prepare_context(&mut coordinator);
        let sources = test_sources();
        let driver = driver_for(
            "fixture.context",
            Stage::PrepareContext,
            StageOutcome::AddContext(Arc::from([item("injected-by-middleware")])),
        );

        block_on(settle_facade_stage(
            &mut coordinator,
            Some(&driver),
            &sources,
            env(1_200, &[3, 4], &[], &[], &[101], &[], &[], 103),
            StageSettled {
                cursor: StageCursor {
                    cycle: 0,
                    stage: Stage::PrepareContext,
                },
                outcome: ReducerStageOutcome::ContextPrepared {
                    messages: Arc::from([user_message(4, "hi")]),
                },
            },
        ))
        .expect("settles");

        let committed = coordinator
            .state()
            .current_turn
            .as_ref()
            .expect("current turn");
        assert!(
            committed
                .context
                .messages
                .iter()
                .any(|message| message_text(message).contains("injected-by-middleware")),
            "middleware context never reached the committed ContextPrepared"
        );
        assert!(
            committed
                .context
                .messages
                .iter()
                .any(|message| message_text(message) == "hi"),
            "the facade's own base message must survive the fold"
        );
    }

    // ---- replacement vs additive precedence ------------------------------

    #[test]
    fn replacement_rebases_and_additive_contributions_apply_on_top() {
        let sources = test_sources();
        let canonical =
            serde_json_canonicalizer::to_vec(&[user_message(11, "replaced")]).expect("canonical");
        let replacement = RawJson::parse(&canonical).expect("raw json");
        let fold = StageFold {
            replacement: Some(replacement),
            context: vec![item("added-after-replace")],
            ..StageFold::default()
        };

        let applied =
            apply_context_prepared(&fold, &[user_message(4, "base")], &sources).expect("applied");

        let texts = applied.iter().map(message_text).collect::<Vec<_>>();
        assert_eq!(
            texts,
            vec!["replaced".to_owned(), "added-after-replace".to_owned()],
            "Replace rebases the base payload; additive contributions still land, after it"
        );
    }

    #[test]
    fn additive_contributions_append_after_the_base_when_nothing_replaced() {
        let sources = test_sources();
        let fold = StageFold {
            instructions: vec![item("system-add")],
            context: vec![item("context-add")],
            ..StageFold::default()
        };

        let applied =
            apply_context_prepared(&fold, &[user_message(4, "base")], &sources).expect("applied");

        assert_eq!(
            applied.iter().map(message_text).collect::<Vec<_>>(),
            vec![
                "base".to_owned(),
                "system-add".to_owned(),
                "context-add".to_owned()
            ]
        );
        assert_eq!(applied[1].role(), MessageRole::System);
        assert_eq!(applied[2].role(), MessageRole::User);
    }

    #[test]
    fn oversized_context_fold_is_a_stable_bounds_error() {
        let sources = test_sources();
        let fold = StageFold {
            context: (0..finstack_ai_kernel::SEMANTIC_ARRAY_MAX_ITEMS)
                .map(|_| item("x"))
                .collect(),
            ..StageFold::default()
        };

        let error = apply_context_prepared(&fold, &[user_message(4, "base")], &sources)
            .expect_err("base + additions exceed the semantic array bound");
        assert!(
            matches!(&error, RunHandleError::Middleware { code }
                if code.as_ref() == crate::middleware_driver::MIDDLEWARE_STAGE_BOUNDS_EXCEEDED),
            "expected a stable bounds error, got {error:?}"
        );
    }

    // ---- stage inputs -----------------------------------------------------

    fn assistant_message(ordinal: u64, text: &str) -> Message {
        Message::try_new(
            id(ordinal),
            MessageRole::Assistant,
            vec![ContentBlock::Text(TextBlock::try_new(text).expect("text"))],
            timestamp(900),
            None,
            ProviderIds::empty(),
            Metadata::empty(),
        )
        .expect("message")
    }

    fn state_with_messages(messages: Vec<Message>) -> finstack_ai_kernel::KernelState {
        finstack_ai_kernel::KernelState {
            messages: Arc::new(messages),
            ..finstack_ai_kernel::KernelState::default()
        }
    }

    #[test]
    fn trailing_role_run_selects_only_the_final_contiguous_block() {
        let messages = vec![
            user_message(1, "u1"),
            assistant_message(2, "a1"),
            assistant_message(3, "a2"),
        ];
        let run = trailing_role_run(&messages, MessageRole::Assistant);
        assert_eq!(
            run.iter().map(message_text).collect::<Vec<_>>(),
            vec!["a1".to_owned(), "a2".to_owned()],
            "an earlier User message must terminate the trailing Assistant run"
        );
        assert!(
            trailing_role_run(&messages, MessageRole::Tool).is_empty(),
            "no trailing Tool message means an empty run, not the whole history"
        );
        assert!(trailing_role_run(&[], MessageRole::Tool).is_empty());
    }

    #[test]
    fn after_model_stage_input_is_the_latest_message() {
        let state = state_with_messages(vec![user_message(1, "u1"), assistant_message(2, "a1")]);
        let input = stage_input(&state, Stage::AfterModel, &ReducerStageOutcome::Continue)
            .expect("after model input");
        assert_eq!(input.stage(), Stage::AfterModel);
        let StageInput::AfterModel { value } = input else {
            panic!("wrong variant");
        };
        let decoded: Message = serde_json::from_slice(value.as_bytes()).expect("one message");
        assert_eq!(message_text(&decoded), "a1");
    }

    #[test]
    fn after_model_stage_input_without_a_message_is_a_stable_error() {
        let error = stage_input(
            &state_with_messages(Vec::new()),
            Stage::AfterModel,
            &ReducerStageOutcome::Continue,
        )
        .expect_err("no message to observe");
        assert!(matches!(&error, RunHandleError::Middleware { code }
            if code.as_ref() == "middleware_stage_input_invalid"));
    }

    #[test]
    fn after_tool_batch_stage_input_is_the_trailing_tool_result_run() {
        let state = state_with_messages(vec![user_message(1, "u1"), assistant_message(2, "a1")]);
        let input = stage_input(
            &state,
            Stage::AfterToolBatch,
            &ReducerStageOutcome::Continue,
        )
        .expect("after tool batch input");
        let StageInput::AfterToolBatch { value } = input else {
            panic!("wrong variant");
        };
        let decoded: Vec<Message> = serde_json::from_slice(value.as_bytes()).expect("array");
        assert!(
            decoded.is_empty(),
            "with no trailing tool results the array is empty, not the whole history"
        );
    }

    #[test]
    fn before_finalize_stage_input_is_the_live_terminal_candidate() {
        let candidate = finstack_ai_kernel::TerminalCandidate::Completed {
            cycle: 0,
            turn_id: id(20),
            model_request_id: id(21),
            effect_id: id(22),
            message_id: id(23),
            result_digest: Digest::raw_json(b"{}"),
        };
        let state = finstack_ai_kernel::KernelState {
            terminal_candidate: Some(candidate.clone()),
            ..finstack_ai_kernel::KernelState::default()
        };

        let input = stage_input(
            &state,
            Stage::BeforeFinalize,
            &ReducerStageOutcome::FinalizeAccepted,
        )
        .expect("before finalize input");
        let StageInput::BeforeFinalize { candidate: value } = input else {
            panic!("wrong variant");
        };
        let decoded: finstack_ai_kernel::TerminalCandidate =
            serde_json::from_slice(value.as_bytes()).expect("candidate");
        assert_eq!(decoded, candidate);

        let error = stage_input(
            &finstack_ai_kernel::KernelState::default(),
            Stage::BeforeFinalize,
            &ReducerStageOutcome::FinalizeAccepted,
        )
        .expect_err("no candidate to observe");
        assert!(matches!(&error, RunHandleError::Middleware { code }
            if code.as_ref() == "middleware_stage_input_invalid"));
    }

    #[test]
    fn stage_input_refuses_the_two_stages_this_choke_point_does_not_fold() {
        for stage in [Stage::BeforeModel, Stage::BeforeToolBatch] {
            assert!(
                !folds_at(stage),
                "{stage:?} must be excluded from the choke point's fold set"
            );
            assert!(
                stage_input(
                    &state_with_messages(Vec::new()),
                    stage,
                    &ReducerStageOutcome::Continue,
                )
                .is_err(),
                "{stage:?} has no untyped stage input to build here"
            );
        }
    }

    // ---- terminal folds ---------------------------------------------------

    #[test]
    fn fail_terminal_replaces_the_base_outcome_at_after_model() {
        let fold = StageFold {
            terminal: Some(StageTerminal::Fail(Box::new(
                ErrorDescriptor::new("boom", "fixture", ErrorCategory::Middleware, false)
                    .expect("descriptor"),
            ))),
            ..StageFold::default()
        };
        let sources = test_sources();

        let outcome = apply_fold(
            &fold,
            StageCursor {
                cycle: 0,
                stage: Stage::AfterModel,
            },
            ReducerStageOutcome::Continue,
            &sources,
        )
        .expect("applied");

        assert!(matches!(outcome, ReducerStageOutcome::Fail(ref d) if d.code.as_str() == "boom"));
    }

    #[test]
    fn a_non_terminal_fold_with_no_landing_is_rejected_not_dropped() {
        // FilterTools is port-legal at BeforeToolBatch and BeforeModel only, so
        // it can never reach `apply_fold` through the facade choke point — but
        // `run_stage_chain` is shared with the tool-batch hook, so the applier
        // must refuse a fold it cannot land rather than discard it.
        let fold = StageFold {
            retained_tools: Some(std::collections::BTreeSet::new()),
            ..StageFold::default()
        };
        let sources = test_sources();

        let error = apply_fold(
            &fold,
            StageCursor {
                cycle: 0,
                stage: Stage::AfterModel,
            },
            ReducerStageOutcome::Continue,
            &sources,
        )
        .expect_err("nothing at AfterModel can carry a tool filter");
        assert!(matches!(&error, RunHandleError::Middleware { code }
            if code.as_ref() == crate::middleware_driver::MIDDLEWARE_STAGE_UNLANDABLE));
    }

    #[test]
    fn an_identity_fold_returns_the_base_outcome_untouched() {
        let sources = test_sources();
        let outcome = apply_fold(
            &StageFold::default(),
            StageCursor {
                cycle: 0,
                stage: Stage::AfterToolBatch,
            },
            ReducerStageOutcome::Continue,
            &sources,
        )
        .expect("identity");
        assert_eq!(outcome, ReducerStageOutcome::Continue);
    }

    // ---- decide_limit interception ---------------------------------------

    /// The stage table and `decide_limit` demand *mutually exclusive* id bags
    /// once a limit is crossed, because `validate_allocated_ids`
    /// (`allocated_ids.rs:97-107`) rejects both under- and over-allocation.
    /// This pins that exclusivity directly against the kernel, so the two-shot
    /// probe in `submit_folded` cannot be "simplified" into one lookup.
    #[test]
    fn the_stage_table_and_the_limit_requirement_are_mutually_exclusive() {
        let sources = test_sources();
        let state = finstack_ai_kernel::KernelState {
            session_id: Some(id::<SessionTag>(1)),
            lane_id: Some(id::<LaneTag>(2)),
            accepted: Some(acceptance(RunLimits {
                max_retries: Some(0),
                ..RunLimits::empty()
            })),
            accepted_at: Some(timestamp(1_000)),
            phase: Some(finstack_ai_kernel::RunPhase::BeforeFinalize),
            terminal_candidate: Some(finstack_ai_kernel::TerminalCandidate::Completed {
                cycle: 0,
                turn_id: id(20),
                model_request_id: id(21),
                effect_id: id(22),
                message_id: id(23),
                result_digest: Digest::raw_json(b"{}"),
            }),
            ..finstack_ai_kernel::KernelState::default()
        };
        let cursor = StageCursor {
            cycle: 0,
            stage: Stage::BeforeFinalize,
        };
        let outcome = ReducerStageOutcome::Retry(
            finstack_ai_kernel::RetryDirective::try_new(
                finstack_ai_kernel::RetryClassification::Framework,
                finstack_ai_kernel::Duration::from_millis(1),
                "fixture-policy-v1",
            )
            .expect("directive"),
        );
        let input = KernelInput::StageSettled(StageSettled {
            cursor,
            outcome: outcome.clone(),
        });

        let table = crate::settlement::stage_allocation(&state, cursor, &outcome, &sources)
            .expect("stage table allocation");
        assert_eq!(
            table.record_ids().len(),
            3,
            "Retry's stage tuple is (3,1,1)"
        );
        let rejected = finstack_ai_kernel::Kernel::try_restore(state.clone())
            .expect("restore")
            .decide(
                &TransitionEnv {
                    now: timestamp(1_500),
                    ids: table,
                },
                input.clone(),
            );
        assert!(
            rejected.is_err(),
            "the stage tuple must NOT satisfy decide_limit once max_retries is crossed: {rejected:?}"
        );

        let accepted = finstack_ai_kernel::Kernel::try_restore(state)
            .expect("restore")
            .decide(
                &TransitionEnv {
                    now: timestamp(1_500),
                    ids: limit_crossing_allocation(&sources).expect("limit allocation"),
                },
                input,
            );
        assert!(
            accepted.is_ok(),
            "the fixed limit-crossing bag must be the one that lands: {accepted:?}"
        );
    }

    /// End to end through the choke point: a `PrepareContext` fold submitted
    /// against `max_turns = 0`. `decide_limit` increments `usage.turns` from
    /// the submitted `ContextPrepared` and intercepts, so the stage table's
    /// `(2, 0, 0, 1, 0, 0)` is rejected and the fallback bag has to carry the
    /// settlement. Without the fallback this returns
    /// `Coordinator(Decision { code: "unused_allocated_ids" })`.
    #[test]
    fn a_fold_that_crosses_a_limit_still_lands_through_the_choke_point() {
        let mut coordinator = accepted_coordinator(RunLimits {
            max_turns: Some(0),
            ..RunLimits::empty()
        });
        drive_to_prepare_context(&mut coordinator);
        let sources = test_sources();
        let driver = driver_for(
            "fixture.context",
            Stage::PrepareContext,
            StageOutcome::AddContext(Arc::from([item("pushes-context-over")])),
        );

        block_on(settle_facade_stage(
            &mut coordinator,
            Some(&driver),
            &sources,
            env(1_200, &[3, 4], &[], &[], &[101], &[], &[], 103),
            StageSettled {
                cursor: StageCursor {
                    cycle: 0,
                    stage: Stage::PrepareContext,
                },
                outcome: ReducerStageOutcome::ContextPrepared {
                    messages: Arc::from([user_message(4, "hi")]),
                },
            },
        ))
        .expect("the limit-crossing fold must still commit");

        assert!(
            matches!(
                coordinator.state().terminal,
                Some(finstack_ai_kernel::TerminalState::Failed(_))
            ),
            "decide_limit must have terminated the run, not been bypassed: {:?}",
            coordinator.state().terminal
        );
    }
}
