//! Bounded native Tokio task ownership for one runtime coordinator.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use finstack_ai_kernel::{
    AllocatedIds, AppendBatchTag, EffectId, KernelInput, RecordTag, TransitionEnv,
};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinSet;
use tokio::time::timeout;

use crate::event_hub::{EventHubHandle, event_hub};
use crate::middleware_driver::StageDriver;
use crate::model_runtime::{ModelDispatcher, ModelDriverMessage, run_model_jobs};
use crate::run_types::{
    ModelTaskConfig, RunHandleError, RunStatus, RunTaskConfig, ShutdownOutcome, ShutdownReport,
    TimerDiagnostics,
};
use crate::settlement::{
    SettlementSources, apply_interaction_resume, drain_idle_cancellation, model_handle_error,
    prepare_tool_batch_if_ready, process_model_progress, process_model_result,
    process_tool_progress, process_tool_result, reconcile_cancelled_effect,
    resume_pending_model_effect, resume_pending_tool_effects, validate_model_binding,
};
use crate::stage_settlement::submit_command;
use crate::timer_runtime::{
    TimerDispatcher, TimerDriverMessage, TimerDriverResult, run_timer_jobs,
};
use crate::tool_runtime::{
    RuntimeDispatcher, ToolDispatcher, ToolDriverMessage, ToolExecutionContext, ToolTaskConfig,
    run_tool_jobs,
};
use crate::{
    CancellationSignal, Clock, CommitCoordinator, CommitCoordinatorError, CommitOutcome,
    DeadlineDiagnostic, EventSubscription, EventSubscriptionConfig, EventSubscriptionError,
    LockedModelContextProfile, MODEL_RECONCILIATION_UNSUPPORTED, Model, ModelResumeAction,
    ModelWarmupContext, RandomSource, ResolvedToolCatalog, TOOL_RECONCILIATION_UNSUPPORTED,
    ToolResumeAction, ToolStreamAssembler,
};

/// Cloneable bounded command/status/shutdown handle.
#[derive(Clone)]
pub struct RunHandle {
    shared: Arc<Shared>,
    status: watch::Receiver<RunStatus>,
}

impl RunHandle {
    /// Register an interactive subscription before publishing later run events.
    ///
    /// # Errors
    ///
    /// Rejects invalid configuration, exhausted subscriber capacity, or a closed hub.
    pub async fn subscribe_events(
        &self,
        config: EventSubscriptionConfig,
    ) -> Result<EventSubscription, EventSubscriptionError> {
        self.shared.events.subscribe_interactive(config).await
    }

    /// Register a read-only observer subscription isolated from interactive delivery.
    ///
    /// # Errors
    ///
    /// Rejects invalid configuration, exhausted subscriber capacity, or a closed hub.
    pub async fn subscribe_observer(
        &self,
        config: EventSubscriptionConfig,
    ) -> Result<EventSubscription, EventSubscriptionError> {
        self.shared.events.subscribe_observer(config).await
    }

    /// Submit one command, awaiting bounded-channel capacity when necessary.
    ///
    /// # Errors
    ///
    /// Rejects after shutdown/fault and forwards coordinator failures.
    pub async fn submit(
        &self,
        env: TransitionEnv,
        input: KernelInput,
    ) -> Result<CommitOutcome, RunHandleError> {
        match self.status() {
            RunStatus::Running => {}
            RunStatus::ShuttingDown => return Err(RunHandleError::ShuttingDown),
            RunStatus::Stopped => return Err(RunHandleError::Stopped),
            RunStatus::Faulted { code } => return Err(RunHandleError::Faulted { code }),
        }
        let sender = self
            .shared
            .sender
            .lock()
            .map_err(|_| RunHandleError::IntakeClosed)?
            .clone()
            .ok_or(RunHandleError::ShuttingDown)?;
        let (reply, receive) = oneshot::channel();
        sender
            .send(RunCommand { env, input, reply })
            .await
            .map_err(|_| self.closed_error())?;
        receive.await.map_err(|_| self.closed_error())?
    }

    /// Initiate idempotent shutdown and close shared intake.
    pub fn shutdown(&self) {
        if !self.shared.shutting_down.swap(true, Ordering::AcqRel) {
            if let Ok(mut sender) = self.shared.sender.lock() {
                sender.take();
            }
            if !matches!(
                self.status(),
                RunStatus::Faulted { .. } | RunStatus::Stopped
            ) {
                self.shared.status.send_replace(RunStatus::ShuttingDown);
            }
        }
    }

    /// Read the latest lifecycle state without blocking.
    #[must_use]
    pub fn status(&self) -> RunStatus {
        *self.status.borrow()
    }

    /// Clone a watch receiver for asynchronous status observation.
    #[must_use]
    pub fn observe_status(&self) -> watch::Receiver<RunStatus> {
        self.status.clone()
    }

    /// Read the shutdown report after the owner has settled.
    #[must_use]
    pub fn shutdown_report(&self) -> Option<ShutdownReport> {
        self.shared
            .shutdown_report
            .lock()
            .ok()
            .and_then(|report| *report)
    }

    /// Read cumulative safe timer diagnostics for this process-local run owner.
    #[must_use]
    pub fn timer_diagnostics(&self) -> TimerDiagnostics {
        TimerDiagnostics {
            already_due: self.shared.timer_already_due.load(Ordering::Acquire),
            backward_clock_clamped: self
                .shared
                .timer_backward_clock_clamped
                .load(Ordering::Acquire),
        }
    }

    fn closed_error(&self) -> RunHandleError {
        match self.status() {
            RunStatus::Running => RunHandleError::IntakeClosed,
            RunStatus::ShuttingDown => RunHandleError::ShuttingDown,
            RunStatus::Stopped => RunHandleError::Stopped,
            RunStatus::Faulted { code } => RunHandleError::Faulted { code },
        }
    }
}

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
    let action = resume_pending_model_effect(coordinator, model, sources, cancellation).await?;
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
        if cancelling {
            drain_idle_cancellation(&mut coordinator, &sources, true).await?;
        } else {
            resume_model_effect(
                &mut coordinator,
                model.as_ref(),
                &model_dispatcher,
                &sources,
                &model_cancellation,
            )
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
        ));
        tasks.spawn(run_model_jobs(
            model,
            assembler,
            Arc::clone(&model_active),
            job_receiver,
            result_sender,
            Arc::clone(&runtime_clock),
            run_config.shutdown_deadline,
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
        let model_assembler = model_config.validate()?;
        let tool_config = tool_config
            .validate()
            .map_err(|_| RunHandleError::InvalidConfiguration)?;
        validate_model_binding(model.as_ref(), &profile)?;
        let sources = SettlementSources::try_new(clock, random)?;
        let runtime_clock = sources.clock();
        let run_cancellation = CancellationSignal::new();
        let model_cancellation = run_cancellation.child();
        let tool_batch_cancellation = run_cancellation.child();
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
        let (model_job_sender, model_job_receiver) = mpsc::channel(model_config.job_capacity);
        let (model_result_sender, model_result_receiver) =
            mpsc::channel(model_config.result_capacity);
        let (tool_job_sender, tool_job_receiver) = mpsc::channel(tool_config.job_capacity);
        let (tool_result_sender, tool_result_receiver) = mpsc::channel(tool_config.result_capacity);
        let (timer_job_sender, timer_job_receiver) = mpsc::channel(run_config.command_capacity);
        let (timer_result_sender, timer_result_receiver) =
            mpsc::channel(run_config.command_capacity);

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
            resume_model_effect(
                &mut coordinator,
                model.as_ref(),
                &model_dispatcher,
                &sources,
                &model_cancellation,
            )
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
        tasks.spawn(run_worker_with_model_and_tools(
            coordinator,
            receiver,
            model_result_receiver,
            tool_result_receiver,
            timer_result_receiver,
            Arc::clone(&shared),
            sources,
            catalog,
            stage_driver,
        ));
        tasks.spawn(run_model_jobs(
            model,
            model_assembler,
            Arc::clone(&model_active),
            model_job_receiver,
            model_result_sender,
            Arc::clone(&runtime_clock),
            run_config.shutdown_deadline,
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

struct Shared {
    sender: Mutex<Option<mpsc::Sender<RunCommand>>>,
    shutting_down: AtomicBool,
    status: watch::Sender<RunStatus>,
    events: EventHubHandle,
    shutdown_report: Mutex<Option<ShutdownReport>>,
    timer_already_due: AtomicU64,
    timer_backward_clock_clamped: AtomicU64,
}

struct RunCommand {
    env: TransitionEnv,
    input: KernelInput,
    reply: oneshot::Sender<Result<CommitOutcome, RunHandleError>>,
}

async fn run_worker(
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

async fn run_worker_with_model<C, R>(
    mut coordinator: CommitCoordinator,
    mut receiver: mpsc::Receiver<RunCommand>,
    mut results: mpsc::Receiver<ModelDriverMessage>,
    mut timers: mpsc::Receiver<TimerDriverMessage>,
    shared: Arc<Shared>,
    sources: SettlementSources<C, R>,
    stage_driver: Option<StageDriver>,
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
                    env,
                    input,
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
async fn run_worker_with_model_and_tools<C, R>(
    mut coordinator: CommitCoordinator,
    mut receiver: mpsc::Receiver<RunCommand>,
    mut model_results: mpsc::Receiver<ModelDriverMessage>,
    mut tool_results: mpsc::Receiver<ToolDriverMessage>,
    mut timer_results: mpsc::Receiver<TimerDriverMessage>,
    shared: Arc<Shared>,
    sources: SettlementSources<C, R>,
    catalog: Arc<ResolvedToolCatalog>,
    stage_driver: Option<StageDriver>,
) where
    C: Clock + Send + Sync + 'static,
    R: RandomSource + Send + Sync + 'static,
{
    let mut model_path_open = true;
    let mut tool_path_open = true;
    let mut timer_path_open = true;
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
                        if let Err(error) = process_tool_result(&mut coordinator, *result, &sources).await {
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
            command = receiver.recv() => {
                let Some(command) = command else { break; };
                if shared.shutting_down.load(Ordering::Acquire) {
                    let _ = command.reply.send(Err(RunHandleError::ShuttingDown));
                    continue;
                }
                let RunCommand { env, input, reply } = command;
                let mut result = submit_command(
                    &mut coordinator,
                    stage_driver.as_ref(),
                    &sources,
                    env,
                    input,
                ).await;
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

fn result_fault_code(result: &Result<CommitOutcome, RunHandleError>) -> Option<&'static str> {
    match result {
        Ok(outcome) => outcome.fault.map(|fault| fault.code),
        Err(
            RunHandleError::Faulted { code }
            | RunHandleError::ModelSettlement { code }
            | RunHandleError::ToolSettlement { code }
            | RunHandleError::InteractionSettlement { code }
            | RunHandleError::EventDelivery { code }
            | RunHandleError::Coordinator(
                CommitCoordinatorError::BoundaryFault { code }
                | CommitCoordinatorError::Faulted { code }
                | CommitCoordinatorError::EventDelivery { code },
            ),
        ) => Some(*code),
        _ => None,
    }
}

fn fault_worker(shared: &Shared, receiver: &mut mpsc::Receiver<RunCommand>, code: &'static str) {
    shared.shutting_down.store(true, Ordering::Release);
    if let Ok(mut sender) = shared.sender.lock() {
        sender.take();
    }
    receiver.close();
    shared.status.send_replace(RunStatus::Faulted { code });
}

fn model_runtime_fault(error: &RunHandleError) -> &'static str {
    match error {
        RunHandleError::Faulted { code }
        | RunHandleError::ModelSettlement { code }
        | RunHandleError::Timer { code }
        | RunHandleError::CancellationSettlement { code }
        | RunHandleError::EventDelivery { code }
        | RunHandleError::Coordinator(
            CommitCoordinatorError::BoundaryFault { code }
            | CommitCoordinatorError::Decision { code }
            | CommitCoordinatorError::Faulted { code }
            | CommitCoordinatorError::EventDelivery { code },
        ) => code,
        _ => "model_runtime_failed",
    }
}

fn runtime_fault(error: &RunHandleError) -> &'static str {
    match error {
        RunHandleError::ToolSettlement { code }
        | RunHandleError::InteractionSettlement { code } => code,
        _ => model_runtime_fault(error),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use finstack_ai_kernel::{
        AcceptRun, AllocatedIds, BudgetPropagation, CancellationPropagation, Digest, Id, IdTag,
        PrincipalPropagation, PrincipalRef, RunAccepted, RunLimits, RunPropagationPolicy,
        RunRelation, RunSecurityContext, Timestamp,
    };
    use tokio::sync::Notify;

    use super::*;
    use crate::{
        EventHubConfig, JournalStore, LoadRequest, LoadedSession, PortFuture, SnapshotReceipt,
        SnapshotRequest, StoreError, StoreHealth,
    };

    struct BlockingStore {
        calls: AtomicUsize,
        started: AtomicUsize,
        release: Arc<Notify>,
        block_every_call: bool,
    }

    impl BlockingStore {
        fn new(block_every_call: bool) -> Self {
            Self {
                calls: AtomicUsize::new(0),
                started: AtomicUsize::new(0),
                release: Arc::new(Notify::new()),
                block_every_call,
            }
        }
    }

    impl JournalStore for BlockingStore {
        fn append(
            &self,
            _request: finstack_ai_kernel::AppendRequest,
        ) -> PortFuture<Result<finstack_ai_kernel::CommittedBatch, StoreError>> {
            let call = self.calls.fetch_add(1, Ordering::AcqRel);
            self.started.fetch_add(1, Ordering::Release);
            let release = Arc::clone(&self.release);
            let block = self.block_every_call || call == 0;
            Box::pin(async move {
                if block {
                    release.notified().await;
                }
                Err(StoreError::Unavailable {
                    reason_code: "test_store_unavailable",
                })
            })
        }

        fn load(&self, request: LoadRequest) -> PortFuture<Result<LoadedSession, StoreError>> {
            Box::pin(async move { Ok(LoadedSession::empty(request.session_id)) })
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

    fn id<T: IdTag>(ordinal: u64) -> Id<T> {
        let mut bytes = [0_u8; 16];
        bytes[6] = 0x70;
        bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        Id::from_bytes(bytes)
    }

    fn accepted() -> RunAccepted {
        let run_id = id(3);
        RunAccepted::try_new(
            run_id,
            RunRelation::root(run_id).expect("relation"),
            RunSecurityContext::try_new(
                "tenant-a",
                PrincipalRef::try_new("issuer", "subject", Some("tenant-a")).expect("principal"),
                "oidc",
                "high",
                "policy-v1",
                "decision-v1",
                None,
            )
            .expect("security"),
            None,
            RunLimits::empty(),
            RunPropagationPolicy {
                cancellation: CancellationPropagation::Cascade,
                deadline: finstack_ai_kernel::DeadlinePropagation::MinimumOfParentAndChild,
                budget: BudgetPropagation::SharedScope,
                principal: PrincipalPropagation::Inherit,
            },
            Digest::raw_json(b"agent"),
            None,
        )
        .expect("accepted")
    }

    fn command(ordinal: u64) -> (TransitionEnv, KernelInput) {
        (
            TransitionEnv {
                now: Timestamp::from_unix_ms(1_000).expect("timestamp"),
                ids: AllocatedIds::try_new(
                    vec![id(ordinal)],
                    vec![id(ordinal)],
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    vec![id(ordinal)],
                    Vec::new(),
                )
                .expect("ids"),
            },
            KernelInput::AcceptRun(AcceptRun {
                session_id: id(1),
                lane_id: id(2),
                accepted: accepted(),
            }),
        )
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("runtime")
    }

    #[test]
    fn bounded_channel_applies_backpressure() {
        runtime().block_on(async {
            let store = Arc::new(BlockingStore::new(false));
            let mut owner = RunTaskOwner::spawn(
                CommitCoordinator::new(store.clone()),
                RunTaskConfig {
                    command_capacity: 1,
                    event_hub: EventHubConfig {
                        source_capacity: 8,
                        max_subscribers: 4,
                    },
                    shutdown_deadline: Duration::from_millis(100),
                },
            )
            .expect("owner");
            let handle = owner.handle();
            let first_handle = handle.clone();
            let first = tokio::spawn(async move {
                let (env, input) = command(10);
                first_handle.submit(env, input).await
            });
            while store.started.load(Ordering::Acquire) == 0 {
                tokio::task::yield_now().await;
            }
            let second_handle = handle.clone();
            let second = tokio::spawn(async move {
                let (env, input) = command(11);
                second_handle.submit(env, input).await
            });
            tokio::task::yield_now().await;
            let (env, input) = command(12);
            assert!(
                tokio::time::timeout(Duration::from_millis(5), handle.submit(env, input),)
                    .await
                    .is_err()
            );
            store.release.notify_waiters();
            let _ = first.await.expect("first task");
            let _ = second.await.expect("second task");
            owner.shutdown().await;
            assert_eq!(handle.status(), RunStatus::Stopped);
        });
    }

    #[test]
    fn shutdown_is_idempotent_and_handle_drop_does_not_cancel() {
        runtime().block_on(async {
            let store = Arc::new(BlockingStore::new(false));
            let mut owner = RunTaskOwner::spawn(
                CommitCoordinator::new(store),
                RunTaskConfig {
                    command_capacity: 1,
                    event_hub: EventHubConfig {
                        source_capacity: 8,
                        max_subscribers: 4,
                    },
                    shutdown_deadline: Duration::from_millis(100),
                },
            )
            .expect("owner");
            let handle = owner.handle();
            let clone = handle.clone();
            drop(clone);
            assert_eq!(handle.status(), RunStatus::Running);
            handle.shutdown();
            handle.shutdown();
            assert_eq!(handle.status(), RunStatus::ShuttingDown);
            let (env, input) = command(20);
            assert_eq!(
                handle.submit(env, input).await.expect_err("closed"),
                RunHandleError::ShuttingDown
            );
            owner.shutdown().await;
            owner.shutdown().await;
            assert_eq!(handle.status(), RunStatus::Stopped);
        });
    }

    #[test]
    fn shutdown_deadline_aborts_active_store_wait() {
        runtime().block_on(async {
            let store = Arc::new(BlockingStore::new(true));
            let mut owner = RunTaskOwner::spawn(
                CommitCoordinator::new(store.clone()),
                RunTaskConfig {
                    command_capacity: 1,
                    event_hub: EventHubConfig {
                        source_capacity: 8,
                        max_subscribers: 4,
                    },
                    shutdown_deadline: Duration::from_millis(5),
                },
            )
            .expect("owner");
            let handle = owner.handle();
            let submit_handle = handle.clone();
            let submit = tokio::spawn(async move {
                let (env, input) = command(30);
                submit_handle.submit(env, input).await
            });
            while store.started.load(Ordering::Acquire) == 0 {
                tokio::task::yield_now().await;
            }
            owner.shutdown().await;
            assert_eq!(handle.status(), RunStatus::Stopped);
            assert!(matches!(
                submit.await.expect("submit task"),
                Err(RunHandleError::Stopped | RunHandleError::ShuttingDown)
            ));
        });
    }

    #[test]
    fn repeated_idle_owners_join_every_owned_task_without_abort() {
        runtime().block_on(async {
            for _ in 0..128 {
                let store = Arc::new(BlockingStore::new(false));
                let mut owner = RunTaskOwner::spawn(
                    CommitCoordinator::new(store),
                    RunTaskConfig {
                        command_capacity: 1,
                        event_hub: EventHubConfig {
                            source_capacity: 1,
                            max_subscribers: 1,
                        },
                        shutdown_deadline: Duration::from_millis(100),
                    },
                )
                .expect("owner");
                let handle = owner.handle();

                let report = owner.shutdown().await;

                assert_eq!(report.outcome, ShutdownOutcome::Graceful);
                assert_eq!(report.signalled_effects, 0);
                assert_eq!(report.aborted_tasks, 0);
                assert_eq!(handle.status(), RunStatus::Stopped);
            }
        });
    }
}
