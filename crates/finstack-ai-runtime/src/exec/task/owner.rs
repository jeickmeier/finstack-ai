use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use finstack_ai_kernel::{EffectId, OperationLocator, Timestamp};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinSet;
use tokio::time::timeout;

use crate::commit::CommitCoordinator;
use crate::event_hub::event_hub;
use crate::events::{EventSubscriptionConfig, EventSubscriptionError};
use crate::ids::{Clock, RandomSource};
use crate::native::model::{ModelDispatcher, run_model_jobs};
use crate::native::timer::{TimerDispatcher, run_timer_jobs};
use crate::native::tool::{
    RuntimeDispatcher, ToolDispatcher, ToolExecutionContext, ToolTaskConfig, run_tool_jobs,
};
use crate::observer::{
    OBSERVER_DELIVERY_FAILED, OBSERVER_SHUTDOWN_TIMEOUT, OBSERVER_SUBSCRIPTION_FAILED,
};
use crate::ports::model::{CancellationSignal, LockedModelContextProfile, Model, ReadyModel};
use crate::ports::observer::{Observer, ObserverDiagnostic};
use crate::ports::tool::{
    ResolvedToolCatalog, TOOL_RECONCILIATION_UNSUPPORTED, ToolStreamAssembler,
};
use crate::run::MonotonicDeadline;
use crate::run_types::{
    ModelTaskConfig, RunHandleError, RunLifecycle, RunTaskConfig, ShutdownOutcome, ShutdownReport,
};
use crate::settlement::{
    NestedSamplingPorts, SettlementSources, apply_interaction_resume, drain_idle_cancellation,
    drive_due_polls, model_resume_retry_seed, next_due_poll_or_expiry, prepare_tool_batch_if_ready,
    resume_pending_model_effect, resume_pending_tool_effects, tool_resume_retry_seeds,
    validate_model_binding,
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
    let Some(seed) = model_resume_retry_seed(action, coordinator.pending_model_seed())? else {
        return Ok(());
    };
    dispatcher
        .resume_request(seed)
        .await
        .map_err(|error| RunHandleError::Model {
            code: Arc::from(error.code),
        })
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
    let Some(seeds) = tool_resume_retry_seeds(action, coordinator.pending_tool_seeds())? else {
        return Ok(());
    };
    for seed in seeds {
        dispatcher
            .resume_call(seed)
            .await
            .map_err(|error| RunHandleError::Tool {
                code: Arc::from(error.code),
            })?;
    }
    Ok(())
}

/// Replace the sibling poll wait with the latest committed or live-only deadline.
pub(super) fn arm_due_poll_wait(
    coordinator: &CommitCoordinator,
    schedules: &watch::Sender<Option<Timestamp>>,
    process_local_deadlines: &BTreeMap<EffectId, Option<Timestamp>>,
) {
    let deadline = next_due_poll_or_expiry(coordinator.state(), process_local_deadlines);
    schedules.send_replace(deadline);
}

/// Wake-up outcome emitted by the process-local deferred-poll waiter.
#[derive(Clone, Copy)]
pub(super) enum DuePollWake {
    /// No poll deadline has elapsed.
    Idle,
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
    mut schedules: watch::Receiver<Option<Timestamp>>,
    fired: watch::Sender<DuePollWake>,
) {
    let mut wait: Option<MonotonicDeadline> = None;
    loop {
        let due = async {
            match wait.as_ref() {
                Some(active) => active.wait().await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            () = cancellation.cancelled() => break,
            schedule = schedules.changed() => {
                if schedule.is_err() {
                    break;
                }
                let schedule = *schedules.borrow_and_update();
                wait = match schedule.map(|deadline| due_poll_wait(clock.as_ref(), deadline)) {
                    Some(Ok(DuePollWait::Waiting(next))) => Some(next),
                    Some(Ok(DuePollWait::Due)) => {
                        fired.send_replace(DuePollWake::Due);
                        None
                    }
                    Some(Err(())) => {
                        fired.send_replace(DuePollWake::ConstructionFailed);
                        None
                    }
                    None => None,
                };
            }
            () = due => {
                fired.send_replace(DuePollWake::Due);
                wait = None;
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
    observer_tasks: JoinSet<()>,
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
        let (shared, handle) = Shared::spawn(sender, event_handle, coordinator.state());
        coordinator.install_live_state_publisher(shared.clone());
        coordinator.run_control = Some(Arc::clone(&shared.control));
        let mut tasks = JoinSet::new();
        tasks.spawn(run_worker(coordinator, receiver, shared));
        tasks.spawn(event_task.run());
        Ok(Self {
            handle,
            tasks,
            observer_tasks: JoinSet::new(),
            active_effects: Vec::new(),
            run_cancellation: CancellationSignal::new(),
            shutdown_deadline: config.shutdown_deadline,
            joined: false,
        })
    }

    /// Spawn the bounded commit/model workers with a prepared model.
    ///
    /// The ready handle is not returned until the default-no-op or provider
    /// Model jobs can only be enqueued by the coordinator after the request
    /// and effect records are committed/applied.
    ///
    /// # Errors
    ///
    /// Returns configuration errors before publishing a run handle.
    pub async fn spawn_with_model<C, R>(
        coordinator: CommitCoordinator,
        run_config: RunTaskConfig,
        model_config: ModelTaskConfig,
        ready_model: Arc<ReadyModel>,
        profile: LockedModelContextProfile,
        clock: C,
        random: R,
    ) -> Result<Self, RunHandleError>
    where
        C: Clock + Send + Sync + 'static,
        R: RandomSource + Send + Sync + 'static,
    {
        Self::spawn_with_model_and_artifacts(
            coordinator,
            run_config,
            model_config,
            ready_model,
            profile,
            None,
            clock,
            random,
        )
        .await
    }

    /// Start the model owner with an optional artifact ownership service.
    ///
    /// # Errors
    ///
    /// Returns configuration, binding, recovery, or worker-start errors before
    /// publishing a run handle.
    #[expect(
        clippy::too_many_arguments,
        reason = "the constructor receives explicit port configuration, artifact ownership, and injected identity sources"
    )]
    pub async fn spawn_with_model_and_artifacts<C, R>(
        coordinator: CommitCoordinator,
        run_config: RunTaskConfig,
        model_config: ModelTaskConfig,
        ready_model: Arc<ReadyModel>,
        profile: LockedModelContextProfile,
        artifact_store: Option<(Arc<dyn crate::artifact::ArtifactStore>, OperationLocator)>,
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
            ready_model,
            profile,
            artifact_store,
            clock,
            random,
        ))
        .await
    }

    #[expect(
        clippy::too_many_arguments,
        clippy::too_many_lines,
        reason = "timer resume, model resume, and worker spawn stay contiguous with explicit dependencies"
    )]
    async fn spawn_with_model_inner<C, R>(
        mut coordinator: CommitCoordinator,
        run_config: RunTaskConfig,
        model_config: ModelTaskConfig,
        ready_model: Arc<ReadyModel>,
        profile: LockedModelContextProfile,
        artifact_store: Option<(Arc<dyn crate::artifact::ArtifactStore>, OperationLocator)>,
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
        let model = ready_model.shared_model();
        validate_model_binding(model.as_ref(), &profile)?;
        let mut sources = SettlementSources::try_new(clock, random)?;
        if let Some((store, locator)) = artifact_store {
            sources.attach_artifact_store(store, locator);
        }
        sources.reconcile_recovered_artifacts(&coordinator).await?;
        sources.set_approval_grant(run_config.approval_grant);
        let runtime_clock = sources.clock();
        let run_cancellation = CancellationSignal::new();
        let model_cancellation = run_cancellation.child();
        let timer_cancellation = run_cancellation.child();
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
        let cancelling = coordinator.state().cancellation().is_some();
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

        let (shared, handle) = Shared::spawn(sender, event_handle, coordinator.state());
        coordinator.install_live_state_publisher(shared.clone());
        coordinator.run_control = Some(Arc::clone(&shared.control));
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
            observer_tasks: JoinSet::new(),
            active_effects: vec![model_active, timer_active],
            run_cancellation,
            shutdown_deadline: run_config.shutdown_deadline,
            joined: false,
        })
    }

    /// Spawn the combined bounded model/tool runtime with a prepared model.
    ///
    /// Tool effects are routed only after their request records commit and the
    /// coordinator repeats its authorization/deadline check. The kernel remains
    /// the sole owner of execution groups and durable source ordering.
    ///
    /// # Errors
    ///
    /// Returns configuration or binding errors before publishing a run handle.
    #[expect(
        clippy::too_many_arguments,
        reason = "the public constructor receives the two explicit port configurations and injected identity sources"
    )]
    pub async fn spawn_with_model_and_tools<C, R>(
        coordinator: CommitCoordinator,
        run_config: RunTaskConfig,
        model_config: ModelTaskConfig,
        tool_config: ToolTaskConfig,
        ready_model: Arc<ReadyModel>,
        profile: LockedModelContextProfile,
        catalog: Arc<ResolvedToolCatalog>,
        clock: C,
        random: R,
    ) -> Result<Self, RunHandleError>
    where
        C: Clock + Send + Sync + 'static,
        R: RandomSource + Send + Sync + 'static,
    {
        Self::spawn_with_model_tools_and_artifacts(
            coordinator,
            run_config,
            model_config,
            tool_config,
            ready_model,
            profile,
            catalog,
            None,
            clock,
            random,
        )
        .await
    }

    /// Start the model/tool owner with an optional artifact ownership service.
    ///
    /// # Errors
    ///
    /// Returns configuration, binding, recovery, or worker-start errors before
    /// publishing a run handle.
    #[expect(
        clippy::too_many_arguments,
        reason = "the constructor receives explicit model, tool, artifact, and identity dependencies"
    )]
    pub async fn spawn_with_model_tools_and_artifacts<C, R>(
        coordinator: CommitCoordinator,
        run_config: RunTaskConfig,
        model_config: ModelTaskConfig,
        tool_config: ToolTaskConfig,
        ready_model: Arc<ReadyModel>,
        profile: LockedModelContextProfile,
        catalog: Arc<ResolvedToolCatalog>,
        artifact_store: Option<(Arc<dyn crate::artifact::ArtifactStore>, OperationLocator)>,
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
            ready_model,
            profile,
            catalog,
            artifact_store,
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
        ready_model: Arc<ReadyModel>,
        profile: LockedModelContextProfile,
        catalog: Arc<ResolvedToolCatalog>,
        artifact_store: Option<(Arc<dyn crate::artifact::ArtifactStore>, OperationLocator)>,
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
        let model = ready_model.shared_model();
        validate_model_binding(model.as_ref(), &profile)?;
        let mut sources = SettlementSources::try_new(clock, random)?;
        if let Some((store, locator)) = artifact_store {
            sources.attach_artifact_store(store, locator);
        }
        sources.reconcile_recovered_artifacts(&coordinator).await?;
        sources.set_approval_grant(run_config.approval_grant);
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
        let (sender, receiver) = mpsc::channel(run_config.command_capacity);
        let (model_job_sender, model_job_receiver) = mpsc::channel(model_config.job_capacity);
        let (model_result_sender, model_result_receiver) =
            mpsc::channel(model_config.result_capacity);
        let (tool_job_sender, tool_job_receiver) = mpsc::channel(tool_config.job_capacity);
        let (tool_result_sender, tool_result_receiver) = mpsc::channel(tool_config.result_capacity);
        let (timer_job_sender, timer_job_receiver) = mpsc::channel(run_config.command_capacity);
        let (timer_result_sender, timer_result_receiver) =
            mpsc::channel(run_config.command_capacity);
        let (due_poll_schedule_sender, due_poll_schedule_receiver) = watch::channel(None);
        let (due_poll_fired_sender, due_poll_fired_receiver) = watch::channel(DuePollWake::Idle);
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
        let cancelling = coordinator.state().cancellation().is_some();
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
            );
        }

        let (shared, handle) = Shared::spawn(sender, event_handle, coordinator.state());
        coordinator.install_live_state_publisher(shared.clone());
        coordinator.run_control = Some(Arc::clone(&shared.control));
        let due_poll_clock = sources.clock();
        let startup_tools =
            tool_dispatcher
                .finish_startup()
                .map_err(|error| RunHandleError::Tool {
                    code: Arc::from(error.code),
                })?;
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
        tasks.spawn(startup_tools);
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
            observer_tasks: JoinSet::new(),
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

    /// Take the latest confirmed structural session head retained by this owner.
    ///
    /// Callers should consume this only after graceful, non-faulted shutdown.
    #[must_use]
    pub fn take_session_head(&mut self) -> Option<crate::session::SessionHeadUpdate> {
        self.handle
            .shared
            .session_head
            .lock()
            .ok()
            .and_then(|mut update| update.take())
    }

    /// Take the latest validated process-local compaction checkpoint.
    #[doc(hidden)]
    #[must_use]
    pub fn take_compaction_checkpoint(
        &mut self,
    ) -> Option<crate::ports::middleware::CompactionCheckpoint> {
        self.handle
            .shared
            .compaction_checkpoint
            .lock()
            .ok()
            .and_then(|mut checkpoint| checkpoint.take())
    }

    /// Attach one observer pump to this owner's shutdown and abort lifecycle.
    ///
    /// # Errors
    ///
    /// Returns a subscription error when the event hub is closed, full, or
    /// the supplied delivery configuration is invalid.
    pub async fn attach_observer(
        &mut self,
        observer: Arc<dyn Observer>,
        config: EventSubscriptionConfig,
    ) -> Result<(), EventSubscriptionError> {
        let mut subscription = match self.handle.subscribe_observer(config).await {
            Ok(subscription) => subscription,
            Err(error) => {
                self.handle.record_observer_diagnostic(ObserverDiagnostic {
                    code: OBSERVER_SUBSCRIPTION_FAILED,
                    detail: "observer subscription failed",
                });
                return Err(error);
            }
        };
        let handle = self.handle.clone();
        self.observer_tasks.spawn(async move {
            while let Some(batch) = subscription.next_batch().await {
                if observer.observe(Arc::from(batch.events())).await.is_err() {
                    handle.record_observer_diagnostic(ObserverDiagnostic {
                        code: OBSERVER_DELIVERY_FAILED,
                        detail: "observer delivery failed",
                    });
                    break;
                }
            }
            subscription.close();
        });
        Ok(())
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
            self.handle.shared.publish_stopped_unless_faulted();
        }
        let observers_joined = timeout(self.shutdown_deadline, async {
            while self.observer_tasks.join_next().await.is_some() {}
        })
        .await
        .is_ok();
        if !observers_joined {
            self.handle.record_observer_diagnostic(ObserverDiagnostic {
                code: OBSERVER_SHUTDOWN_TIMEOUT,
                detail: "observer shutdown timed out",
            });
            aborted_tasks = aborted_tasks.saturating_add(self.observer_tasks.len());
            self.observer_tasks.abort_all();
            while self.observer_tasks.join_next().await.is_some() {}
        }
        let graceful = joined && observers_joined;
        let report = ShutdownReport {
            outcome: if graceful {
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
            let aborted_tasks = self.tasks.len().saturating_add(self.observer_tasks.len());
            self.tasks.abort_all();
            self.observer_tasks.abort_all();
            self.handle.shared.publish_stopped_unless_faulted();
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use finstack_ai_kernel::Timestamp;
    use tokio::sync::watch;

    use super::{CancellationSignal, Clock, DuePollWake, run_due_poll_waits};

    #[derive(Clone, Copy)]
    struct TestClock(Timestamp);

    impl Clock for TestClock {
        fn now(&self) -> Result<Timestamp, crate::ids::IdGenerationError> {
            Ok(self.0)
        }
    }

    #[tokio::test]
    async fn due_poll_waiter_consumes_latest_schedule_with_pending_wake() {
        let cancellation = CancellationSignal::new();
        let (schedule_sender, schedule_receiver) = watch::channel(None);
        let (fired_sender, mut fired_receiver) = watch::channel(DuePollWake::Idle);
        let task = tokio::spawn(run_due_poll_waits(
            Arc::new(TestClock(
                Timestamp::from_unix_ms(2_000).expect("timestamp"),
            )),
            cancellation.child(),
            schedule_receiver,
            fired_sender,
        ));
        let due = Some(Timestamp::from_unix_ms(2_000).expect("timestamp"));

        schedule_sender.send_replace(due);
        tokio::time::timeout(Duration::from_secs(1), fired_receiver.changed())
            .await
            .expect("first wake")
            .expect("first wake sender");
        assert!(matches!(
            *fired_receiver.borrow_and_update(),
            DuePollWake::Due
        ));
        schedule_sender.send_replace(due);
        tokio::time::timeout(Duration::from_secs(1), async {
            while !fired_receiver.has_changed().expect("wake sender") {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("second wake");
        fired_receiver.changed().await.expect("second wake sender");
        assert!(matches!(
            *fired_receiver.borrow_and_update(),
            DuePollWake::Due
        ));
        schedule_sender.send_replace(None);
        schedule_sender.send_replace(due);
        tokio::time::timeout(Duration::from_secs(1), fired_receiver.changed())
            .await
            .expect("latest schedule wake")
            .expect("latest wake sender");
        assert!(matches!(
            *fired_receiver.borrow_and_update(),
            DuePollWake::Due
        ));

        cancellation.cancel();
        task.await.expect("wait task");
    }
}
