use std::collections::{BTreeMap, VecDeque};
use std::future::Future;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::task::Poll;

use finstack_ai_kernel::{EffectId, RunPhase};

use crate::coordinator::{CommitCoordinator, ModelDispatchSeed, ToolDispatchSeed};
use crate::middleware_driver::StageDriver;
use crate::run_types::{RunHandleError, result_fault_code};
use crate::run_types::{SameIdentityRetryPolicy, provider_retry_after, same_identity_retryable};
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
    ToolCallContext, ToolError, ToolStreamAssembler, ToolTaskConfig,
};

use super::fault::{fault_shared, finish_worker, host_drain_fault, model_cancellation_error};
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
    reason = "the host owner keeps model, tool, command, and bounded child-task state contiguous"
)]
pub(super) async fn run_worker_with_effects<C, R>(
    mut coordinator: CommitCoordinator,
    intake: Arc<CommandIntake>,
    shared: Arc<Shared>,
    model: Arc<dyn Model>,
    model_assembler: ModelStreamAssembler,
    tool_assembler: Option<ToolStreamAssembler>,
    tool_config: Option<ToolTaskConfig>,
    catalog: Option<Arc<ResolvedToolCatalog>>,
    pending: Arc<Mutex<VecDeque<HostWork>>>,
    active: Arc<Mutex<BTreeMap<EffectId, CancellationSignal>>>,
    sources: SettlementSources<C, R>,
    stage_driver: Option<StageDriver>,
    profile: LockedModelContextProfile,
    retry_policy: SameIdentityRetryPolicy,
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
                host_drain_fault(&error, "host_tool_interaction_continue_failed"),
            );
            break;
        }
        if let Err(error) = drain_idle_cancellation(&mut coordinator, &sources, false).await {
            fault_shared(
                &shared,
                host_drain_fault(&error, "host_idle_cancellation_failed"),
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
            tool_config,
            catalog.as_deref(),
            &pending,
            &active,
            &sources,
            stage_driver.as_ref(),
            &profile,
            &mut parked_tool,
            retry_policy,
        ))
        .await
        {
            fault_shared(
                &shared,
                host_drain_fault(&error, "host_effect_drain_failed"),
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
    clippy::too_many_lines,
    reason = "inline drain keeps settlement, dispatch, and scheduling boundaries on one owner stack"
)]
async fn drain_effects_accepting_commands<C, R>(
    coordinator: &mut CommitCoordinator,
    intake: &CommandIntake,
    shared: &Arc<Shared>,
    model: &Arc<dyn Model>,
    model_assembler: ModelStreamAssembler,
    tool_assembler: Option<ToolStreamAssembler>,
    tool_config: Option<ToolTaskConfig>,
    catalog: Option<&ResolvedToolCatalog>,
    pending: &Mutex<VecDeque<HostWork>>,
    active: &Arc<Mutex<BTreeMap<EffectId, CancellationSignal>>>,
    sources: &SettlementSources<C, R>,
    stage_driver: Option<&StageDriver>,
    profile: &LockedModelContextProfile,
    parked_tool: &mut Option<ToolDispatchSeed>,
    retry_policy: SameIdentityRetryPolicy,
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
                    retry_policy,
                ))
                .await?;
            }
            HostWork::Tool {
                seed,
                context,
                resolved,
            } => {
                if seed.call.execution == finstack_ai_kernel::ToolExecutionMode::Parallel {
                    let mut group = VecDeque::from([(seed, context, resolved)]);
                    {
                        let mut queued =
                            pending.lock().map_err(|_| RunHandleError::IntakeClosed)?;
                        loop {
                            match queued.front() {
                                Some(HostWork::Tool { seed, .. })
                                    if seed.call.execution
                                        == finstack_ai_kernel::ToolExecutionMode::Parallel => {}
                                _ => break,
                            }
                            match queued.pop_front() {
                                Some(HostWork::Tool {
                                    seed,
                                    context,
                                    resolved,
                                }) => group.push_back((seed, context, resolved)),
                                Some(work) => {
                                    queued.push_front(work);
                                    break;
                                }
                                None => break,
                            }
                        }
                    }
                    settle_parallel_tools(
                        coordinator,
                        intake,
                        shared,
                        tool_assembler,
                        tool_config,
                        active,
                        sources,
                        stage_driver,
                        profile,
                        model,
                        group,
                        parked_tool,
                    )
                    .await?;
                } else if let Some(parked) = Box::pin(settle_driven_tool(
                    coordinator,
                    intake,
                    shared,
                    model,
                    tool_assembler,
                    active.as_ref(),
                    sources,
                    stage_driver,
                    profile,
                    seed,
                    context,
                    resolved,
                ))
                .await?
                {
                    *parked_tool = Some(parked);
                }
            }
        }
    }
    Ok(())
}

type PendingHostTool = (ToolDispatchSeed, ToolCallContext, Arc<ResolvedTool>);

struct HostToolCompletion {
    tool_id: finstack_ai_kernel::ToolId,
    progress: Vec<finstack_ai_kernel::ToolProgress>,
    result: ToolDriverResult,
}

struct HostToolCompletions {
    values: Mutex<VecDeque<HostToolCompletion>>,
    available: crate::host_driver::Signal,
}

impl HostToolCompletions {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            values: Mutex::new(VecDeque::new()),
            available: crate::host_driver::Signal::new(),
        })
    }

    fn push(&self, completion: HostToolCompletion) {
        if let Ok(mut values) = self.values.lock() {
            values.push_back(completion);
            self.available.notify_waiters();
        }
    }

    fn pop(&self) -> Result<Option<HostToolCompletion>, RunHandleError> {
        self.values
            .lock()
            .map(|mut values| values.pop_front())
            .map_err(|_| RunHandleError::IntakeClosed)
    }
}

enum ParallelToolPoll {
    Command(Option<Box<RunCommand>>),
    Completed,
}

#[expect(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "parallel host execution keeps its single-owner settlement dependencies explicit"
)]
async fn settle_parallel_tools<C, R>(
    coordinator: &mut CommitCoordinator,
    intake: &CommandIntake,
    shared: &Arc<Shared>,
    tool_assembler: Option<ToolStreamAssembler>,
    tool_config: Option<ToolTaskConfig>,
    active: &Arc<Mutex<BTreeMap<EffectId, CancellationSignal>>>,
    sources: &SettlementSources<C, R>,
    stage_driver: Option<&StageDriver>,
    profile: &LockedModelContextProfile,
    model: &Arc<dyn Model>,
    mut pending: VecDeque<PendingHostTool>,
    parked_tool: &mut Option<ToolDispatchSeed>,
) -> Result<(), RunHandleError>
where
    C: Clock + crate::PortObject,
    R: RandomSource + crate::PortObject,
{
    let assembler = tool_assembler.ok_or(RunHandleError::ToolSettlement {
        code: "tool_runtime_unavailable",
    })?;
    let config = tool_config.ok_or(RunHandleError::ToolSettlement {
        code: "tool_runtime_unavailable",
    })?;
    // A completion occupies one result slot until the owner polls it. Keeping
    // active children under both ceilings makes the host path bounded without
    // requiring a target-specific channel implementation.
    let concurrency = config
        .global_max_concurrency
        .min(config.result_capacity)
        .max(1);
    let completions = HostToolCompletions::new();
    let mut running_by_tool = BTreeMap::<finstack_ai_kernel::ToolId, usize>::new();
    let mut tasks = Vec::<crate::host_driver::HostTaskHandle>::new();
    let mut running = 0_usize;
    let mut intake_open = true;

    while !pending.is_empty() || running > 0 {
        while running < concurrency {
            let ready = pending.iter().position(|(_, _, resolved)| {
                running_by_tool.get(&resolved.spec.id).copied().unwrap_or(0)
                    < resolved.policy.max_concurrency
            });
            let Some(index) = ready else { break };
            let Some((seed, context, resolved)) = pending.remove(index) else {
                break;
            };
            let tool_id = resolved.spec.id.clone();
            *running_by_tool.entry(tool_id.clone()).or_default() += 1;
            running += 1;
            let completions = Arc::clone(&completions);
            let task_active = Arc::clone(active);
            let cancellation = context.run.cancellation.clone();
            let call = seed.call.clone();
            let effect_id = context.run.effect_id;
            let task = crate::host_driver::spawn(Box::pin(async move {
                let (progress, result) =
                    drive_tool_cancellable(resolved, context, call, assembler, cancellation).await;
                if let Ok(mut values) = task_active.lock() {
                    values.remove(&effect_id);
                }
                completions.push(HostToolCompletion {
                    tool_id,
                    progress,
                    result: ToolDriverResult { seed, result },
                });
            }));
            let Ok(task) = task else {
                if let Ok(mut values) = active.lock() {
                    values.remove(&effect_id);
                }
                abort_host_tasks(&tasks).await;
                return Err(RunHandleError::InvalidConfiguration);
            };
            tasks.push(task);
        }

        while let Some(completion) = completions.pop()? {
            running = running.saturating_sub(1);
            if let Some(count) = running_by_tool.get_mut(&completion.tool_id) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    running_by_tool.remove(&completion.tool_id);
                }
            }
            if shared.shutting_down.load(Ordering::Acquire) {
                continue;
            }
            let effect_id = completion.result.seed.requested.effect_id();
            for item in completion.progress {
                process_tool_progress(coordinator, effect_id, item, sources).await?;
            }
            let parked = completion.result.seed.clone();
            match process_tool_result(coordinator, completion.result, sources).await? {
                ToolResultDisposition::ParkedForInteraction => *parked_tool = Some(parked),
                ToolResultDisposition::Settled => {}
            }
        }
        if pending.is_empty() && running == 0 {
            break;
        }
        if shared.shutting_down.load(Ordering::Acquire) {
            for cancellation in active
                .lock()
                .map_err(|_| RunHandleError::IntakeClosed)?
                .values()
            {
                cancellation.cancel();
            }
        }

        let mut notified = std::pin::pin!(completions.available.notified());
        let mut recv = std::pin::pin!(intake.recv());
        let outcome = std::future::poll_fn(|cx| {
            if completions
                .values
                .lock()
                .is_ok_and(|values| !values.is_empty())
            {
                return Poll::Ready(ParallelToolPoll::Completed);
            }
            if intake_open && let Poll::Ready(command) = recv.as_mut().poll(cx) {
                return Poll::Ready(ParallelToolPoll::Command(command.map(Box::new)));
            }
            if notified.as_mut().poll(cx).is_ready() {
                return Poll::Ready(ParallelToolPoll::Completed);
            }
            Poll::Pending
        })
        .await;
        match outcome {
            ParallelToolPoll::Completed => {}
            ParallelToolPoll::Command(None) => intake_open = false,
            ParallelToolPoll::Command(Some(command)) => {
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
                    abort_host_tasks(&tasks).await;
                    return Err(RunHandleError::Faulted {
                        code: "host_run_faulted_during_effect",
                    });
                }
            }
        }
    }
    for task in &tasks {
        task.completed().await;
    }
    Ok(())
}

async fn abort_host_tasks(tasks: &[crate::host_driver::HostTaskHandle]) {
    for task in tasks {
        if !task.is_completed() {
            task.abort();
        }
    }
    crate::host_driver::yield_now().await;
    for task in tasks {
        task.completed().await;
    }
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
    retry_policy: SameIdentityRetryPolicy,
) -> Result<(), RunHandleError>
where
    C: Clock + crate::PortObject,
    R: RandomSource + crate::PortObject,
{
    let effect_id = request.call.run.effect_id;
    let draft = request.draft.clone();
    let provider = model.descriptor().provider;
    let (progress, result) = match Box::pin(drive_accepting_commands(
        coordinator,
        intake,
        shared,
        stage_driver,
        profile,
        sources,
        model,
        drive_model(model, model_assembler, request, retry_policy),
    ))
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
    policy: SameIdentityRetryPolicy,
) -> (Vec<crate::ModelProgress>, Result<ModelTerminal, ModelError>) {
    let mut progress = Vec::new();
    let effect_id = request.call.run.effect_id;
    let max_attempts = policy.max_retries.saturating_add(1);
    let mut attempt = 0_u32;
    loop {
        attempt = attempt.saturating_add(1);
        if request.call.run.cancellation.is_cancelled() {
            return (
                progress,
                Err(model_cancellation_error(
                    "model request was cancelled before execution",
                )),
            );
        }
        if let Ok(metadata) = crate::Metadata::parse(format!(r#"{{"attempt":{attempt}}}"#)) {
            progress.push(crate::ModelProgress::Heartbeat(metadata));
        }
        let result = match model.request(request.clone()).await {
            Ok(stream) => {
                assembler
                    .assemble_incremental(stream, |item| {
                        progress.push(item);
                        core::future::ready(Ok(()))
                    })
                    .await
            }
            Err(error) => Err(error),
        };
        match result {
            Ok(terminal) => return (progress, Ok(terminal)),
            Err(error) if attempt < max_attempts && same_identity_retryable(&error) => {
                let delay = provider_retry_after(&error).map_or_else(
                    || policy.backoff.delay(effect_id, attempt),
                    |retry_after| retry_after.max(policy.backoff.delay(effect_id, attempt)),
                );
                if !delay.is_zero() {
                    crate::host_driver::sleep(delay).await;
                }
            }
            Err(error) => return (progress, Err(error)),
        }
    }
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

async fn drive_tool_cancellable(
    resolved: Arc<ResolvedTool>,
    context: ToolCallContext,
    call: finstack_ai_kernel::ValidatedToolCall,
    assembler: ToolStreamAssembler,
    cancellation: CancellationSignal,
) -> (
    Vec<finstack_ai_kernel::ToolProgress>,
    Result<AssembledToolTerminal, ToolError>,
) {
    let mut drive = std::pin::pin!(drive_tool(resolved, context, call, assembler));
    let mut cancelled = std::pin::pin!(cancellation.cancelled());
    std::future::poll_fn(|cx| {
        if cancelled.as_mut().poll(cx).is_ready() {
            return Poll::Ready((
                Vec::new(),
                Err(ToolError::stable(
                    crate::TOOL_CANCELLED,
                    "tool call was cancelled during execution",
                )),
            ));
        }
        drive.as_mut().poll(cx)
    })
    .await
}
