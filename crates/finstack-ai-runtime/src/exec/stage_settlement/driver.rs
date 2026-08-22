use std::sync::Arc;

use finstack_ai_kernel::{
    AllocatedIds, AppendBatchTag, EffectCompleted, EffectFailed, EffectInput, EffectKind,
    EffectOutputContract, EffectOutputKind, EffectRequested, EffectTag, EventTag,
    ExtensionEffectSettled, ExtensionSettlement, KernelInput, PipelinePosition, ProviderIds,
    RECORD_KIND_VERSION, RecordBody, RecordTag, ReducerStageOutcome, RequestExtensionEffect,
    RetrySafety, Stage, StageCursor, StageSettled, TransitionEnv,
};

use crate::commit::CommitOutcome;
use crate::compaction_driver::{
    first_compaction_request, fulfill_compaction_model, load_completed_compaction_resume,
};
use crate::context_driver::{ContextDriver, collect_context_stage};
use crate::coordinator::CommitCoordinator;
use crate::ids::{Clock, RandomSource};
use crate::middleware::{
    CompactionModelResume, MIDDLEWARE_RESOLUTION_INVALID, MiddlewareRole, ResolvedMiddleware,
    StageInput, StageOutcome, stage_name, validate_stage_outcome,
};
use crate::middleware_driver::{StageDriver, StageFold};
use crate::model::{LockedModelContextProfile, Model};
use crate::ports::model::RunCallContext;
use crate::run_types::RunHandleError;
use crate::settlement::SettlementSources;

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
    cancellation: &crate::ports::model::CancellationSignal,
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
    cancellation: &crate::ports::model::CancellationSignal,
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
    model: Option<&dyn Model>,
) -> Result<CommitOutcome, RunHandleError> {
    match input {
        KernelInput::StageSettled(settled) => {
            settle_facade_stage_with_model(
                coordinator,
                driver,
                sources,
                profile,
                env,
                settled,
                model,
            )
            .await
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
#[cfg(test)]
pub(crate) async fn settle_facade_stage<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    driver: Option<&StageDriver>,
    sources: &SettlementSources<C, R>,
    profile: &LockedModelContextProfile,
    env: TransitionEnv,
    settled: StageSettled,
) -> Result<CommitOutcome, RunHandleError> {
    settle_facade_stage_with_model(coordinator, driver, sources, profile, env, settled, None).await
}

/// Fold a facade stage, optionally fulfilling a runtime-owned compaction model.
///
/// When `model` is `None`, `RequestCompactionModel` still reaches
/// [`StageFold::accumulate`] and stays `middleware_stage_unlandable`.
pub(crate) async fn settle_facade_stage_with_model<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    driver: Option<&StageDriver>,
    sources: &SettlementSources<C, R>,
    profile: &LockedModelContextProfile,
    env: TransitionEnv,
    mut settled: StageSettled,
    model: Option<&dyn Model>,
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
    let mut resume = if cursor.stage == Stage::BeforeModel {
        load_completed_compaction_resume(coordinator, cursor.cycle).await?
    } else {
        None
    };
    let mut outcomes = invoke_stage_chain(
        coordinator,
        driver,
        sources,
        cursor,
        input.clone(),
        resume.clone(),
    )
    .await?;
    if cursor.stage == Stage::BeforeModel
        && let Some(request) = first_compaction_request(&outcomes)
        && let Some(model) = model
    {
        resume = Some(
            fulfill_compaction_model(
                coordinator,
                sources,
                profile,
                model,
                request,
                cursor.cycle,
                driver.cancellation(),
            )
            .await?,
        );
        outcomes = invoke_stage_chain(coordinator, driver, sources, cursor, input, resume).await?;
    }
    let fold =
        StageFold::accumulate(cursor.stage, &outcomes).map_err(|error| middleware_error(&error))?;
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
    let cancellation = stage_driver
        .map_or_else(crate::ports::model::CancellationSignal::new, |driver| {
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
        settled.cursor,
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
pub(crate) async fn run_stage_chain<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    driver: Option<&StageDriver>,
    sources: &SettlementSources<C, R>,
    cursor: StageCursor,
    input: StageInput,
) -> Result<StageFold, RunHandleError> {
    let Some(driver) = driver.filter(|driver| driver.is_active(cursor.stage)) else {
        return Ok(StageFold::default());
    };
    let outcomes = invoke_stage_chain(coordinator, driver, sources, cursor, input, None).await?;
    StageFold::accumulate(cursor.stage, &outcomes).map_err(|error| middleware_error(&error))
}

async fn invoke_stage_chain<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    driver: &StageDriver,
    sources: &SettlementSources<C, R>,
    cursor: StageCursor,
    input: StageInput,
    resume: Option<CompactionModelResume>,
) -> Result<Vec<crate::middleware::StageOutcome>, RunHandleError> {
    debug_assert_eq!(
        cursor.stage,
        input.stage(),
        "the cursor and the stage input must describe the same stage"
    );
    let seed = coordinator
        .stage_dispatch_seed()
        .ok_or_else(|| stage_error(MIDDLEWARE_STAGE_IDENTITY_MISSING))?;
    if driver.cancellation().is_cancelled() {
        return Ok(Vec::new());
    }
    let resume_json = resume
        .as_ref()
        .map(serde_json_canonicalizer::to_vec)
        .transpose()
        .map_err(|_| stage_error(crate::ports::middleware::MIDDLEWARE_OUTCOME_NOT_ALLOWED))?
        .map(finstack_ai_kernel::RawJson::parse)
        .transpose()
        .map_err(|_| stage_error(crate::ports::middleware::MIDDLEWARE_OUTCOME_NOT_ALLOWED))?;
    let durable_input = strip_disposable_input_checkpoint(input.clone());
    let input_json = durable_input
        .to_raw_json()
        .map_err(|error| middleware_error(&error))?;
    let invocation = StageInvocation {
        driver,
        sources,
        cursor,
        input: &input,
        resume: &resume,
        resume_json: &resume_json,
        input_json: &input_json,
        seed: &seed,
    };
    let mut outcomes = Vec::new();
    for (index, resolved) in driver.chain().stage(cursor.stage).iter().enumerate() {
        if !coordinator.component_is_active(&resolved.descriptor.invocation.component) {
            continue;
        }
        outcomes
            .push(invoke_middleware_component(coordinator, &invocation, index, resolved).await?);
    }
    Ok(outcomes)
}

/// Remove any caller-supplied checkpoint before constructing durable effect
/// input. Only the coordinator's validated process-local cache may populate a
/// direct compactor call.
fn strip_disposable_input_checkpoint(mut input: StageInput) -> StageInput {
    if let StageInput::BeforeModel(before_model) = &mut input {
        before_model.checkpoint = None;
    }
    input
}

struct StageInvocation<'a, C, R> {
    driver: &'a StageDriver,
    sources: &'a SettlementSources<C, R>,
    cursor: StageCursor,
    input: &'a StageInput,
    resume: &'a Option<CompactionModelResume>,
    resume_json: &'a Option<finstack_ai_kernel::RawJson>,
    input_json: &'a finstack_ai_kernel::RawJson,
    seed: &'a crate::coordinator::StageDispatchSeed,
}

async fn invoke_middleware_component<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    invocation: &StageInvocation<'_, C, R>,
    index: usize,
    resolved: &ResolvedMiddleware,
) -> Result<StageOutcome, RunHandleError> {
    let pipeline_index =
        u32::try_from(index).map_err(|_| stage_error(MIDDLEWARE_RESOLUTION_INVALID))?;
    let recovery = MiddlewareRecovery {
        cursor: invocation.cursor,
        component: &resolved.descriptor.invocation,
        chain_digest: invocation.driver.chain().digest(),
        run_id: invocation.seed.locator.run_id,
        pipeline_index,
        input: invocation.input_json,
        resume: invocation.resume_json.as_ref(),
    };
    if let Some((effect_id, outcome)) = recovered_middleware_outcome(coordinator, &recovery)? {
        if matches!(outcome, StageOutcome::RequestCompactionModel(_)) {
            coordinator.note_middleware_effect(effect_id);
        }
        return Ok(outcome);
    }
    let pending = coordinator
        .state()
        .pending_extension_effect
        .as_ref()
        .filter(|pending| {
            pending.cursor == invocation.cursor
                && pending.requested.component() == Some(&resolved.descriptor.invocation)
                && pending.requested.pipeline().is_some_and(|pipeline| {
                    pipeline.index() == pipeline_index
                        && pipeline.chain_digest() == invocation.driver.chain().digest()
                })
                && matches!(pending.requested.input(), EffectInput::Middleware { .. })
        });
    let effect_id = pending.map_or_else(
        || invocation.sources.generate::<EffectTag>(),
        |pending| Ok(pending.requested.effect_id()),
    )?;
    let requested = if let Some(pending) = pending {
        pending.requested.clone()
    } else {
        commit_middleware_request(coordinator, invocation, resolved, pipeline_index, effect_id)
            .await?
    };
    let context = crate::ports::middleware::MiddlewareContext {
        run: RunCallContext {
            effect_id,
            locator: invocation.seed.locator.clone(),
            authorization: invocation.seed.authorization.clone(),
            attempt: invocation.seed.attempt,
            deadline: invocation.seed.deadline,
            budget_scope_id: invocation.seed.budget_scope_id,
            cancellation: invocation.driver.cancellation().child(),
            relation_depth: invocation.seed.relation_depth,
        },
        chain_digest: invocation.driver.chain().digest(),
        chain_index: pipeline_index,
        compaction_resume: invocation.resume.clone(),
    };
    let component_input = component_input(coordinator, resolved, invocation.input);
    let result = match resolved
        .middleware
        .invoke(context, component_input.clone())
        .await
    {
        Ok(outcome) => validate_stage_outcome(&resolved.descriptor, &component_input, &outcome)
            .map(|()| outcome),
        Err(error) => Err(error),
    };
    let retained_checkpoint = match &result {
        Ok(StageOutcome::CompactContext(result)) => Some(result.checkpoint.clone()),
        _ => None,
    };
    let durable_result = result.clone().map(strip_disposable_checkpoint);
    let settlement = middleware_settlement(&requested, &durable_result)?;
    coordinator
        .submit(
            extension_settlement_env(invocation.sources, &settlement)?,
            KernelInput::ExtensionEffectSettled(ExtensionEffectSettled {
                cursor: invocation.cursor,
                outcome: settlement,
            }),
        )
        .await
        .map_err(RunHandleError::Coordinator)?;
    if let Some(checkpoint) = retained_checkpoint {
        coordinator.retain_compaction_checkpoint(checkpoint);
    }
    let outcome = result.map_err(|error| middleware_error(&error))?;
    if matches!(outcome, StageOutcome::RequestCompactionModel(_)) {
        coordinator.note_middleware_effect(requested.effect_id());
    }
    Ok(outcome)
}

/// Build the direct-call input for one component without changing the durable
/// effect input. A compatible checkpoint is process-local acceleration only:
/// it is never serialized into `EffectRequested`.
pub(super) fn component_input(
    coordinator: &mut CommitCoordinator,
    resolved: &ResolvedMiddleware,
    input: &StageInput,
) -> StageInput {
    let StageInput::BeforeModel(before_model) = input else {
        return input.clone();
    };
    if !matches!(
        resolved.descriptor.role,
        MiddlewareRole::ContextCompactor { .. }
    ) {
        return input.clone();
    }
    let mut before_model = before_model.as_ref().clone();
    before_model.checkpoint =
        coordinator.compatible_compaction_checkpoint(&resolved.descriptor, &before_model);
    StageInput::BeforeModel(Box::new(before_model))
}

/// Remove the disposable checkpoint before a middleware outcome crosses the
/// durable effect-settlement boundary. The validated live outcome still flows
/// into the current fold and its checkpoint is retained only in memory.
fn strip_disposable_checkpoint(mut outcome: StageOutcome) -> StageOutcome {
    if let StageOutcome::CompactContext(result) = &mut outcome {
        result.checkpoint = None;
    }
    outcome
}

async fn commit_middleware_request<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    invocation: &StageInvocation<'_, C, R>,
    resolved: &ResolvedMiddleware,
    pipeline_index: u32,
    effect_id: finstack_ai_kernel::EffectId,
) -> Result<EffectRequested, RunHandleError> {
    let requested = EffectRequested::try_new(
        effect_id,
        EffectKind::Middleware,
        None,
        Some(resolved.descriptor.invocation.clone()),
        Some(
            PipelinePosition::try_new(
                invocation.driver.chain().digest(),
                stage_name(invocation.cursor.stage),
                pipeline_index,
            )
            .map_err(|_| stage_error(MIDDLEWARE_RESOLUTION_INVALID))?,
        ),
        EffectOutputContract {
            kind: EffectOutputKind::MiddlewareOutcome,
            schema_version: 1,
            schema_digest: finstack_ai_kernel::Digest::raw_json(b"middleware-outcome-v1"),
        },
        EffectInput::Middleware {
            cursor: invocation.cursor,
            stage: Arc::from(stage_name(invocation.cursor.stage)),
            input: invocation.input_json.clone(),
            resume: invocation.resume_json.clone(),
        },
        RetrySafety::SafeToRetry,
        invocation.seed.deadline,
    )
    .map_err(|_| stage_error(MIDDLEWARE_RESOLUTION_INVALID))?;
    coordinator
        .submit(
            extension_request_env(invocation.sources, &requested)?,
            KernelInput::RequestExtensionEffect(RequestExtensionEffect {
                requested: requested.clone(),
            }),
        )
        .await
        .map_err(RunHandleError::Coordinator)?;
    Ok(requested)
}

struct MiddlewareRecovery<'a> {
    cursor: StageCursor,
    component: &'a finstack_ai_kernel::ComponentInvocation,
    chain_digest: finstack_ai_kernel::Digest,
    run_id: finstack_ai_kernel::RunId,
    pipeline_index: u32,
    input: &'a finstack_ai_kernel::RawJson,
    resume: Option<&'a finstack_ai_kernel::RawJson>,
}

fn recovered_middleware_outcome(
    coordinator: &CommitCoordinator,
    recovery: &MiddlewareRecovery<'_>,
) -> Result<Option<(finstack_ai_kernel::EffectId, StageOutcome)>, RunHandleError> {
    coordinator
        .replayed_completed_effects()
        .values()
        .find(|(requested, _)| {
            requested.component() == Some(recovery.component)
                && coordinator
                    .replayed_extension_envelope(requested.effect_id())
                    .is_some_and(|envelope| envelope.run_id() == Some(recovery.run_id))
                && requested.pipeline().is_some_and(|pipeline| {
                    pipeline.chain_digest() == recovery.chain_digest
                        && pipeline.index() == recovery.pipeline_index
                })
                && matches!(
                    requested.input(),
                    EffectInput::Middleware {
                        cursor: requested_cursor,
                        input,
                        resume,
                        ..
                    } if *requested_cursor == recovery.cursor
                        && input == recovery.input
                        && resume.as_ref() == recovery.resume
                )
        })
        .map(|(requested, completed)| {
            serde_json::from_slice(completed.output().as_bytes())
                .map(|outcome| (requested.effect_id(), outcome))
                .map_err(|_| stage_error(crate::ports::middleware::MIDDLEWARE_OUTCOME_NOT_ALLOWED))
        })
        .transpose()
}

fn middleware_settlement(
    requested: &EffectRequested,
    result: &Result<StageOutcome, crate::ports::middleware::MiddlewareError>,
) -> Result<ExtensionSettlement, RunHandleError> {
    match result {
        Ok(outcome) => {
            let bytes = serde_json_canonicalizer::to_vec(outcome).map_err(|_| {
                stage_error(crate::ports::middleware::MIDDLEWARE_OUTCOME_NOT_ALLOWED)
            })?;
            let output = finstack_ai_kernel::RawJson::parse(bytes).map_err(|_| {
                stage_error(crate::ports::middleware::MIDDLEWARE_OUTCOME_NOT_ALLOWED)
            })?;
            EffectCompleted::try_new(
                requested.effect_id(),
                requested.output_contract().clone(),
                output,
                None,
                Vec::new(),
                ProviderIds::empty(),
                None::<&str>,
                None,
            )
            .map(ExtensionSettlement::Completed)
            .map_err(|_| stage_error(crate::ports::middleware::MIDDLEWARE_OUTCOME_NOT_ALLOWED))
        }
        Err(error) => EffectFailed::try_new(
            requested.effect_id(),
            requested.output_contract().clone(),
            error.descriptor(),
            None,
            None::<&str>,
        )
        .map(ExtensionSettlement::Failed)
        .map_err(|_| stage_error(crate::ports::middleware::MIDDLEWARE_OUTCOME_NOT_ALLOWED)),
    }
}

fn extension_request_env<C: Clock, R: RandomSource>(
    sources: &SettlementSources<C, R>,
    requested: &EffectRequested,
) -> Result<TransitionEnv, RunHandleError> {
    let body = RecordBody::EffectRequested(requested.clone());
    extension_env(sources, &body, vec![requested.effect_id()])
}

fn extension_settlement_env<C: Clock, R: RandomSource>(
    sources: &SettlementSources<C, R>,
    settlement: &ExtensionSettlement,
) -> Result<TransitionEnv, RunHandleError> {
    let body = match settlement {
        ExtensionSettlement::Completed(value) => RecordBody::EffectCompleted(value.clone()),
        ExtensionSettlement::Failed(value) => RecordBody::EffectFailed(value.clone()),
    };
    extension_env(sources, &body, Vec::new())
}

fn extension_env<C: Clock, R: RandomSource>(
    sources: &SettlementSources<C, R>,
    body: &RecordBody,
    effect_ids: Vec<finstack_ai_kernel::EffectId>,
) -> Result<TransitionEnv, RunHandleError> {
    let event_count = body
        .derived_event_count(RECORD_KIND_VERSION)
        .map_err(|_| stage_error(crate::ports::middleware::MIDDLEWARE_OUTCOME_NOT_ALLOWED))?;
    Ok(TransitionEnv {
        now: sources.now()?,
        ids: AllocatedIds::try_new(
            vec![sources.generate::<RecordTag>()?],
            (0..event_count)
                .map(|_| sources.generate::<EventTag>())
                .collect::<Result<Vec<_>, _>>()?,
            effect_ids,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            vec![sources.generate::<AppendBatchTag>()?],
            Vec::new(),
        )
        .map_err(|_| stage_error(crate::ports::middleware::MIDDLEWARE_OUTCOME_NOT_ALLOWED))?,
    })
}
