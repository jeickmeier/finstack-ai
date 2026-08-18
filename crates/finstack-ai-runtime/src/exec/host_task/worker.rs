use std::collections::{BTreeMap, VecDeque};
use std::future::Future;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::task::Poll;

use finstack_ai_kernel::{EffectId, RunPhase};

use crate::coordinator::{CommitCoordinator, ModelDispatchSeed, ToolDispatchSeed};
use crate::middleware_driver::StageDriver;
use crate::run_types::RunHandleError;
use crate::settlement::{
    ModelDriverResult, SettlementSources, ToolDriverResult, ToolResultDisposition,
    continue_after_interaction, drain_idle_cancellation, parked_tool_continue,
    prepare_tool_batch_if_ready, process_model_progress, process_model_result,
    process_tool_progress, process_tool_result,
};
use crate::stage_settlement::submit_command;
use crate::tool::AssembledToolTerminal;
use crate::{
    CancellationSignal, Clock, LockedModelContextProfile, Model, ModelError, ModelRequest,
    ModelStreamAssembler, ModelTerminal, RandomSource, ResolvedTool, ResolvedToolCatalog,
    ToolCallContext, ToolError, ToolStreamAssembler,
};

use super::fault::{fault_shared, finish_worker, model_cancellation_error, result_fault_code};
use super::shared::{CommandIntake, HostWork, RunCommand, Shared};

pub(super) async fn run_worker(
    mut coordinator: CommitCoordinator,
    intake: Arc<CommandIntake>,
    shared: Arc<Shared>,
) {
    loop {
        if shared.shutting_down.load(Ordering::Acquire) {
            break;
        }
        let Some(command) = intake.recv().await else {
            break;
        };
        if shared.shutting_down.load(Ordering::Acquire) {
            command.reply.send(Err(RunHandleError::ShuttingDown));
            continue;
        }
        let result = coordinator
            .submit(command.env, command.input)
            .await
            .map_err(RunHandleError::Coordinator);
        let fault_code = result_fault_code(&result);
        command.reply.send(result);
        if let Some(code) = fault_code {
            fault_shared(&shared, code);
            break;
        }
    }
    finish_worker(&shared);
}

#[expect(
    clippy::too_many_arguments,
    reason = "the sequential owner keeps model, tool, and command state contiguous"
)]
pub(super) async fn run_worker_with_effects<C, R>(
    mut coordinator: CommitCoordinator,
    intake: Arc<CommandIntake>,
    shared: Arc<Shared>,
    model: Arc<dyn Model>,
    model_assembler: ModelStreamAssembler,
    tool_assembler: Option<ToolStreamAssembler>,
    catalog: Option<Arc<ResolvedToolCatalog>>,
    pending: Arc<Mutex<VecDeque<HostWork>>>,
    active: Arc<Mutex<BTreeMap<EffectId, CancellationSignal>>>,
    sources: SettlementSources<C, R>,
    stage_driver: Option<StageDriver>,
    profile: LockedModelContextProfile,
) where
    C: Clock + crate::PortObject,
    R: RandomSource + crate::PortObject,
{
    let mut parked_tool: Option<ToolDispatchSeed> = None;
    loop {
        if shared.shutting_down.load(Ordering::Acquire) {
            break;
        }
        let Some(command) = intake.recv().await else {
            break;
        };
        if shared.shutting_down.load(Ordering::Acquire) {
            command.reply.send(Err(RunHandleError::ShuttingDown));
            continue;
        }
        let parked_continue = parked_tool_continue(&command.input);
        if submit_and_reply(
            &mut coordinator,
            &shared,
            stage_driver.as_ref(),
            &sources,
            &profile,
            &model,
            command,
        )
        .await
        {
            break;
        }
        if let Some(catalog) = catalog.as_deref()
            && let Err(error) = continue_after_interaction(
                &mut coordinator,
                &sources,
                catalog,
                &mut parked_tool,
                parked_continue,
            )
            .await
        {
            fault_shared(
                &shared,
                match error {
                    RunHandleError::ToolSettlement { code }
                    | RunHandleError::InteractionSettlement { code }
                    | RunHandleError::Faulted { code } => code,
                    _ => "host_tool_interaction_continue_failed",
                },
            );
            break;
        }
        if let Err(error) = drain_idle_cancellation(&mut coordinator, &sources, false).await {
            fault_shared(
                &shared,
                match error {
                    RunHandleError::CancellationSettlement { code }
                    | RunHandleError::Faulted { code } => code,
                    _ => "host_idle_cancellation_failed",
                },
            );
            break;
        }
        if let Err(error) = Box::pin(drain_effects_accepting_commands(
            &mut coordinator,
            &intake,
            &shared,
            &model,
            model_assembler,
            tool_assembler,
            catalog.as_deref(),
            &pending,
            &active,
            &sources,
            stage_driver.as_ref(),
            &profile,
            &mut parked_tool,
        ))
        .await
        {
            fault_shared(
                &shared,
                match error {
                    RunHandleError::ModelSettlement { code }
                    | RunHandleError::ToolSettlement { code }
                    | RunHandleError::InteractionSettlement { code }
                    | RunHandleError::Faulted { code }
                    | RunHandleError::EventDelivery { code } => code,
                    _ => "host_effect_drain_failed",
                },
            );
            break;
        }
    }
    finish_worker(&shared);
}

async fn submit_and_reply<C, R>(
    coordinator: &mut CommitCoordinator,
    shared: &Arc<Shared>,
    stage_driver: Option<&StageDriver>,
    sources: &SettlementSources<C, R>,
    profile: &LockedModelContextProfile,
    model: &Arc<dyn crate::Model>,
    command: RunCommand,
) -> bool
where
    C: Clock + crate::PortObject,
    R: RandomSource + crate::PortObject,
{
    let RunCommand { env, input, reply } = command;
    let result = submit_command(
        coordinator,
        stage_driver,
        sources,
        profile,
        env,
        input,
        Some(model.as_ref()),
    )
    .await;
    let fault_code = result_fault_code(&result);
    reply.send(result);
    if let Some(code) = fault_code {
        fault_shared(shared, code);
        return true;
    }
    false
}

enum DrivePoll<T> {
    Command(Option<Box<RunCommand>>),
    Output(T),
}

#[expect(
    clippy::too_many_arguments,
    reason = "inline drain keeps settlement and dispatch on one sequential stack"
)]
async fn drain_effects_accepting_commands<C, R>(
    coordinator: &mut CommitCoordinator,
    intake: &CommandIntake,
    shared: &Arc<Shared>,
    model: &Arc<dyn Model>,
    model_assembler: ModelStreamAssembler,
    tool_assembler: Option<ToolStreamAssembler>,
    catalog: Option<&ResolvedToolCatalog>,
    pending: &Mutex<VecDeque<HostWork>>,
    active: &Mutex<BTreeMap<EffectId, CancellationSignal>>,
    sources: &SettlementSources<C, R>,
    stage_driver: Option<&StageDriver>,
    profile: &LockedModelContextProfile,
    parked_tool: &mut Option<ToolDispatchSeed>,
) -> Result<(), RunHandleError>
where
    C: Clock + crate::PortObject,
    R: RandomSource + crate::PortObject,
{
    loop {
        let work = pending
            .lock()
            .map_err(|_| RunHandleError::IntakeClosed)?
            .pop_front();
        let Some(work) = work else {
            if let Some(catalog) = catalog {
                prepare_tool_batch_if_ready(coordinator, catalog, sources, stage_driver).await?;
                let more = pending
                    .lock()
                    .map_err(|_| RunHandleError::IntakeClosed)?
                    .front()
                    .is_some();
                if more {
                    continue;
                }
            }
            break;
        };
        match work {
            HostWork::Model { seed, request } => {
                Box::pin(settle_driven_model(
                    coordinator,
                    intake,
                    shared,
                    model,
                    model_assembler,
                    catalog,
                    active,
                    sources,
                    stage_driver,
                    profile,
                    seed,
                    request,
                ))
                .await?;
            }
            HostWork::Tool {
                seed,
                context,
                resolved,
            } => {
                if let Some(parked) = settle_driven_tool(
                    coordinator,
                    intake,
                    shared,
                    model,
                    tool_assembler,
                    active,
                    sources,
                    stage_driver,
                    profile,
                    seed,
                    context,
                    resolved,
                )
                .await?
                {
                    *parked_tool = Some(parked);
                }
            }
        }
    }
    Ok(())
}

#[expect(
    clippy::too_many_arguments,
    reason = "model settlement keeps coordinator, intake, and active effects together"
)]
async fn settle_driven_model<C, R>(
    coordinator: &mut CommitCoordinator,
    intake: &CommandIntake,
    shared: &Arc<Shared>,
    model: &Arc<dyn Model>,
    model_assembler: ModelStreamAssembler,
    catalog: Option<&ResolvedToolCatalog>,
    active: &Mutex<BTreeMap<EffectId, CancellationSignal>>,
    sources: &SettlementSources<C, R>,
    stage_driver: Option<&StageDriver>,
    profile: &LockedModelContextProfile,
    seed: ModelDispatchSeed,
    request: ModelRequest,
) -> Result<(), RunHandleError>
where
    C: Clock + crate::PortObject,
    R: RandomSource + crate::PortObject,
{
    let effect_id = request.call.run.effect_id;
    let draft = request.draft.clone();
    let provider = model.descriptor().provider;
    let (progress, result) = match drive_accepting_commands(
        coordinator,
        intake,
        shared,
        stage_driver,
        profile,
        sources,
        model,
        drive_model(model, model_assembler, request),
    )
    .await
    {
        Ok(output) => output,
        Err(RunHandleError::CancellationSettlement { .. }) => (
            Vec::new(),
            Err(model_cancellation_error(
                "model request was cancelled during execution",
            )),
        ),
        Err(error) => return Err(error),
    };
    if let Ok(mut values) = active.lock() {
        values.remove(&effect_id);
    }
    for item in progress {
        process_model_progress(coordinator, effect_id, &provider, item, sources).await?;
    }
    process_model_result(
        coordinator,
        ModelDriverResult {
            seed,
            draft,
            provider,
            result,
        },
        sources,
    )
    .await?;
    if let Some(catalog) = catalog {
        prepare_tool_batch_if_ready(coordinator, catalog, sources, stage_driver).await?;
    }
    Ok(())
}

#[expect(
    clippy::too_many_arguments,
    reason = "tool settlement keeps coordinator, intake, and active effects together"
)]
async fn settle_driven_tool<C, R>(
    coordinator: &mut CommitCoordinator,
    intake: &CommandIntake,
    shared: &Arc<Shared>,
    model: &Arc<dyn Model>,
    tool_assembler: Option<ToolStreamAssembler>,
    active: &Mutex<BTreeMap<EffectId, CancellationSignal>>,
    sources: &SettlementSources<C, R>,
    stage_driver: Option<&StageDriver>,
    profile: &LockedModelContextProfile,
    seed: ToolDispatchSeed,
    context: ToolCallContext,
    resolved: Arc<ResolvedTool>,
) -> Result<Option<ToolDispatchSeed>, RunHandleError>
where
    C: Clock + crate::PortObject,
    R: RandomSource + crate::PortObject,
{
    let effect_id = context.run.effect_id;
    let assembler = tool_assembler.ok_or(RunHandleError::ToolSettlement {
        code: "tool_runtime_unavailable",
    })?;
    let call = seed.call.clone();
    let (progress, result) = match drive_accepting_commands(
        coordinator,
        intake,
        shared,
        stage_driver,
        profile,
        sources,
        model,
        drive_tool(resolved, context, call, assembler),
    )
    .await
    {
        Ok(output) => output,
        Err(RunHandleError::CancellationSettlement { .. }) => (
            Vec::new(),
            Err(ToolError::stable(
                crate::TOOL_CANCELLED,
                "tool call was cancelled during execution",
            )),
        ),
        Err(error) => return Err(error),
    };
    if let Ok(mut values) = active.lock() {
        values.remove(&effect_id);
    }
    for item in progress {
        process_tool_progress(coordinator, effect_id, item, sources).await?;
    }
    let parked = seed.clone();
    match process_tool_result(coordinator, ToolDriverResult { seed, result }, sources).await? {
        ToolResultDisposition::ParkedForInteraction => Ok(Some(parked)),
        ToolResultDisposition::Settled => Ok(None),
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "the wasm owner interleaves command intake with one in-flight model or tool drive"
)]
async fn drive_accepting_commands<C, R, T>(
    coordinator: &mut CommitCoordinator,
    intake: &CommandIntake,
    shared: &Arc<Shared>,
    stage_driver: Option<&StageDriver>,
    profile: &LockedModelContextProfile,
    sources: &SettlementSources<C, R>,
    model: &Arc<dyn Model>,
    drive: impl Future<Output = T>,
) -> Result<T, RunHandleError>
where
    C: Clock + crate::PortObject,
    R: RandomSource + crate::PortObject,
{
    let mut drive = std::pin::pin!(drive);
    loop {
        let mut recv = std::pin::pin!(intake.recv());
        let outcome = std::future::poll_fn(|cx| {
            if let Poll::Ready(command) = recv.as_mut().poll(cx) {
                return Poll::Ready(DrivePoll::Command(command.map(Box::new)));
            }
            if let Poll::Ready(output) = drive.as_mut().poll(cx) {
                return Poll::Ready(DrivePoll::Output(output));
            }
            Poll::Pending
        })
        .await;
        match outcome {
            DrivePoll::Command(None) => return Err(RunHandleError::IntakeClosed),
            DrivePoll::Command(Some(command)) => {
                if submit_and_reply(
                    coordinator,
                    shared,
                    stage_driver,
                    sources,
                    profile,
                    model,
                    *command,
                )
                .await
                {
                    return Err(RunHandleError::Faulted {
                        code: "host_run_faulted_during_effect",
                    });
                }
                if coordinator.state().phase == Some(RunPhase::Cancelling)
                    || coordinator.state().cancellation.is_some()
                {
                    return Err(RunHandleError::CancellationSettlement {
                        code: "host_effect_cancelled",
                    });
                }
            }
            DrivePoll::Output(output) => return Ok(output),
        }
    }
}

async fn drive_model(
    model: &Arc<dyn Model>,
    assembler: ModelStreamAssembler,
    request: ModelRequest,
) -> (Vec<crate::ModelProgress>, Result<ModelTerminal, ModelError>) {
    if request.call.run.cancellation.is_cancelled() {
        return (
            Vec::new(),
            Err(model_cancellation_error(
                "model request was cancelled before execution",
            )),
        );
    }
    let stream = match model.request(request).await {
        Ok(stream) => stream,
        Err(error) => return (Vec::new(), Err(error)),
    };
    let mut progress = Vec::new();
    let result = assembler
        .assemble_incremental(stream, |item| {
            progress.push(item);
            core::future::ready(Ok(()))
        })
        .await;
    (progress, result)
}

async fn drive_tool(
    resolved: Arc<ResolvedTool>,
    context: ToolCallContext,
    call: finstack_ai_kernel::ValidatedToolCall,
    assembler: ToolStreamAssembler,
) -> (
    Vec<finstack_ai_kernel::ToolProgress>,
    Result<AssembledToolTerminal, ToolError>,
) {
    if context.run.cancellation.is_cancelled() {
        return (
            Vec::new(),
            Err(ToolError::stable(
                crate::TOOL_CANCELLED,
                "tool call was cancelled before execution",
            )),
        );
    }
    let stream = match resolved.toolset.call(context, call).await {
        Ok(stream) => stream,
        Err(error) => return (Vec::new(), Err(error)),
    };
    let mut progress = Vec::new();
    let result = assembler
        .assemble_incremental(
            stream,
            resolved.output_validator.as_deref(),
            resolved.spec.max_result_bytes,
            resolved.spec.deferral,
            |item| {
                progress.push(item);
                core::future::ready(Ok(()))
            },
        )
        .await;
    (progress, result)
}
