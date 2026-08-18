use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use finstack_ai_kernel::{EffectId, Timestamp};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinSet;
use tokio::time::timeout;

use crate::event_hub::event_hub;
use crate::native::model::{ModelDispatcher, run_model_jobs};
use crate::native::timer::{TimerDispatcher, run_timer_jobs};
use crate::native::tool::{
    RuntimeDispatcher, ToolDispatcher, ToolExecutionContext, ToolTaskConfig, run_tool_jobs,
};
use crate::run_types::{
    ModelTaskConfig, RunHandleError, RunStatus, RunTaskConfig, ShutdownOutcome, ShutdownReport,
};
use crate::settlement::{
    NestedSamplingPorts, SettlementSources, apply_interaction_resume, drain_idle_cancellation,
    drive_due_polls, model_handle_error, next_due_poll_or_expiry, prepare_tool_batch_if_ready,
    resume_pending_model_effect, resume_pending_tool_effects, validate_model_binding,
};
use crate::{
    CancellationSignal, Clock, CommitCoordinator, LockedModelContextProfile,
    MODEL_RECONCILIATION_UNSUPPORTED, Model, ModelResumeAction, ModelWarmupContext,
    MonotonicDeadline, RandomSource, ResolvedToolCatalog, TOOL_RECONCILIATION_UNSUPPORTED,
    ToolResumeAction, ToolStreamAssembler,
};

use super::handle::RunHandle;
use super::shared::Shared;
use super::worker::{run_worker, run_worker_with_model, run_worker_with_model_and_tools};

async fn resume_model_effect<C, R>(
    coordinator: &mut CommitCoordinator,
    model: &dyn Model,
    dispatcher: &ModelDispatcher,
    sources: &SettlementSources<C, R>,
    cancellation: &CancellationSignal,
) -> Result<(), RunHandleError>
where
    C: Clock + Send + Sync + 'static,
    R: RandomSource + Send + Sync + 'static,
{
    let action = Box::pin(resume_pending_model_effect(
        coordinator,
        model,
        sources,
        cancellation,
    ))
    .await?;
    match action {
        ModelResumeAction::Retry => {
            let seed = coordinator
                .pending_model_seed()
                .ok_or(RunHandleError::ModelSettlement {
                    code: "model_resume_seed_missing",
                })?;
            dispatcher
                .resume_request(seed)
                .await
                .map_err(|error| RunHandleError::Model {
                    code: Arc::from(error.code),
                })
        }
        ModelResumeAction::SuspendUncertain => Err(RunHandleError::Model {
            code: Arc::from(MODEL_RECONCILIATION_UNSUPPORTED),
        }),
        ModelResumeAction::NoOutstanding
        | ModelResumeAction::UseRecorded
        | ModelResumeAction::Reconcile
        | ModelResumeAction::WaitExternal => Ok(()),
    }
}

async fn resume_tool_effects<C, R>(
    coordinator: &mut CommitCoordinator,
    catalog: &ResolvedToolCatalog,
    dispatcher: &ToolDispatcher,
    sources: &SettlementSources<C, R>,
    cancellation: &CancellationSignal,
) -> Result<(), RunHandleError>
where
    C: Clock + Send + Sync + 'static,
    R: RandomSource + Send + Sync + 'static,
{
    let action = resume_pending_tool_effects(coordinator, catalog, sources, cancellation).await?;
    match action {
        ToolResumeAction::Retry => {
            for seed in coordinator.pending_tool_seeds() {
                dispatcher
                    .resume_call(seed)
                    .await
                    .map_err(|error| RunHandleError::Tool {
                        code: Arc::from(error.code),
                    })?;
            }
            Ok(())
        }
        ToolResumeAction::SuspendUncertain => Err(RunHandleError::Tool {
            code: Arc::from(TOOL_RECONCILIATION_UNSUPPORTED),
        }),
        ToolResumeAction::NoOutstanding
        | ToolResumeAction::UseRecorded
        | ToolResumeAction::Reconcile
        | ToolResumeAction::WaitExternal => Ok(()),
    }
}

/// Schedule the next sibling poll wait from committed and live-only state.
///
/// # Errors
///
/// Returns a timer error when the bounded sibling-wait queue is closed.
pub(super) async fn arm_due_poll_wait(
    coordinator: &CommitCoordinator,
    schedules: &mpsc::Sender<Option<Timestamp>>,
    process_local_deadlines: &BTreeMap<EffectId, Timestamp>,
) -> Result<(), RunHandleError> {
    let deadline = next_due_poll_or_expiry(coordinator.state(), process_local_deadlines);
    schedules
        .send(deadline)
        .await
        .map_err(|_| RunHandleError::Timer {
            code: "poll_wait_queue_closed",
        })
}

/// Wake-up outcome emitted by the process-local deferred-poll waiter.
pub(super) enum DuePollWake {
    /// The current sibling poll deadline has elapsed.
    Due,
    /// The sibling deadline could not be converted into a monotonic wait.
    ConstructionFailed,
}

enum DuePollWait {
    Waiting(MonotonicDeadline),
    Due,
}

async fn run_due_poll_waits<C: Clock + Send + Sync + 'static>(
    clock: Arc<C>,
    cancellation: CancellationSignal,
    mut schedules: mpsc::Receiver<Option<Timestamp>>,
    fired: mpsc::Sender<DuePollWake>,
) {
    let mut wait: Option<MonotonicDeadline> = None;
    loop {
        if let Some(active) = wait.as_ref() {
            tokio::select! {
                () = cancellation.cancelled() => break,
                schedule = schedules.recv() => {
                    let Some(schedule) = schedule else { break; };
                    match schedule.map(|deadline| due_poll_wait(clock.as_ref(), deadline)) {
                        Some(Ok(DuePollWait::Waiting(next))) => wait = Some(next),
                        Some(Ok(DuePollWait::Due)) => {
                            if fired.send(DuePollWake::Due).await.is_err() {
                                break;
                            }
                            wait = None;
                        }
                        Some(Err(())) => {
                            if fired.send(DuePollWake::ConstructionFailed).await.is_err() {
                                break;
                            }
                            wait = None;
                        }
                        None => wait = None,
                    }
                }
                () = active.wait() => {
                    if fired.send(DuePollWake::Due).await.is_err() {
                        break;
                    }
                    wait = None;
                }
            }
        } else {
            tokio::select! {
                () = cancellation.cancelled() => break,
                schedule = schedules.recv() => {
                    let Some(schedule) = schedule else { break; };
                    match schedule.map(|deadline| due_poll_wait(clock.as_ref(), deadline)) {
                        Some(Ok(DuePollWait::Waiting(next))) => wait = Some(next),
                        Some(Ok(DuePollWait::Due)) => {
                            if fired.send(DuePollWake::Due).await.is_err() {
                                break;
                            }
                        }
                        Some(Err(()))
                            if fired.send(DuePollWake::ConstructionFailed).await.is_err() =>
                        {
                            break;
                        }
                        Some(Err(())) | None => {}
                    }
                }
            }
        }
    }
}

fn due_poll_wait(clock: &impl Clock, deadline: Timestamp) -> Result<DuePollWait, ()> {
    let scheduled_at = clock.now().map_err(|_| ())?;
    if deadline <= scheduled_at {
        return Ok(DuePollWait::Due);
    }
    MonotonicDeadline::from_persisted(clock, scheduled_at, deadline)
        .map(DuePollWait::Waiting)
        .map_err(|_| ())
}

/// Single owner of all tasks spawned for one run.
pub struct RunTaskOwner {
    handle: RunHandle,
    tasks: JoinSet<()>,
    active_effects: Vec<Arc<Mutex<BTreeMap<EffectId, CancellationSignal>>>>,
    run_cancellation: CancellationSignal,
    shutdown_deadline: Duration,
    joined: bool,
}

impl RunTaskOwner {
    /// Spawn one bounded run worker on the current Tokio runtime.
    ///
    /// # Errors
    ///
    /// Returns [`RunHandleError::InvalidConfiguration`] for zero bounds.
    pub fn spawn(
        mut coordinator: CommitCoordinator,
        config: RunTaskConfig,
    ) -> Result<Self, RunHandleError> {
        let config = config.validate()?;
        let (event_handle, event_task) =
            event_hub(config.event_hub).map_err(|_| RunHandleError::InvalidConfiguration)?;
        coordinator.install_event_publisher(Arc::new(event_handle.clone()));
        let (sender, receiver) = mpsc::channel(config.command_capacity);
        let (status_sender, status_receiver) = watch::channel(RunStatus::Running);
        let shared = Arc::new(Shared {
            sender: Mutex::new(Some(sender)),
            shutting_down: AtomicBool::new(false),
            status: status_sender,
            events: event_handle,
            shutdown_report: Mutex::new(None),
            timer_already_due: AtomicU64::new(0),
            timer_backward_clock_clamped: AtomicU64::new(0),
        });
        let handle = RunHandle {
            shared: Arc::clone(&shared),
            status: status_receiver,
        };
        let mut tasks = JoinSet::new();
        tasks.spawn(run_worker(coordinator, receiver, shared));
        tasks.spawn(event_task.run());
        Ok(Self {
            handle,
            tasks,
            active_effects: Vec::new(),
            run_cancellation: CancellationSignal::new(),
            shutdown_deadline: config.shutdown_deadline,
            joined: false,
        })
    }

    /// Warm one retained model and spawn the bounded commit/model workers.
    ///
    /// The ready handle is not returned until the default-no-op or provider
    /// warmup completes exactly once. Model jobs can only be enqueued by the
    /// coordinator after the request and effect records are committed/applied.
    ///
    /// # Errors
    ///
    /// Returns configuration or warmup errors before publishing a run handle.
    pub async fn spawn_with_model<C, R>(
        coordinator: CommitCoordinator,
        run_config: RunTaskConfig,
        model_config: ModelTaskConfig,
        model: Arc<dyn Model>,
        profile: LockedModelContextProfile,
        clock: C,
        random: R,
    ) -> Result<Self, RunHandleError>
    where
        C: Clock + Send + Sync + 'static,
        R: RandomSource + Send + Sync + 'static,
    {
        Box::pin(Self::spawn_with_model_inner(
            coordinator,
            run_config,
            model_config,
            model,
            profile,
            clock,
            random,
        ))
        .await
    }

    #[expect(
        clippy::too_many_lines,
        reason = "warmup, timer resume, model resume, and worker spawn stay contiguous"
    )]
    async fn spawn_with_model_inner<C, R>(
        mut coordinator: CommitCoordinator,
        run_config: RunTaskConfig,
        model_config: ModelTaskConfig,
        model: Arc<dyn Model>,
        profile: LockedModelContextProfile,
        clock: C,
        random: R,
    ) -> Result<Self, RunHandleError>
    where
        C: Clock + Send + Sync + 'static,
        R: RandomSource + Send + Sync + 'static,
    {
        let run_config = run_config.validate()?;
        let (event_handle, event_task) =
            event_hub(run_config.event_hub).map_err(|_| RunHandleError::InvalidConfiguration)?;
        coordinator.install_event_publisher(Arc::new(event_handle.clone()));
        let retry_policy = model_config.same_identity_retry;
        let assembler = model_config.validate()?;
        validate_model_binding(model.as_ref(), &profile)?;
        let sources = SettlementSources::try_new(clock, random)?;
        let runtime_clock = sources.clock();
        let run_cancellation = CancellationSignal::new();
        let model_cancellation = run_cancellation.child();
        let timer_cancellation = run_cancellation.child();
        model
            .warmup(ModelWarmupContext {
                cancellation: model_cancellation.child(),
                deadline: model_config.warmup_deadline,
                metadata: model_config.warmup_metadata.clone(),
            })
            .await
            .map_err(|error| model_handle_error(&error))?;

        let (sender, receiver) = mpsc::channel(run_config.command_capacity);
        let (job_sender, job_receiver) = mpsc::channel(model_config.job_capacity);
        let (result_sender, result_receiver) = mpsc::channel(model_config.result_capacity);
        let (timer_job_sender, timer_job_receiver) = mpsc::channel(run_config.command_capacity);
        let (timer_result_sender, timer_result_receiver) =
            mpsc::channel(run_config.command_capacity);
        // Cloned before the move: the worker needs the locked profile to
        // assemble `StageInput::BeforeModel`, and the dispatcher takes ownership.
        let stage_profile = profile.clone();
        let model_dispatcher = Arc::new(ModelDispatcher::new(
            Arc::clone(&model),
            profile,
            job_sender,
            model_cancellation.clone(),
        ));
        let model_active = model_dispatcher.active();
        let timer_dispatcher = Arc::new(TimerDispatcher::new(
            Arc::clone(&runtime_clock),
            timer_job_sender,
            timer_cancellation,
        ));
        let timer_active = timer_dispatcher.active();
        let mut tasks = JoinSet::new();
        tasks.spawn(event_task.run());
        let cancelling = coordinator.state().cancellation.is_some();
        if !cancelling && let Some(seed) = coordinator.pending_timer_seed() {
            timer_dispatcher
                .resume(seed)
                .await
                .map_err(|error| RunHandleError::Timer { code: error.code })?;
        }
        if !cancelling && next_due_poll_or_expiry(coordinator.state(), &BTreeMap::new()).is_some() {
            // This constructor has no tool catalog to reconcile committed deferred
            // effects. Refuse startup rather than silently discarding their deadline.
            return Err(RunHandleError::Tool {
                code: Arc::from(TOOL_RECONCILIATION_UNSUPPORTED),
            });
        }
        if cancelling {
            drain_idle_cancellation(&mut coordinator, &sources, true).await?;
        } else {
            Box::pin(resume_model_effect(
                &mut coordinator,
                model.as_ref(),
                &model_dispatcher,
                &sources,
                &model_cancellation,
            ))
            .await?;
        }
        coordinator.install_dispatcher(Arc::new(RuntimeDispatcher::model_only(
            model_dispatcher,
            timer_dispatcher,
        )));
        if !cancelling {
            apply_interaction_resume(&mut coordinator, &sources).await?;
        }

        let (status_sender, status_receiver) = watch::channel(RunStatus::Running);
        let shared = Arc::new(Shared {
            sender: Mutex::new(Some(sender)),
            shutting_down: AtomicBool::new(false),
            status: status_sender,
            events: event_handle,
            shutdown_report: Mutex::new(None),
            timer_already_due: AtomicU64::new(0),
            timer_backward_clock_clamped: AtomicU64::new(0),
        });
        let handle = RunHandle {
            shared: Arc::clone(&shared),
            status: status_receiver,
        };
        let stage_driver = crate::stage_settlement::stage_driver(&coordinator, &run_cancellation);
        tasks.spawn(run_worker_with_model(
            coordinator,
            receiver,
            result_receiver,
            timer_result_receiver,
            Arc::clone(&shared),
            sources,
            stage_driver,
            stage_profile,
            Arc::clone(&model),
        ));
        tasks.spawn(run_model_jobs(
            model,
            assembler,
            Arc::clone(&model_active),
            job_receiver,
            result_sender,
            Arc::clone(&runtime_clock),
            run_config.shutdown_deadline,
            retry_policy,
        ));
        tasks.spawn(run_timer_jobs(
            runtime_clock,
            Arc::clone(&timer_active),
            timer_job_receiver,
            timer_result_sender,
        ));
        Ok(Self {
            handle,
            tasks,
            active_effects: vec![model_active, timer_active],
            run_cancellation,
            shutdown_deadline: run_config.shutdown_deadline,
            joined: false,
        })
    }

    /// Warm one retained model and spawn the combined bounded model/tool runtime.
    ///
    /// Tool effects are routed only after their request records commit and the
    /// coordinator repeats its authorization/deadline check. The kernel remains
    /// the sole owner of execution groups and durable source ordering.
    ///
    /// # Errors
    ///
    /// Returns configuration, model warmup, or binding errors before publishing
    /// a run handle.
    #[expect(
        clippy::too_many_arguments,
        reason = "the public constructor receives the two explicit port configurations and injected identity sources"
    )]
    pub async fn spawn_with_model_and_tools<C, R>(
        coordinator: CommitCoordinator,
        run_config: RunTaskConfig,
        model_config: ModelTaskConfig,
        tool_config: ToolTaskConfig,
        model: Arc<dyn Model>,
        profile: LockedModelContextProfile,
        catalog: Arc<ResolvedToolCatalog>,
        clock: C,
        random: R,
    ) -> Result<Self, RunHandleError>
    where
        C: Clock + Send + Sync + 'static,
        R: RandomSource + Send + Sync + 'static,
    {
        Box::pin(Self::spawn_with_model_and_tools_inner(
            coordinator,
            run_config,
            model_config,
            tool_config,
            model,
            profile,
            catalog,
            clock,
            random,
        ))
        .await
    }

    #[expect(
        clippy::too_many_arguments,
        clippy::too_many_lines,
        reason = "the public constructor receives the two explicit port configurations and injected identity sources"
    )]
    async fn spawn_with_model_and_tools_inner<C, R>(
        mut coordinator: CommitCoordinator,
        run_config: RunTaskConfig,
        model_config: ModelTaskConfig,
        tool_config: ToolTaskConfig,
        model: Arc<dyn Model>,
        profile: LockedModelContextProfile,
        catalog: Arc<ResolvedToolCatalog>,
        clock: C,
        random: R,
    ) -> Result<Self, RunHandleError>
    where
        C: Clock + Send + Sync + 'static,
        R: RandomSource + Send + Sync + 'static,
    {
        let run_config = run_config.validate()?;
        let (event_handle, event_task) =
            event_hub(run_config.event_hub).map_err(|_| RunHandleError::InvalidConfiguration)?;
        coordinator.install_event_publisher(Arc::new(event_handle.clone()));
        let retry_policy = model_config.same_identity_retry;
        let model_assembler = model_config.validate()?;
        let tool_config = tool_config
            .validate()
            .map_err(|_| RunHandleError::InvalidConfiguration)?;
        validate_model_binding(model.as_ref(), &profile)?;
        let mut sources = SettlementSources::try_new(clock, random)?;
        let runtime_clock = sources.clock();
        let run_cancellation = CancellationSignal::new();
        let model_cancellation = run_cancellation.child();
        let tool_batch_cancellation = run_cancellation.child();
        let timer_cancellation = run_cancellation.child();
        sources.attach_nested_sampling(NestedSamplingPorts {
            model: Arc::clone(&model),
            profile: profile.clone(),
            catalog: Arc::clone(&catalog),
            cancellation: run_cancellation.child(),
        });
        model
            .warmup(ModelWarmupContext {
                cancellation: model_cancellation.child(),
                deadline: model_config.warmup_deadline,
                metadata: model_config.warmup_metadata.clone(),
            })
            .await
            .map_err(|error| model_handle_error(&error))?;

        let (sender, receiver) = mpsc::channel(run_config.command_capacity);
        let (model_job_sender, model_job_receiver) = mpsc::channel(model_config.job_capacity);
        let (model_result_sender, model_result_receiver) =
            mpsc::channel(model_config.result_capacity);
        let (tool_job_sender, tool_job_receiver) = mpsc::channel(tool_config.job_capacity);
        let (tool_result_sender, tool_result_receiver) = mpsc::channel(tool_config.result_capacity);
        let (timer_job_sender, timer_job_receiver) = mpsc::channel(run_config.command_capacity);
        let (timer_result_sender, timer_result_receiver) =
            mpsc::channel(run_config.command_capacity);
        let (due_poll_schedule_sender, due_poll_schedule_receiver) =
            mpsc::channel(run_config.command_capacity);
        let (due_poll_fired_sender, due_poll_fired_receiver) =
            mpsc::channel(run_config.command_capacity);
        let mut process_local_poll_deadlines = BTreeMap::new();

        // Cloned before the move: the worker needs the locked profile to
        // assemble `StageInput::BeforeModel`, and the dispatcher takes ownership.
        let stage_profile = profile.clone();
        let model_dispatcher = Arc::new(ModelDispatcher::new(
            Arc::clone(&model),
            profile,
            model_job_sender,
            model_cancellation.clone(),
        ));
        let model_active = model_dispatcher.active();
        let tool_dispatcher = Arc::new(ToolDispatcher::new(
            Arc::clone(&catalog),
            tool_job_sender,
            tool_batch_cancellation.clone(),
        ));
        let tool_active = tool_dispatcher.active();
        let tool_semaphores = tool_dispatcher.semaphores();
        let timer_dispatcher = Arc::new(TimerDispatcher::new(
            Arc::clone(&runtime_clock),
            timer_job_sender,
            timer_cancellation,
        ));
        let timer_active = timer_dispatcher.active();
        let mut tasks = JoinSet::new();
        tasks.spawn(event_task.run());
        let cancelling = coordinator.state().cancellation.is_some();
        if !cancelling && let Some(seed) = coordinator.pending_timer_seed() {
            timer_dispatcher
                .resume(seed)
                .await
                .map_err(|error| RunHandleError::Timer { code: error.code })?;
        }
        if cancelling {
            drain_idle_cancellation(&mut coordinator, &sources, true).await?;
        } else {
            Box::pin(resume_model_effect(
                &mut coordinator,
                model.as_ref(),
                &model_dispatcher,
                &sources,
                &model_cancellation,
            ))
            .await?;
        }
        coordinator.install_dispatcher(Arc::new(RuntimeDispatcher::with_tools(
            Arc::clone(&model_dispatcher),
            Arc::clone(&tool_dispatcher),
            timer_dispatcher,
        )));
        // Built before the resume-path tool batch, not after it: a resumed run
        // must route `BeforeToolBatch` through the same chain a steady-state
        // one does, or middleware would apply on some resume paths only.
        let stage_driver = crate::stage_settlement::stage_driver(&coordinator, &run_cancellation);
        if !cancelling {
            apply_interaction_resume(&mut coordinator, &sources).await?;
            let opened_tool_batch = prepare_tool_batch_if_ready(
                &mut coordinator,
                catalog.as_ref(),
                &sources,
                stage_driver.as_ref(),
            )
            .await?;
            if !opened_tool_batch {
                resume_tool_effects(
                    &mut coordinator,
                    catalog.as_ref(),
                    &tool_dispatcher,
                    &sources,
                    &tool_batch_cancellation,
                )
                .await?;
            }
            drive_due_polls(
                &mut coordinator,
                catalog.as_ref(),
                &sources,
                &tool_batch_cancellation,
                &mut process_local_poll_deadlines,
            )
            .await?;
            arm_due_poll_wait(
                &coordinator,
                &due_poll_schedule_sender,
                &process_local_poll_deadlines,
            )
            .await?;
        }

        let (status_sender, status_receiver) = watch::channel(RunStatus::Running);
        let shared = Arc::new(Shared {
            sender: Mutex::new(Some(sender)),
            shutting_down: AtomicBool::new(false),
            status: status_sender,
            events: event_handle,
            shutdown_report: Mutex::new(None),
            timer_already_due: AtomicU64::new(0),
            timer_backward_clock_clamped: AtomicU64::new(0),
        });
        let handle = RunHandle {
            shared: Arc::clone(&shared),
            status: status_receiver,
        };
        let due_poll_clock = sources.clock();
        tasks.spawn(run_worker_with_model_and_tools(
            coordinator,
            receiver,
            model_result_receiver,
            tool_result_receiver,
            timer_result_receiver,
            due_poll_fired_receiver,
            due_poll_schedule_sender,
            tool_batch_cancellation,
            process_local_poll_deadlines,
            Arc::clone(&shared),
            sources,
            catalog,
            stage_driver,
            stage_profile,
            Arc::clone(&model),
        ));
        tasks.spawn(run_model_jobs(
            model,
            model_assembler,
            Arc::clone(&model_active),
            model_job_receiver,
            model_result_sender,
            Arc::clone(&runtime_clock),
            run_config.shutdown_deadline,
            retry_policy,
        ));
        tasks.spawn(run_tool_jobs(
            tool_config,
            tool_semaphores,
            tool_job_receiver,
            ToolExecutionContext::new(
                ToolStreamAssembler::new(tool_config.stream_limits),
                Arc::clone(&tool_active),
                tool_result_sender,
                Arc::clone(&runtime_clock),
                run_config.shutdown_deadline,
            ),
        ));
        tasks.spawn(run_timer_jobs(
            runtime_clock,
            Arc::clone(&timer_active),
            timer_job_receiver,
            timer_result_sender,
        ));
        tasks.spawn(run_due_poll_waits(
            due_poll_clock,
            run_cancellation.child(),
            due_poll_schedule_receiver,
            due_poll_fired_sender,
        ));
        Ok(Self {
            handle,
            tasks,
            active_effects: vec![model_active, tool_active, timer_active],
            run_cancellation,
            shutdown_deadline: run_config.shutdown_deadline,
            joined: false,
        })
    }

    /// Clone the run handle without transferring task ownership.
    #[must_use]
    pub fn handle(&self) -> RunHandle {
        self.handle.clone()
    }

    /// Close intake, join normally, and abort on deadline expiry.
    pub async fn shutdown(&mut self) -> ShutdownReport {
        if self.joined {
            return self.handle.shutdown_report().unwrap_or(ShutdownReport {
                outcome: ShutdownOutcome::Graceful,
                signalled_effects: 0,
                aborted_tasks: 0,
            });
        }
        self.handle.shutdown();
        let signalled_effects = self.active_effect_count();
        self.run_cancellation.cancel();
        let joined = timeout(self.shutdown_deadline, async {
            while self.tasks.join_next().await.is_some() {}
        })
        .await
        .is_ok();
        let mut aborted_tasks = 0;
        if !joined {
            aborted_tasks = self.tasks.len();
            self.tasks.abort_all();
            while self.tasks.join_next().await.is_some() {}
            if !matches!(self.handle.status(), RunStatus::Faulted { .. }) {
                self.handle.shared.status.send_replace(RunStatus::Stopped);
            }
        }
        let report = ShutdownReport {
            outcome: if joined {
                ShutdownOutcome::Graceful
            } else {
                ShutdownOutcome::Forced
            },
            signalled_effects,
            aborted_tasks,
        };
        if let Ok(mut value) = self.handle.shared.shutdown_report.lock() {
            *value = Some(report);
        }
        self.joined = true;
        report
    }

    fn active_effect_count(&self) -> usize {
        self.active_effects
            .iter()
            .filter_map(|active| active.lock().ok().map(|values| values.len()))
            .sum()
    }
}

impl Drop for RunTaskOwner {
    fn drop(&mut self) {
        if !self.joined {
            self.handle.shutdown();
            let signalled_effects = self.active_effect_count();
            self.run_cancellation.cancel();
            let aborted_tasks = self.tasks.len();
            self.tasks.abort_all();
            if !matches!(self.handle.status(), RunStatus::Faulted { .. }) {
                self.handle.shared.status.send_replace(RunStatus::Stopped);
            }
            if let Ok(mut value) = self.handle.shared.shutdown_report.lock() {
                *value = Some(ShutdownReport {
                    outcome: ShutdownOutcome::OwnerDropped,
                    signalled_effects,
                    aborted_tasks,
                });
            }
        }
    }
}
