use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use finstack_ai_kernel::{
    AllocatedIds, AppendBatchTag, EffectId, KernelInput, RecordTag, Timestamp, TransitionEnv,
};
use tokio::sync::mpsc;

use crate::coordinator::ToolDispatchSeed;
use crate::middleware_driver::StageDriver;
use crate::model::Model;
use crate::native::model::ModelDriverMessage;
use crate::native::timer::{TimerDriverMessage, TimerDriverResult};
use crate::native::tool::ToolDriverMessage;
use crate::run_types::RunHandleError;
use crate::settlement::{
    SettlementSources, ToolResultDisposition, continue_after_interaction, drain_idle_cancellation,
    drive_due_polls, parked_tool_continue, prepare_tool_batch_if_ready, process_model_progress,
    process_model_result, process_tool_progress, process_tool_result, reconcile_cancelled_effect,
};
use crate::stage_settlement::submit_command;
use crate::{
    Clock, CommitCoordinator, CommitCoordinatorError, DeadlineDiagnostic,
    LockedModelContextProfile, RandomSource, ResolvedToolCatalog, RunStatus,
};

use super::fault::{fault_worker, model_runtime_fault, result_fault_code, runtime_fault};
use super::owner::{DuePollWake, arm_due_poll_wait};
use super::shared::{RunCommand, Shared};

pub(super) async fn run_worker(
    mut coordinator: CommitCoordinator,
    mut receiver: mpsc::Receiver<RunCommand>,
    shared: Arc<Shared>,
) {
    while let Some(command) = receiver.recv().await {
        if shared.shutting_down.load(Ordering::Acquire) {
            let _ = command.reply.send(Err(RunHandleError::ShuttingDown));
            continue;
        }
        let result = coordinator
            .submit(command.env, command.input)
            .await
            .map_err(RunHandleError::Coordinator);
        let fault_code = match &result {
            Ok(outcome) => outcome.fault.map(|fault| fault.code),
            Err(RunHandleError::Coordinator(
                CommitCoordinatorError::BoundaryFault { code }
                | CommitCoordinatorError::Faulted { code }
                | CommitCoordinatorError::EventDelivery { code },
            )) => Some(*code),
            _ => None,
        };
        let _ = command.reply.send(result);
        if let Some(code) = fault_code {
            shared.shutting_down.store(true, Ordering::Release);
            if let Ok(mut sender) = shared.sender.lock() {
                sender.take();
            }
            receiver.close();
            shared.status.send_replace(RunStatus::Faulted { code });
        }
    }
    if !matches!(*shared.status.borrow(), RunStatus::Faulted { .. }) {
        shared.status.send_replace(RunStatus::Stopped);
    }
    shared.events.close().await;
}

#[expect(
    clippy::too_many_arguments,
    reason = "the single owner select keeps command, model, and timer ordering visibly contiguous"
)]
pub(super) async fn run_worker_with_model<C, R>(
    mut coordinator: CommitCoordinator,
    mut receiver: mpsc::Receiver<RunCommand>,
    mut results: mpsc::Receiver<ModelDriverMessage>,
    mut timers: mpsc::Receiver<TimerDriverMessage>,
    shared: Arc<Shared>,
    sources: SettlementSources<C, R>,
    stage_driver: Option<StageDriver>,
    profile: LockedModelContextProfile,
    model: Arc<dyn Model>,
) where
    C: Clock + Send + Sync + 'static,
    R: RandomSource + Send + Sync + 'static,
{
    let mut result_path_open = true;
    let mut timer_path_open = true;
    loop {
        tokio::select! {
            biased;
            result = results.recv(), if result_path_open => {
                match result {
                    Some(ModelDriverMessage::Progress { effect_id, provider, progress }) => {
                        if shared.shutting_down.load(Ordering::Acquire) {
                            continue;
                        }
                        if let Err(error) = process_model_progress(
                            &mut coordinator,
                            effect_id,
                            &provider,
                            progress,
                            &sources,
                        ).await {
                            fault_worker(&shared, &mut receiver, model_runtime_fault(&error));
                            break;
                        }
                    }
                    Some(ModelDriverMessage::Terminal(result)) => {
                        if shared.shutting_down.load(Ordering::Acquire) {
                            continue;
                        }
                        if let Err(error) = process_model_result(&mut coordinator, *result, &sources).await {
                            fault_worker(&shared, &mut receiver, model_runtime_fault(&error));
                            break;
                        }
                    }
                    None => result_path_open = false,
                }
            }
            timer = timers.recv(), if timer_path_open => {
                match timer {
                    Some(TimerDriverMessage::Fired(result)) => {
                        if shared.shutting_down.load(Ordering::Acquire) {
                            continue;
                        }
                        record_timer_diagnostic(&shared, result.diagnostic);
                        if let Err(error) = process_timer_result(&mut coordinator, result, &sources).await {
                            fault_worker(&shared, &mut receiver, model_runtime_fault(&error));
                            break;
                        }
                    }
                    Some(TimerDriverMessage::Failed) => {
                        fault_worker(&shared, &mut receiver, "timer_clock_failed");
                        break;
                    }
                    None => timer_path_open = false,
                }
            }
            command = receiver.recv() => {
                let Some(command) = command else { break; };
                if shared.shutting_down.load(Ordering::Acquire) {
                    let _ = command.reply.send(Err(RunHandleError::ShuttingDown));
                    continue;
                }
                let RunCommand { env, input, reply } = command;
                let result = submit_command(
                    &mut coordinator,
                    stage_driver.as_ref(),
                    &sources,
                    &profile,
                    env,
                    input,
                    Some(model.as_ref()),
                ).await;
                let fault_code = result_fault_code(&result);
                let drain = if result.is_ok() {
                    drain_idle_cancellation(&mut coordinator, &sources, false)
                        .await
                        .err()
                } else {
                    None
                };
                let _ = reply.send(result);
                if let Some(code) = fault_code {
                    fault_worker(&shared, &mut receiver, code);
                    break;
                }
                if let Some(error) = drain {
                    fault_worker(&shared, &mut receiver, model_runtime_fault(&error));
                    break;
                }
            }
        }
    }
    if !matches!(*shared.status.borrow(), RunStatus::Faulted { .. }) {
        shared.status.send_replace(RunStatus::Stopped);
    }
    shared.events.close().await;
}

#[expect(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "the single owner select keeps command, model, tool, and timer ordering visibly contiguous"
)]
pub(super) async fn run_worker_with_model_and_tools<C, R>(
    mut coordinator: CommitCoordinator,
    mut receiver: mpsc::Receiver<RunCommand>,
    mut model_results: mpsc::Receiver<ModelDriverMessage>,
    mut tool_results: mpsc::Receiver<ToolDriverMessage>,
    mut timer_results: mpsc::Receiver<TimerDriverMessage>,
    mut due_poll_fired: mpsc::Receiver<DuePollWake>,
    due_poll_schedules: mpsc::Sender<Option<Timestamp>>,
    due_poll_cancellation: crate::CancellationSignal,
    mut process_local_poll_deadlines: BTreeMap<EffectId, Option<Timestamp>>,
    shared: Arc<Shared>,
    sources: SettlementSources<C, R>,
    catalog: Arc<ResolvedToolCatalog>,
    stage_driver: Option<StageDriver>,
    profile: LockedModelContextProfile,
    model: Arc<dyn Model>,
) where
    C: Clock + Send + Sync + 'static,
    R: RandomSource + Send + Sync + 'static,
{
    let mut model_path_open = true;
    let mut tool_path_open = true;
    let mut timer_path_open = true;
    let mut due_poll_path_open = true;
    let mut parked_tool: Option<ToolDispatchSeed> = None;
    loop {
        tokio::select! {
            biased;
            result = model_results.recv(), if model_path_open => {
                match result {
                    Some(ModelDriverMessage::Progress { effect_id, provider, progress }) => {
                        if shared.shutting_down.load(Ordering::Acquire) {
                            continue;
                        }
                        if let Err(error) = process_model_progress(
                            &mut coordinator,
                            effect_id,
                            &provider,
                            progress,
                            &sources,
                        ).await {
                            fault_worker(&shared, &mut receiver, runtime_fault(&error));
                            break;
                        }
                    }
                    Some(ModelDriverMessage::Terminal(result)) => {
                        if shared.shutting_down.load(Ordering::Acquire) {
                            continue;
                        }
                        let processed = process_model_result(&mut coordinator, *result, &sources).await;
                        let processed = match processed {
                            Ok(()) => prepare_tool_batch_if_ready(
                                &mut coordinator,
                                &catalog,
                                &sources,
                                stage_driver.as_ref(),
                            ).await,
                            Err(error) => Err(error),
                        };
                        if let Err(error) = processed {
                            fault_worker(&shared, &mut receiver, runtime_fault(&error));
                            break;
                        }
                    }
                    None => model_path_open = false,
                }
            }
            result = tool_results.recv(), if tool_path_open => {
                match result {
                    Some(ToolDriverMessage::Progress { effect_id, progress }) => {
                        if shared.shutting_down.load(Ordering::Acquire) {
                            continue;
                        }
                        if let Err(error) = process_tool_progress(
                            &mut coordinator,
                            effect_id,
                            progress,
                            &sources,
                        ).await {
                            fault_worker(&shared, &mut receiver, runtime_fault(&error));
                            break;
                        }
                    }
                    Some(ToolDriverMessage::Terminal(result)) => {
                        if shared.shutting_down.load(Ordering::Acquire) {
                            continue;
                        }
                        let seed = result.seed.clone();
                        match process_tool_result(&mut coordinator, *result, &sources).await {
                            Ok(ToolResultDisposition::ParkedForInteraction) => {
                                parked_tool = Some(seed);
                            }
                            Ok(ToolResultDisposition::Settled) => {}
                            Err(error) => {
                                fault_worker(&shared, &mut receiver, runtime_fault(&error));
                                break;
                            }
                        }
                        if let Err(error) =
                            arm_due_poll_wait(
                                &coordinator,
                                &due_poll_schedules,
                                &process_local_poll_deadlines,
                            )
                            .await
                        {
                            fault_worker(&shared, &mut receiver, runtime_fault(&error));
                            break;
                        }
                    }
                    None => tool_path_open = false,
                }
            }
            result = timer_results.recv(), if timer_path_open => {
                match result {
                    Some(TimerDriverMessage::Fired(result)) => {
                        if shared.shutting_down.load(Ordering::Acquire) {
                            continue;
                        }
                        record_timer_diagnostic(&shared, result.diagnostic);
                        if let Err(error) = process_timer_result(&mut coordinator, result, &sources).await {
                            fault_worker(&shared, &mut receiver, runtime_fault(&error));
                            break;
                        }
                    }
                    Some(TimerDriverMessage::Failed) => {
                        fault_worker(&shared, &mut receiver, "timer_clock_failed");
                        break;
                    }
                    None => timer_path_open = false,
                }
            }
            fired = due_poll_fired.recv(), if due_poll_path_open => {
                match fired {
                    Some(DuePollWake::Due) => {
                        if shared.shutting_down.load(Ordering::Acquire) {
                            continue;
                        }
                        let result = async {
                            drive_due_polls(
                                &mut coordinator,
                                catalog.as_ref(),
                                &sources,
                                &due_poll_cancellation,
                                &mut process_local_poll_deadlines,
                            )
                            .await?;
                            arm_due_poll_wait(
                                &coordinator,
                                &due_poll_schedules,
                                &process_local_poll_deadlines,
                            )
                            .await
                        }
                        .await;
                        if let Err(error) = result {
                            fault_worker(&shared, &mut receiver, runtime_fault(&error));
                            break;
                        }
                    }
                    Some(DuePollWake::ConstructionFailed) => {
                        fault_worker(&shared, &mut receiver, "timer_deadline_invalid");
                        break;
                    }
                    None => due_poll_path_open = false,
                }
            }
            command = receiver.recv() => {
                let Some(command) = command else { break; };
                if shared.shutting_down.load(Ordering::Acquire) {
                    let _ = command.reply.send(Err(RunHandleError::ShuttingDown));
                    continue;
                }
                let RunCommand { env, input, reply } = command;
                let parked_continue = parked_tool_continue(&input);
                let mut result = submit_command(
                    &mut coordinator,
                    stage_driver.as_ref(),
                    &sources,
                    &profile,
                    env,
                    input,
                    Some(model.as_ref()),
                ).await;
                if result.as_ref().is_ok_and(|outcome| outcome.fault.is_none())
                    && let Err(error) = continue_after_interaction(
                        &mut coordinator,
                        &sources,
                        &catalog,
                        &mut parked_tool,
                        parked_continue,
                    ).await
                {
                    result = Err(error);
                }
                if result.as_ref().is_ok_and(|outcome| outcome.fault.is_none())
                    && let Err(error) = prepare_tool_batch_if_ready(
                        &mut coordinator,
                        &catalog,
                        &sources,
                        stage_driver.as_ref(),
                    ).await
                {
                    result = Err(error);
                }
                if result.as_ref().is_ok_and(|outcome| outcome.fault.is_none())
                    && let Err(error) =
                        drain_idle_cancellation(&mut coordinator, &sources, false).await
                {
                    result = Err(error);
                }
                let fault_code = result_fault_code(&result);
                let _ = reply.send(result);
                if let Some(code) = fault_code {
                    fault_worker(&shared, &mut receiver, code);
                    break;
                }
            }
        }
    }
    if !matches!(*shared.status.borrow(), RunStatus::Faulted { .. }) {
        shared.status.send_replace(RunStatus::Stopped);
    }
    shared.events.close().await;
}

async fn process_timer_result<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    result: TimerDriverResult,
    sources: &SettlementSources<C, R>,
) -> Result<(), RunHandleError> {
    let TimerDriverResult {
        input,
        diagnostic: _,
    } = result;
    if coordinator.state().terminal.is_some() {
        return Ok(());
    }
    if coordinator.state().cancellation.is_some() {
        let effect_id = input.effect_id;
        let outstanding = coordinator
            .state()
            .cancellation
            .as_ref()
            .is_some_and(|cancellation| cancellation.outstanding_effects.contains(&effect_id));
        if outstanding {
            return reconcile_cancelled_effect(coordinator, effect_id, true, sources).await;
        }
        return Ok(());
    }
    let now = input.fired_at;
    let ids = AllocatedIds::try_new(
        vec![sources.generate::<RecordTag>()?],
        Vec::new(),
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
    .map_err(|_| RunHandleError::Timer {
        code: "timer_firing_ids_invalid",
    })?;
    let input = KernelInput::TimerFired(input);
    coordinator
        .classify(
            &TransitionEnv {
                now,
                ids: ids.clone(),
            },
            input.clone(),
        )
        .map_err(|_| RunHandleError::Timer {
            code: "timer_firing_allocation_mismatch",
        })?;
    let outcome = coordinator
        .submit(TransitionEnv { now, ids }, input)
        .await
        .map_err(RunHandleError::Coordinator)?;
    if let Some(fault) = outcome.fault {
        return Err(RunHandleError::Faulted { code: fault.code });
    }
    Ok(())
}

fn record_timer_diagnostic(shared: &Shared, diagnostic: DeadlineDiagnostic) {
    match diagnostic {
        DeadlineDiagnostic::None => {}
        DeadlineDiagnostic::AlreadyDue => {
            shared.timer_already_due.fetch_add(1, Ordering::AcqRel);
        }
        DeadlineDiagnostic::BackwardClockClamped => {
            shared
                .timer_backward_clock_clamped
                .fetch_add(1, Ordering::AcqRel);
        }
    }
}
