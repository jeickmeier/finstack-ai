use std::sync::Arc;

use finstack_ai_kernel::{
    KernelInput, ReducerStageOutcome, Stage, StageCursor, StageSettled, TransitionEnv,
};

use crate::context_driver::{ContextDriver, collect_context_stage};
use crate::coordinator::CommitCoordinator;
use crate::middleware::StageInput;
use crate::middleware_driver::{
    MiddlewareStageContext, StageDriver, StageFold, derived_stage_effect_id,
};
use crate::model::LockedModelContextProfile;
use crate::run_types::RunHandleError;
use crate::settlement::SettlementSources;
use crate::{Clock, CommitOutcome, RandomSource, RunCallContext};

use super::apply::apply_fold;
use super::codec::{canonical_draft, parse_draft};
use super::input::stage_input;
use super::submit::{submit_folded, submit_settled};
use super::{MIDDLEWARE_STAGE_IDENTITY_MISSING, middleware_error, stage_error};

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

/// Build the run's context driver from the providers the facade installed.
///
/// `None` means no provider list was installed. An installed empty list still
/// produces a driver so `BeforeModel` can apply the structural protected
/// projection (system/developer and the trailing current user).
pub(crate) fn context_driver(
    coordinator: &CommitCoordinator,
    cancellation: &crate::CancellationSignal,
) -> Option<ContextDriver> {
    coordinator
        .context_providers()
        .map(|providers| ContextDriver::new(Arc::clone(providers), cancellation.child()))
}

/// Whether [`settle_facade_stage`] folds a chain at `stage`.
///
/// `BeforeToolBatch` is the sole exclusion — it never reaches this choke point.
/// See the module docs.
pub(super) const fn folds_at(stage: Stage) -> bool {
    matches!(
        stage,
        Stage::BeforeRun
            | Stage::PrepareContext
            | Stage::BeforeModel
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
    profile: &LockedModelContextProfile,
    env: TransitionEnv,
    input: KernelInput,
) -> Result<CommitOutcome, RunHandleError> {
    match input {
        KernelInput::StageSettled(settled) => {
            settle_facade_stage(coordinator, driver, sources, profile, env, settled).await
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
/// that stage, when the base outcome is not one the stage can fold on top of
/// (see [`foldable_base`]), or when the chain's fold is the identity. In all
/// five cases the facade's `env` — its `now` and its pre-minted id bags — is
/// reused verbatim.
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
    profile: &LockedModelContextProfile,
    env: TransitionEnv,
    mut settled: StageSettled,
) -> Result<CommitOutcome, RunHandleError> {
    let cursor = settled.cursor;
    if matches!(cursor.stage, Stage::PrepareContext | Stage::BeforeModel) {
        apply_context_providers(coordinator, driver, sources, profile, &mut settled).await?;
    }
    let Some(driver) = driver.filter(|driver| {
        folds_at(cursor.stage)
            && driver.is_active(cursor.stage)
            && foldable_base(cursor.stage, &settled.outcome)
    }) else {
        return submit_settled(coordinator, env, settled).await;
    };
    let input = stage_input(
        coordinator.state(),
        cursor.stage,
        &settled.outcome,
        profile,
        coordinator.context_projection(),
    )?;
    let fold = run_stage_chain(coordinator, Some(driver), cursor, input).await?;
    if fold.is_identity() {
        return submit_settled(coordinator, env, settled).await;
    }
    let outcome = apply_fold(&fold, cursor, settled.outcome, sources)?;
    submit_folded(coordinator, sources, env.now, cursor, outcome).await
}

async fn apply_context_providers<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    stage_driver: Option<&StageDriver>,
    sources: &SettlementSources<C, R>,
    profile: &LockedModelContextProfile,
    settled: &mut StageSettled,
) -> Result<(), RunHandleError> {
    if settled.cursor.stage == Stage::BeforeModel && coordinator.context_projection().is_some() {
        return Ok(());
    }
    let cancellation = stage_driver.map_or_else(crate::CancellationSignal::new, |driver| {
        driver.cancellation().child()
    });
    let Some(driver) = context_driver(coordinator, &cancellation) else {
        return Ok(());
    };
    let base_messages = match &settled.outcome {
        ReducerStageOutcome::ContextPrepared { messages } => messages.to_vec(),
        ReducerStageOutcome::ModelRequestPrepared { request, .. } => {
            parse_draft(request)?.messages.to_vec()
        }
        _ => coordinator.state().messages.to_vec(),
    };
    let plan = collect_context_stage(
        coordinator,
        &driver,
        sources,
        profile,
        settled.cursor.cycle,
        &base_messages,
    )
    .await?;
    coordinator.store_context_projection(plan.projection.into_map());
    match (&mut settled.outcome, settled.cursor.stage) {
        (ReducerStageOutcome::ContextPrepared { messages }, Stage::PrepareContext) => {
            *messages = plan.messages;
        }
        (ReducerStageOutcome::ModelRequestPrepared { request, .. }, Stage::BeforeModel) => {
            let mut draft = parse_draft(request)?;
            draft.messages = plan.messages;
            *request = canonical_draft(&draft)?;
        }
        _ => {}
    }
    Ok(())
}

/// Whether the facade's base outcome at `stage` is one a chain can fold on top
/// of.
///
/// Only `BeforeModel` narrows here, and it must. The kernel admits exactly two
/// outcomes at that cursor — `ModelRequestPrepared` (`decide.rs:1134-1141`) and
/// `Fail` (`decide.rs:1165-1177`) — and only the first carries the
/// [`ModelRequestDraft`] that *is* the whole payload of
/// `StageInput::BeforeModel`. Without this gate a `Fail` settled at
/// `BeforeModel` would reach [`before_model_input`], which has no draft to
/// assemble from and returns [`MIDDLEWARE_STAGE_INPUT_INVALID`] — so a
/// kernel-admissible, deliberate stage failure would stop landing the moment a
/// component was registered at the stage, even a purely observational one that
/// would have folded to the identity. That is the exact inverse of the
/// governing passthrough invariant, so the settlement passes through instead
/// and the run fails as it would with no middleware installed.
///
/// Every other stage returns `true`: their inputs are built from live
/// `KernelState` rather than from the base outcome, so no base outcome can
/// starve them at [`stage_input`].
///
/// # Known residual gap at `PrepareContext`
///
/// The gate is deliberately narrower than the failure mode it is named for, and
/// `PrepareContext` keeps a smaller version of the same problem. `stage_input`
/// is fine there — a non-`ContextPrepared` base falls back to `state.messages`
/// rather than failing. The gap is one function later, in [`apply_fold`]: a
/// `Fail` base at `PrepareContext` (kernel-admissible, `decide.rs:1165-1177`)
/// combined with an *additive* component produces a non-identity fold that
/// matches neither the `ContextPrepared` nor the `ModelRequestPrepared` arm, so
/// it falls to `_ => Err(MIDDLEWARE_STAGE_UNLANDABLE)` and the deliberate `Fail`
/// still does not land.
///
/// It is materially weaker than the `BeforeModel` case this gate fixes, which
/// is why the gate was not widened to match:
///
/// - A *passive* component is harmless. The fold is the identity, and
///   [`settle_facade_stage`] short-circuits to passthrough before [`apply_fold`]
///   is reached, so merely registering an observational component cannot break a
///   `Fail`. At `BeforeModel` the equivalent case *did* break, because the input
///   assembly failed before any component ran.
/// - No in-tree producer reaches it. `Agent::run` settles only
///   `ContextPrepared` at this cursor (`agent.rs:788-795`); an out-of-tree
///   `KernelInput::StageSettled` producer is required to construct the base.
///
/// Widening `foldable_base` is **not** the fix if it is ever hit: it would make
/// an additive component silently no-op instead of failing loudly. The fix is an
/// [`apply_fold`] arm that decides, explicitly, that a terminal base outcome
/// wins over additive contributions at every stage.
fn foldable_base(stage: Stage, outcome: &ReducerStageOutcome) -> bool {
    stage != Stage::BeforeModel
        || matches!(outcome, ReducerStageOutcome::ModelRequestPrepared { .. })
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
        .run_stage_masked(&ctx, input, |component| {
            coordinator.component_is_active(component)
        })
        .await
        .map_err(|error| middleware_error(&error))?;
    StageFold::accumulate(cursor.stage, &outcomes).map_err(|error| middleware_error(&error))
}
