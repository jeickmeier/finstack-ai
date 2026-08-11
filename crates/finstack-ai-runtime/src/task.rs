//! Bounded native Tokio task ownership for one runtime coordinator.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use finstack_ai_kernel::{
    AllocatedIds, AppendBatchId, AppendBatchTag, CancellationReconciledInput,
    CancellationRequestTag, ContentBlock, EffectCompleted, EffectDeferred, EffectFailed, EffectId,
    EffectTag, ErrorCategory, EventId, EventTag, Id, IdTag, InteractionTag, KernelError,
    KernelInput, Message, MessageId, MessageRole, MessageTag, Metadata, ModelRef, ModelRequestTag,
    ModelSettled, ModelSettlement, ProviderIds, RawJson, RecordId, RecordTag, ReducerStageOutcome,
    RunPhase, Stage, StageCursor, StageSettled, ToolBatchContinuation, ToolBatchSettled,
    ToolBatchTag, ToolCallBlock, ToolCallId, ToolCallPlan, ToolCallTag, ToolFailurePolicy,
    ToolSettlement, TransitionEnv, TurnTag,
};
use thiserror::Error;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinSet;
use tokio::time::timeout;

use crate::event_hub::{EventHubHandle, event_hub};
use crate::model_runtime::{
    ModelDispatcher, ModelDriverMessage, ModelDriverResult, run_model_jobs,
};
use crate::timer_runtime::{
    TimerDispatcher, TimerDriverMessage, TimerDriverResult, run_timer_jobs,
};
use crate::tool_runtime::{
    RuntimeDispatcher, ToolDispatcher, ToolDriverMessage, ToolDriverResult, ToolExecutionContext,
    ToolTaskConfig, run_tool_jobs,
};
use crate::{
    CancellationSignal, Clock, CommitCoordinator, CommitCoordinatorError, CommitOutcome,
    DeadlineDiagnostic, EventHubConfig, EventSubscription, EventSubscriptionConfig,
    EventSubscriptionError, IdGenerationError, LockedModelContextProfile, Model,
    ModelContextProfileOverride, ModelError, ModelProgress, ModelStreamAssembler,
    ModelStreamLimits, ModelTerminal, ModelWarmupContext, RandomSource, ResolvedToolCatalog,
    ToolError, ToolProgress, ToolStreamAssembler, UuidV7Generator, normalize_tool_result,
    resolve_model_context_profile,
};

/// Observable lifecycle of one owned runtime task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStatus {
    /// Intake and commit processing are active.
    Running,
    /// New intake is closed while the current boundary finishes.
    ShuttingDown,
    /// The owned worker has stopped.
    Stopped,
    /// Runtime uncertainty faulted this run only.
    Faulted {
        /// Stable fault code.
        code: &'static str,
    },
}

/// How owned runtime tasks stopped during explicit shutdown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownOutcome {
    /// Every owned task joined within the configured grace period.
    Graceful,
    /// Remaining owned tasks were aborted after the grace period elapsed.
    Forced,
    /// Dropping the sole task owner forced immediate local termination.
    OwnerDropped,
}

/// Safe operational diagnostics for one completed shutdown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShutdownReport {
    /// Graceful or forced termination classification.
    pub outcome: ShutdownOutcome,
    /// Active effect/timer signals present when shutdown began.
    pub signalled_effects: usize,
    /// Owned tasks still present when forced termination began.
    pub aborted_tasks: usize,
}

/// Process-local diagnostics from durable wall-to-monotonic timer conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimerDiagnostics {
    /// Timers already due when installed or restored.
    pub already_due: u64,
    /// Restored timers clamped after a backward wall-clock anomaly.
    pub backward_clock_clamped: u64,
}

/// Configuration for a bounded native run task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunTaskConfig {
    /// Bounded command queue capacity.
    pub command_capacity: usize,
    /// Bounded event-hub source and subscriber limits.
    pub event_hub: EventHubConfig,
    /// Maximum time the owner waits before aborting owned tasks.
    pub shutdown_deadline: Duration,
}

/// Configuration for the private bounded model job/result path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelTaskConfig {
    /// Bounded committed model-job queue capacity.
    pub job_capacity: usize,
    /// Bounded incremental driver-message queue capacity.
    pub result_capacity: usize,
    /// Pure stream-assembler bounds.
    pub stream_limits: ModelStreamLimits,
    /// Optional construction warmup deadline.
    pub warmup_deadline: Option<finstack_ai_kernel::Timestamp>,
    /// Bounded non-secret warmup metadata.
    pub warmup_metadata: Metadata,
}

impl ModelTaskConfig {
    fn validate(&self) -> Result<ModelStreamAssembler, RunHandleError> {
        if self.job_capacity == 0 || self.result_capacity == 0 {
            return Err(RunHandleError::InvalidConfiguration);
        }
        ModelStreamAssembler::new(self.stream_limits)
            .map_err(|_| RunHandleError::InvalidConfiguration)
    }
}

impl RunTaskConfig {
    /// Validate non-zero queue and shutdown bounds.
    ///
    /// # Errors
    ///
    /// Returns [`RunHandleError::InvalidConfiguration`] for a zero bound.
    pub fn validate(self) -> Result<Self, RunHandleError> {
        if self.command_capacity == 0
            || self.shutdown_deadline.is_zero()
            || self.event_hub.validate().is_err()
        {
            return Err(RunHandleError::InvalidConfiguration);
        }
        Ok(self)
    }
}

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
            model_cancellation,
        ));
        let model_active = model_dispatcher.active();
        let timer_dispatcher = Arc::new(TimerDispatcher::new(
            Arc::clone(&runtime_clock),
            timer_job_sender,
            timer_cancellation,
        ));
        let timer_active = timer_dispatcher.active();
        if let Some(seed) = coordinator.pending_timer_seed() {
            timer_dispatcher
                .resume(seed)
                .await
                .map_err(|error| RunHandleError::Timer { code: error.code })?;
        }
        coordinator.install_dispatcher(Arc::new(RuntimeDispatcher::model_only(
            model_dispatcher,
            timer_dispatcher,
        )));

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
        tasks.spawn(run_worker_with_model(
            coordinator,
            receiver,
            result_receiver,
            timer_result_receiver,
            Arc::clone(&shared),
            sources,
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
        tasks.spawn(event_task.run());
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
        clippy::too_many_lines,
        reason = "the public constructor receives the two explicit port configurations and injected identity sources"
    )]
    pub async fn spawn_with_model_and_tools<C, R>(
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
            model_cancellation,
        ));
        let model_active = model_dispatcher.active();
        let tool_dispatcher = Arc::new(ToolDispatcher::new(
            Arc::clone(&catalog),
            tool_job_sender,
            tool_batch_cancellation,
        ));
        let tool_active = tool_dispatcher.active();
        let tool_semaphores = tool_dispatcher.semaphores();
        let timer_dispatcher = Arc::new(TimerDispatcher::new(
            Arc::clone(&runtime_clock),
            timer_job_sender,
            timer_cancellation,
        ));
        let timer_active = timer_dispatcher.active();
        if let Some(seed) = coordinator.pending_timer_seed() {
            timer_dispatcher
                .resume(seed)
                .await
                .map_err(|error| RunHandleError::Timer { code: error.code })?;
        }
        coordinator.install_dispatcher(Arc::new(RuntimeDispatcher::with_tools(
            Arc::clone(&model_dispatcher),
            tool_dispatcher,
            timer_dispatcher,
        )));

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
        tasks.spawn(run_worker_with_model_and_tools(
            coordinator,
            receiver,
            model_result_receiver,
            tool_result_receiver,
            timer_result_receiver,
            Arc::clone(&shared),
            sources,
            catalog,
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
        tasks.spawn(event_task.run());
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

/// Bounded run-handle failures.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RunHandleError {
    /// Queue capacity or shutdown deadline was zero.
    #[error("invalid run task configuration")]
    InvalidConfiguration,
    /// Shutdown has closed command intake.
    #[error("run is shutting down")]
    ShuttingDown,
    /// Owned worker has stopped.
    #[error("run has stopped")]
    Stopped,
    /// Runtime uncertainty faulted the run.
    #[error("run faulted: {code}")]
    Faulted {
        /// Stable fault code.
        code: &'static str,
    },
    /// Worker intake or reply path closed unexpectedly.
    #[error("run intake closed")]
    IntakeClosed,
    /// Coordinator rejected or faulted the submission.
    #[error(transparent)]
    Coordinator(CommitCoordinatorError),
    /// Model warmup or execution adapter failed before a durable settlement.
    #[error("model adapter failed: {code}")]
    Model {
        /// Stable adapter code.
        code: Arc<str>,
    },
    /// Runtime could not construct a valid kernel settlement.
    #[error("model settlement construction failed: {code}")]
    ModelSettlement {
        /// Stable runtime code.
        code: &'static str,
    },
    /// Tool execution adapter failed before a durable settlement.
    #[error("tool adapter failed: {code}")]
    Tool {
        /// Stable adapter code.
        code: Arc<str>,
    },
    /// Runtime could not construct a valid kernel tool settlement.
    #[error("tool settlement construction failed: {code}")]
    ToolSettlement {
        /// Stable runtime code.
        code: &'static str,
    },
    /// Durable timer adapter failed before a firing could be committed.
    #[error("timer adapter failed: {code}")]
    Timer {
        /// Stable runtime code.
        code: &'static str,
    },
    /// Cancellation reconciliation could not be normalized or committed.
    #[error("cancellation reconciliation failed: {code}")]
    CancellationSettlement {
        /// Stable runtime code.
        code: &'static str,
    },
    /// Runtime event publication failed after apply and before dispatch.
    #[error("event delivery failed: {code}")]
    EventDelivery {
        /// Stable event-delivery code.
        code: &'static str,
    },
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

struct SettlementSources<C, R> {
    clock: Arc<C>,
    random: R,
    progress_random: ProgressRandom,
}

impl<C: Clock, R: RandomSource> SettlementSources<C, R> {
    fn try_new(clock: C, random: R) -> Result<Self, RunHandleError> {
        let progress_random = ProgressRandom::try_new(&random)?;
        Ok(Self {
            clock: Arc::new(clock),
            random,
            progress_random,
        })
    }

    fn now(&self) -> Result<finstack_ai_kernel::Timestamp, RunHandleError> {
        self.clock.now().map_err(id_source_error)
    }

    fn clock(&self) -> Arc<C> {
        Arc::clone(&self.clock)
    }

    fn generate<T: IdTag>(&self) -> Result<Id<T>, RunHandleError> {
        UuidV7Generator::new(self.clock.as_ref(), &self.random)
            .generate()
            .map_err(id_source_error)
    }

    fn generate_progress_event(&self) -> Result<EventId, RunHandleError> {
        UuidV7Generator::new(self.clock.as_ref(), &self.progress_random)
            .generate()
            .map_err(id_source_error)
    }
}

struct ProgressRandom {
    seed: [u8; 32],
    counter: AtomicU64,
}

impl ProgressRandom {
    fn try_new(random: &impl RandomSource) -> Result<Self, RunHandleError> {
        let mut seed = [0_u8; 32];
        random.fill_bytes(&mut seed).map_err(id_source_error)?;
        Ok(Self {
            seed,
            counter: AtomicU64::new(0),
        })
    }
}

impl RandomSource for ProgressRandom {
    fn fill_bytes(&self, bytes: &mut [u8]) -> Result<(), IdGenerationError> {
        let mut written = 0_usize;
        while written < bytes.len() {
            let counter = self
                .counter
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                    value.checked_add(1)
                })
                .map_err(|_| {
                    IdGenerationError::Source("progress event entropy exhausted".into())
                })?;
            let mut material = [0_u8; 40];
            material[..32].copy_from_slice(&self.seed);
            material[32..].copy_from_slice(&counter.to_be_bytes());
            let digest = finstack_ai_kernel::Digest::raw_json(&material);
            let count = (bytes.len() - written).min(digest.as_bytes().len());
            bytes[written..written + count].copy_from_slice(&digest.as_bytes()[..count]);
            written += count;
        }
        Ok(())
    }
}

async fn run_worker_with_model<C, R>(
    mut coordinator: CommitCoordinator,
    mut receiver: mpsc::Receiver<RunCommand>,
    mut results: mpsc::Receiver<ModelDriverMessage>,
    mut timers: mpsc::Receiver<TimerDriverMessage>,
    shared: Arc<Shared>,
    sources: SettlementSources<C, R>,
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
                let result = coordinator
                    .submit(command.env, command.input)
                    .await
                    .map_err(RunHandleError::Coordinator);
                let fault_code = result_fault_code(&result);
                let _ = command.reply.send(result);
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
                            Ok(()) => prepare_tool_batch_if_ready(&mut coordinator, &catalog, &sources).await,
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
                let mut result = coordinator
                    .submit(command.env, command.input)
                    .await
                    .map_err(RunHandleError::Coordinator);
                if result.as_ref().is_ok_and(|outcome| outcome.fault.is_none())
                    && let Err(error) = prepare_tool_batch_if_ready(&mut coordinator, &catalog, &sources).await
                {
                    result = Err(error);
                }
                let fault_code = result_fault_code(&result);
                let _ = command.reply.send(result);
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

async fn prepare_tool_batch_if_ready<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    catalog: &ResolvedToolCatalog,
    sources: &SettlementSources<C, R>,
) -> Result<(), RunHandleError> {
    if coordinator.state().phase != Some(RunPhase::BeforeToolBatch) {
        return Ok(());
    }
    let state = coordinator.state();
    let source = state
        .messages
        .last()
        .ok_or(RunHandleError::ToolSettlement {
            code: "tool_source_message_missing",
        })?;
    let calls = source
        .content()
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolCall(call)
                if !finstack_ai_kernel::is_internal_tool_name(call.tool_name()) =>
            {
                Some(call.clone())
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    if calls.is_empty() {
        return Err(RunHandleError::ToolSettlement {
            code: "tool_source_calls_missing",
        });
    }
    let deadline = state
        .accepted
        .as_ref()
        .and_then(finstack_ai_kernel::RunAccepted::effective_deadline);
    let plans = calls
        .into_iter()
        .map(|call| catalog.plan_call(call, deadline, None))
        .collect::<Vec<_>>();
    let continuation = if state.final_result.is_some() {
        ToolBatchContinuation::Finalize
    } else {
        ToolBatchContinuation::ContinueModel
    };
    let input = KernelInput::StageSettled(StageSettled {
        cursor: StageCursor {
            cycle: state.cycle,
            stage: Stage::BeforeToolBatch,
        },
        outcome: ReducerStageOutcome::ToolBatchPrepared {
            calls: plans.clone().into(),
            continuation,
        },
    });
    let now = sources.now()?;
    let ids = allocate_tool_opening(&plans, sources)?;
    let env = TransitionEnv { now, ids };
    let decision =
        coordinator
            .classify(&env, input.clone())
            .map_err(|_| RunHandleError::ToolSettlement {
                code: "tool_opening_allocation_mismatch",
            })?;
    debug_assert_eq!(decision.records.len(), env.ids.record_ids().len());
    let outcome = coordinator
        .submit(env, input)
        .await
        .map_err(RunHandleError::Coordinator)?;
    if let Some(fault) = outcome.fault {
        return Err(RunHandleError::Faulted { code: fault.code });
    }
    Ok(())
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

async fn reconcile_cancelled_effect<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    effect_id: EffectId,
    cancelled: bool,
    sources: &SettlementSources<C, R>,
) -> Result<(), RunHandleError> {
    let request_id = coordinator
        .state()
        .cancellation
        .as_ref()
        .ok_or(RunHandleError::CancellationSettlement {
            code: "cancellation_request_missing",
        })?
        .request
        .request_id;
    let (completed_effects, cancelled_effects) = if cancelled {
        (Arc::from([]), Arc::from([effect_id]))
    } else {
        (Arc::from([effect_id]), Arc::from([]))
    };
    let input = KernelInput::CancellationReconciled(CancellationReconciledInput {
        request_id,
        completed_effects,
        cancelled_effects,
        uncertain_effects: Arc::from([]),
    });
    let now = sources.now()?;
    let ids = allocate_for_runtime_input(coordinator, now, &input, sources)?;
    let outcome = coordinator
        .submit(TransitionEnv { now, ids }, input)
        .await
        .map_err(RunHandleError::Coordinator)?;
    if let Some(fault) = outcome.fault {
        return Err(RunHandleError::Faulted { code: fault.code });
    }
    Ok(())
}

#[derive(Default)]
struct RuntimeIdAllocation {
    records: Vec<RecordId>,
    events: Vec<EventId>,
    effects: Vec<Id<EffectTag>>,
    interactions: Vec<Id<InteractionTag>>,
    messages: Vec<MessageId>,
    turns: Vec<Id<TurnTag>>,
    model_requests: Vec<Id<ModelRequestTag>>,
    tool_batches: Vec<Id<ToolBatchTag>>,
    tool_calls: Vec<Id<ToolCallTag>>,
    cancellations: Vec<Id<CancellationRequestTag>>,
}

impl RuntimeIdAllocation {
    fn freeze(&self, append_batch_id: AppendBatchId) -> Result<AllocatedIds, RunHandleError> {
        AllocatedIds::try_new(
            self.records.clone(),
            self.events.clone(),
            self.effects.clone(),
            self.interactions.clone(),
            self.messages.clone(),
            self.turns.clone(),
            self.model_requests.clone(),
            self.tool_batches.clone(),
            self.tool_calls.clone(),
            vec![append_batch_id],
            self.cancellations.clone(),
        )
        .map_err(|_| RunHandleError::CancellationSettlement {
            code: "runtime_input_ids_invalid",
        })
    }
}

fn allocate_for_runtime_input<C: Clock, R: RandomSource>(
    coordinator: &CommitCoordinator,
    now: finstack_ai_kernel::Timestamp,
    input: &KernelInput,
    sources: &SettlementSources<C, R>,
) -> Result<AllocatedIds, RunHandleError> {
    let append_batch_id = sources.generate::<AppendBatchTag>()?;
    let mut allocation = RuntimeIdAllocation::default();
    for _ in 0..finstack_ai_kernel::SEMANTIC_ARRAY_MAX_ITEMS {
        let ids = allocation.freeze(append_batch_id)?;
        match coordinator.classify(
            &TransitionEnv {
                now,
                ids: ids.clone(),
            },
            input.clone(),
        ) {
            Ok(_) => return Ok(ids),
            Err(KernelError::AllocatedIdsExhausted { kind }) => match kind {
                "record_ids" => allocation.records.push(sources.generate::<RecordTag>()?),
                "event_ids" => allocation.events.push(sources.generate::<EventTag>()?),
                "effect_ids" => allocation.effects.push(sources.generate::<EffectTag>()?),
                "interaction_ids" => allocation
                    .interactions
                    .push(sources.generate::<InteractionTag>()?),
                "message_ids" => allocation.messages.push(sources.generate::<MessageTag>()?),
                "turn_ids" => allocation.turns.push(sources.generate::<TurnTag>()?),
                "model_request_ids" => allocation
                    .model_requests
                    .push(sources.generate::<ModelRequestTag>()?),
                "tool_batch_ids" => allocation
                    .tool_batches
                    .push(sources.generate::<ToolBatchTag>()?),
                "tool_call_ids" => allocation
                    .tool_calls
                    .push(sources.generate::<ToolCallTag>()?),
                "cancellation_request_ids" => allocation
                    .cancellations
                    .push(sources.generate::<CancellationRequestTag>()?),
                _ => {
                    return Err(RunHandleError::CancellationSettlement {
                        code: "runtime_input_id_kind_unknown",
                    });
                }
            },
            Err(_) => {
                return Err(RunHandleError::CancellationSettlement {
                    code: "runtime_input_rejected",
                });
            }
        }
    }
    Err(RunHandleError::CancellationSettlement {
        code: "runtime_input_allocation_exhausted",
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ToolOpeningCounts {
    records: usize,
    events: usize,
    effects: usize,
    messages: usize,
}

fn tool_opening_counts(plans: &[ToolCallPlan]) -> Result<ToolOpeningCounts, RunHandleError> {
    let mut groups = Vec::with_capacity(plans.len());
    let mut group = 0_u32;
    for (index, plan) in plans.iter().enumerate() {
        if index > 0
            && !(plans[index - 1].execution() == finstack_ai_kernel::ToolExecutionMode::Parallel
                && plan.execution() == finstack_ai_kernel::ToolExecutionMode::Parallel)
        {
            group = group.checked_add(1).ok_or(RunHandleError::ToolSettlement {
                code: "tool_group_count_overflow",
            })?;
        }
        groups.push(group);
    }
    let first_executable_group = plans
        .iter()
        .zip(&groups)
        .find_map(|(plan, group)| matches!(plan, ToolCallPlan::Execute(_)).then_some(*group));
    let requests = first_executable_group.map_or(0, |first| {
        plans
            .iter()
            .zip(&groups)
            .filter(|(plan, group)| **group == first && matches!(plan, ToolCallPlan::Execute(_)))
            .count()
    });
    let messages = plans
        .iter()
        .take_while(|plan| matches!(plan, ToolCallPlan::SyntheticClosure(_)))
        .count();
    Ok(ToolOpeningCounts {
        records: 2 + requests + messages + usize::from(first_executable_group.is_none()),
        events: requests + 2 * messages,
        effects: plans.len(),
        messages,
    })
}

fn allocate_tool_opening<C: Clock, R: RandomSource>(
    plans: &[ToolCallPlan],
    sources: &SettlementSources<C, R>,
) -> Result<AllocatedIds, RunHandleError> {
    let counts = tool_opening_counts(plans)?;
    AllocatedIds::try_new(
        generate_tool_ids::<RecordTag, _, _>(counts.records, sources)?,
        generate_tool_ids::<EventTag, _, _>(counts.events, sources)?,
        generate_tool_ids::<EffectTag, _, _>(counts.effects, sources)?,
        Vec::new(),
        generate_tool_ids::<MessageTag, _, _>(counts.messages, sources)?,
        Vec::new(),
        Vec::new(),
        vec![generate_tool_id::<ToolBatchTag, _, _>(sources)?],
        Vec::new(),
        vec![generate_tool_id::<AppendBatchTag, _, _>(sources)?],
        Vec::new(),
    )
    .map_err(|_| RunHandleError::ToolSettlement {
        code: "tool_opening_ids_invalid",
    })
}

async fn process_tool_result<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    mut driver_result: ToolDriverResult,
    sources: &SettlementSources<C, R>,
) -> Result<(), RunHandleError> {
    let effect_id = driver_result.seed.requested.effect_id();
    let state = coordinator.state();
    let Some(batch) = state.active_tool_batch.as_ref() else {
        return Ok(());
    };
    let Some(active) = batch
        .calls
        .iter()
        .find(|call| call.assigned.effect_id == effect_id)
    else {
        return Ok(());
    };
    if let Some(cancellation) = state.cancellation.as_ref()
        && cancellation.outstanding_effects.contains(&effect_id)
    {
        let cancelled = driver_result
            .result
            .as_ref()
            .is_err_and(|error| error.category() == ErrorCategory::Cancellation);
        return reconcile_cancelled_effect(coordinator, effect_id, cancelled, sources).await;
    }
    if batch.opened.tool_batch_id != driver_result.seed.tool_batch_id
        || !matches!(
            active.status,
            finstack_ai_kernel::ActiveToolCallStatus::Requested { deferred: None, .. }
        )
        || state.terminal.is_some()
    {
        return Ok(());
    }
    let now = sources.now()?;
    if driver_result
        .seed
        .requested
        .deadline()
        .is_some_and(|deadline| deadline <= now)
    {
        driver_result.result = Err(ToolError::try_new(
            crate::TOOL_DEADLINE_EXCEEDED,
            ErrorCategory::Deadline,
            false,
            "tool result arrived after the committed deadline",
            Metadata::empty(),
        )
        .map_err(|error| tool_handle_error(&error))?);
    }
    let settled = build_tool_settlement(driver_result)?;
    let input = KernelInput::ToolBatchSettled(settled.clone());
    let allocation = allocate_tool_settlement(coordinator.state(), &settled, sources)?;
    let env = TransitionEnv {
        now,
        ids: allocation,
    };
    coordinator
        .classify(&env, input.clone())
        .map_err(|_| RunHandleError::ToolSettlement {
            code: "tool_settlement_allocation_mismatch",
        })?;
    let outcome = coordinator
        .submit(env, input)
        .await
        .map_err(RunHandleError::Coordinator)?;
    if let Some(fault) = outcome.fault {
        return Err(RunHandleError::Faulted { code: fault.code });
    }
    Ok(())
}

async fn process_tool_progress<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    effect_id: EffectId,
    progress: ToolProgress,
    sources: &SettlementSources<C, R>,
) -> Result<(), RunHandleError> {
    let state = coordinator.state();
    let Some(batch) = state.active_tool_batch.as_ref() else {
        return Ok(());
    };
    let Some(active) = batch
        .calls
        .iter()
        .find(|call| call.assigned.effect_id == effect_id)
    else {
        return Ok(());
    };
    if !matches!(
        active.status,
        finstack_ai_kernel::ActiveToolCallStatus::Requested { deferred: None, .. }
    ) || state.terminal.is_some()
        || state.cancellation.is_some()
    {
        return Ok(());
    }
    let now = sources.now()?;
    let event_id = sources.generate_progress_event()?;
    let event = coordinator
        .materialize_tool_progress(&progress, event_id, effect_id, now)
        .map_err(|code| RunHandleError::ToolSettlement { code })?;
    coordinator
        .publish_events(Arc::from([event]))
        .await
        .map_err(RunHandleError::Coordinator)
}

fn build_tool_settlement(result: ToolDriverResult) -> Result<ToolBatchSettled, RunHandleError> {
    let requested = &result.seed.requested;
    let effect_id = requested.effect_id();
    let outcome = match result.result {
        Ok(assembled) => {
            let block = normalize_tool_result(result.seed.tool_call_id, assembled.result)
                .map_err(|error| tool_handle_error(&error))?;
            let bytes = serde_json_canonicalizer::to_vec(&block).map_err(|_| {
                RunHandleError::ToolSettlement {
                    code: "tool_result_serialize_failed",
                }
            })?;
            let output = RawJson::parse(bytes).map_err(|_| RunHandleError::ToolSettlement {
                code: "tool_result_output_invalid",
            })?;
            ToolSettlement::Completed(
                EffectCompleted::try_new(
                    effect_id,
                    requested.output_contract().clone(),
                    output,
                    assembled.usage,
                    Vec::new(),
                    ProviderIds::empty(),
                    None::<&str>,
                    None,
                )
                .map_err(|_| RunHandleError::ToolSettlement {
                    code: "tool_effect_completion_invalid",
                })?,
            )
        }
        Err(error) => {
            let descriptor = error
                .to_descriptor()
                .map_err(|error| tool_handle_error(&error))?;
            ToolSettlement::Failed(
                EffectFailed::try_new(
                    effect_id,
                    requested.output_contract().clone(),
                    descriptor,
                    None,
                    None::<&str>,
                )
                .map_err(|_| RunHandleError::ToolSettlement {
                    code: "tool_effect_failure_invalid",
                })?,
            )
        }
    };
    Ok(ToolBatchSettled {
        tool_batch_id: result.seed.tool_batch_id,
        outcome,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PredictedToolStatus {
    Undispatched,
    Requested,
    Buffered,
    Settled,
}

#[expect(
    clippy::too_many_lines,
    reason = "exact allocation mirrors the kernel's contiguous-prefix and next-group cardinalities"
)]
fn allocate_tool_settlement<C: Clock, R: RandomSource>(
    state: &finstack_ai_kernel::KernelState,
    settled: &ToolBatchSettled,
    sources: &SettlementSources<C, R>,
) -> Result<AllocatedIds, RunHandleError> {
    let batch = state
        .active_tool_batch
        .as_ref()
        .ok_or(RunHandleError::ToolSettlement {
            code: "tool_settlement_batch_missing",
        })?;
    let effect_id = match &settled.outcome {
        ToolSettlement::Completed(value) => value.effect_id(),
        ToolSettlement::Failed(value) => value.effect_id(),
        ToolSettlement::Deferred(value) => value.effect_id,
    };
    let target = batch
        .calls
        .iter()
        .position(|call| call.assigned.effect_id == effect_id)
        .ok_or(RunHandleError::ToolSettlement {
            code: "tool_settlement_call_missing",
        })?;
    let mut statuses = batch
        .calls
        .iter()
        .map(|call| match call.status {
            finstack_ai_kernel::ActiveToolCallStatus::Undispatched => {
                PredictedToolStatus::Undispatched
            }
            finstack_ai_kernel::ActiveToolCallStatus::Requested { .. } => {
                PredictedToolStatus::Requested
            }
            finstack_ai_kernel::ActiveToolCallStatus::Buffered { .. } => {
                PredictedToolStatus::Buffered
            }
            finstack_ai_kernel::ActiveToolCallStatus::Settled { .. } => {
                PredictedToolStatus::Settled
            }
        })
        .collect::<Vec<_>>();
    statuses[target] = PredictedToolStatus::Buffered;
    let target_plan = &batch.calls[target].assigned.plan;
    let fatal = batch.fatal_error.is_some()
        || (matches!(settled.outcome, ToolSettlement::Failed(_))
            && target_plan.failure_policy() == ToolFailurePolicy::FailRun);
    let current_complete = batch.calls.iter().enumerate().all(|(index, call)| {
        call.assigned.group_index != batch.current_group
            || matches!(
                statuses[index],
                PredictedToolStatus::Buffered | PredictedToolStatus::Settled
            )
    });
    if fatal && current_complete {
        for status in &mut statuses {
            if *status == PredictedToolStatus::Undispatched {
                *status = PredictedToolStatus::Buffered;
            }
        }
    }
    let mut messages = 0_usize;
    let start =
        usize::try_from(batch.next_source_index).map_err(|_| RunHandleError::ToolSettlement {
            code: "tool_source_index_invalid",
        })?;
    for status in statuses.iter_mut().skip(start) {
        if *status != PredictedToolStatus::Buffered {
            break;
        }
        *status = PredictedToolStatus::Settled;
        messages += 1;
    }
    let requests = if !fatal && current_complete {
        let next_group = batch.calls.iter().enumerate().find_map(|(index, call)| {
            (statuses[index] == PredictedToolStatus::Undispatched
                && matches!(call.assigned.plan, ToolCallPlan::Execute(_)))
            .then_some(call.assigned.group_index)
        });
        next_group.map_or(0, |group| {
            batch
                .calls
                .iter()
                .enumerate()
                .filter(|(index, call)| {
                    statuses[*index] == PredictedToolStatus::Undispatched
                        && call.assigned.group_index == group
                        && matches!(call.assigned.plan, ToolCallPlan::Execute(_))
                })
                .count()
        })
    } else {
        0
    };
    let close = usize::from(
        statuses
            .iter()
            .all(|status| *status == PredictedToolStatus::Settled),
    );
    let records = 1 + messages + requests + close;
    let events = 1 + 2 * messages + requests;
    AllocatedIds::try_new(
        generate_tool_ids::<RecordTag, _, _>(records, sources)?,
        generate_tool_ids::<EventTag, _, _>(events, sources)?,
        Vec::new(),
        Vec::new(),
        generate_tool_ids::<MessageTag, _, _>(messages, sources)?,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        vec![generate_tool_id::<AppendBatchTag, _, _>(sources)?],
        Vec::new(),
    )
    .map_err(|_| RunHandleError::ToolSettlement {
        code: "tool_settlement_ids_invalid",
    })
}

fn generate_tool_ids<T: IdTag, C: Clock, R: RandomSource>(
    count: usize,
    sources: &SettlementSources<C, R>,
) -> Result<Vec<Id<T>>, RunHandleError> {
    (0..count)
        .map(|_| generate_tool_id::<T, _, _>(sources))
        .collect()
}

fn generate_tool_id<T: IdTag, C: Clock, R: RandomSource>(
    sources: &SettlementSources<C, R>,
) -> Result<Id<T>, RunHandleError> {
    sources
        .generate::<T>()
        .map_err(|_| RunHandleError::ToolSettlement {
            code: "tool_settlement_id_source_failed",
        })
}

async fn process_model_result<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    mut driver_result: ModelDriverResult,
    sources: &SettlementSources<C, R>,
) -> Result<(), RunHandleError> {
    let effect_id = driver_result.seed.pending.requested.effect_id();
    let state = coordinator.state();
    let Some(pending) = state.pending_model_effect.as_ref() else {
        return Ok(());
    };
    if let Some(cancellation) = state.cancellation.as_ref()
        && cancellation.outstanding_effects.contains(&effect_id)
    {
        let cancelled = driver_result
            .result
            .as_ref()
            .is_err_and(|error| error.category() == ErrorCategory::Cancellation);
        return reconcile_cancelled_effect(coordinator, effect_id, cancelled, sources).await;
    }
    if pending.requested.effect_id() != effect_id
        || pending.model_request_id != driver_result.seed.pending.model_request_id
        || pending.deferred.is_some()
        || state.terminal.is_some()
    {
        return Ok(());
    }
    let now = sources.now()?;
    if pending
        .requested
        .deadline()
        .is_some_and(|deadline| deadline <= now)
    {
        driver_result.result = Err(ModelError::try_new(
            "model_deadline_exceeded",
            ErrorCategory::Deadline,
            false,
            "model result arrived after the committed deadline",
            Metadata::empty(),
        )
        .map_err(|error| model_handle_error(&error))?);
    }
    let allocation = allocate_settlement(&driver_result, sources)?;
    let settled = build_settlement(driver_result, now, &allocation)?;
    let outcome = coordinator
        .submit(
            TransitionEnv {
                now,
                ids: allocation.ids,
            },
            KernelInput::ModelSettled(settled),
        )
        .await
        .map_err(RunHandleError::Coordinator)?;
    if let Some(fault) = outcome.fault {
        return Err(RunHandleError::Faulted { code: fault.code });
    }
    Ok(())
}

async fn process_model_progress<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    effect_id: EffectId,
    provider: &str,
    progress: ModelProgress,
    sources: &SettlementSources<C, R>,
) -> Result<(), RunHandleError> {
    let state = coordinator.state();
    let Some(pending) = state.pending_model_effect.as_ref() else {
        return Ok(());
    };
    if pending.requested.effect_id() != effect_id
        || pending.deferred.is_some()
        || state.terminal.is_some()
        || state.cancellation.is_some()
    {
        return Ok(());
    }
    let now = sources.now()?;
    let event_id = sources.generate_progress_event()?;
    let event = coordinator
        .materialize_model_progress(&progress, event_id, provider, now)
        .map_err(|code| RunHandleError::ModelSettlement { code })?;
    coordinator
        .publish_events(Arc::from([event]))
        .await
        .map_err(RunHandleError::Coordinator)
}

struct SettlementAllocation {
    ids: AllocatedIds,
    message_id: Option<MessageId>,
    tool_call_ids: Vec<ToolCallId>,
}

fn allocate_settlement<C: Clock, R: RandomSource>(
    result: &ModelDriverResult,
    sources: &SettlementSources<C, R>,
) -> Result<SettlementAllocation, RunHandleError> {
    let completed = matches!(result.result, Ok(ModelTerminal::Completed(_)));
    let tool_count = match &result.result {
        Ok(value) => match value {
            ModelTerminal::Completed(response) => response.tool_calls.len(),
            ModelTerminal::Deferred(_) => 0,
        },
        Err(_) => 0,
    };
    let record_count = if completed { 2 } else { 1 };
    let event_count = if completed { 2 } else { 1 };
    let records = (0..record_count)
        .map(|_| sources.generate::<RecordTag>())
        .collect::<Result<Vec<RecordId>, _>>()?;
    let events = (0..event_count)
        .map(|_| sources.generate::<EventTag>())
        .collect::<Result<Vec<EventId>, _>>()?;
    let message_id = completed
        .then(|| sources.generate::<MessageTag>())
        .transpose()?;
    let tool_call_ids = (0..tool_count)
        .map(|_| sources.generate::<ToolCallTag>())
        .collect::<Result<Vec<ToolCallId>, _>>()?;
    let append_batch_id: AppendBatchId = sources.generate::<AppendBatchTag>()?;
    let ids = AllocatedIds::try_new(
        records,
        events,
        Vec::new(),
        Vec::new(),
        message_id.into_iter().collect(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        tool_call_ids.clone(),
        vec![append_batch_id],
        Vec::new(),
    )
    .map_err(|_| RunHandleError::ModelSettlement {
        code: "model_settlement_ids_invalid",
    })?;
    Ok(SettlementAllocation {
        ids,
        message_id,
        tool_call_ids,
    })
}

#[expect(
    clippy::too_many_lines,
    reason = "terminal conversion keeps response, deferral, and failure identity continuity visible"
)]
fn build_settlement(
    result: ModelDriverResult,
    now: finstack_ai_kernel::Timestamp,
    allocation: &SettlementAllocation,
) -> Result<ModelSettled, RunHandleError> {
    let requested = &result.seed.pending.requested;
    let effect_id = requested.effect_id();
    let outcome = match result.result {
        Ok(terminal) => match terminal {
            ModelTerminal::Completed(response) => {
                let output_bytes = serde_json_canonicalizer::to_vec(&response).map_err(|_| {
                    RunHandleError::ModelSettlement {
                        code: "model_response_serialize_failed",
                    }
                })?;
                let output =
                    RawJson::parse(output_bytes).map_err(|_| RunHandleError::ModelSettlement {
                        code: "model_response_output_invalid",
                    })?;
                let completion = EffectCompleted::try_new(
                    effect_id,
                    requested.output_contract().clone(),
                    output,
                    Some(response.usage.clone()),
                    Vec::new(),
                    response.provider_ids.clone(),
                    Some(response.completion_id.as_ref()),
                    None,
                )
                .map_err(|_| RunHandleError::ModelSettlement {
                    code: "model_effect_completion_invalid",
                })?;
                let mut content = response.assistant_content.to_vec();
                if allocation.tool_call_ids.len() != response.tool_calls.len() {
                    return Err(RunHandleError::ModelSettlement {
                        code: "model_tool_call_id_cardinality",
                    });
                }
                for (call, tool_call_id) in
                    response.tool_calls.iter().zip(&allocation.tool_call_ids)
                {
                    content.push(ContentBlock::ToolCall(
                        ToolCallBlock::try_new(*tool_call_id, &call.name, call.arguments.clone())
                            .map_err(|_| RunHandleError::ModelSettlement {
                            code: "model_tool_call_invalid",
                        })?,
                    ));
                }
                let model_ref = ModelRef::try_new(&result.provider, result.draft.model.as_str())
                    .map_err(|_| RunHandleError::ModelSettlement {
                        code: "model_reference_invalid",
                    })?;
                let message_id = allocation
                    .message_id
                    .ok_or(RunHandleError::ModelSettlement {
                        code: "model_message_id_missing",
                    })?;
                let assistant_message = Message::try_new(
                    message_id,
                    MessageRole::Assistant,
                    content,
                    now,
                    Some(model_ref),
                    response.provider_ids,
                    Metadata::empty(),
                )
                .map_err(|_| RunHandleError::ModelSettlement {
                    code: "model_assistant_message_invalid",
                })?;
                ModelSettlement::Completed {
                    completion,
                    assistant_message,
                }
            }
            ModelTerminal::Deferred(deferral) => ModelSettlement::Deferred(EffectDeferred {
                effect_id,
                handle: deferral.handle,
                reconciliation: deferral.reconciliation,
                next_poll_at: deferral.next_poll_at,
                expires_at: deferral.expires_at,
                output_contract: requested.output_contract().clone(),
            }),
        },
        Err(error) => {
            let descriptor = error
                .to_descriptor()
                .map_err(|error| model_handle_error(&error))?;
            ModelSettlement::Failed(
                EffectFailed::try_new(
                    effect_id,
                    requested.output_contract().clone(),
                    descriptor,
                    None,
                    None::<&str>,
                )
                .map_err(|_| RunHandleError::ModelSettlement {
                    code: "model_effect_failure_invalid",
                })?,
            )
        }
    };
    Ok(ModelSettled {
        turn_id: result.seed.pending.turn_id,
        model_request_id: result.seed.pending.model_request_id,
        outcome,
    })
}

fn result_fault_code(result: &Result<CommitOutcome, RunHandleError>) -> Option<&'static str> {
    match result {
        Ok(outcome) => outcome.fault.map(|fault| fault.code),
        Err(
            RunHandleError::Faulted { code }
            | RunHandleError::ModelSettlement { code }
            | RunHandleError::ToolSettlement { code }
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
        RunHandleError::ToolSettlement { code } => code,
        _ => model_runtime_fault(error),
    }
}

fn model_handle_error(error: &ModelError) -> RunHandleError {
    RunHandleError::Model {
        code: Arc::from(error.code()),
    }
}

fn tool_handle_error(error: &ToolError) -> RunHandleError {
    RunHandleError::Tool {
        code: Arc::from(error.code()),
    }
}

fn validate_model_binding(
    model: &dyn Model,
    profile: &LockedModelContextProfile,
) -> Result<(), RunHandleError> {
    let descriptor = model.descriptor();
    descriptor
        .validate()
        .map_err(|error| model_handle_error(&error))?;
    if descriptor.provider != profile.profile.provider
        || !descriptor.models.contains(&profile.profile.model)
    {
        return Err(RunHandleError::Model {
            code: Arc::from(crate::MODEL_PROFILE_INVALID),
        });
    }
    let provider = model.capabilities(&profile.profile.model).context_profile;
    let effective = &profile.profile;
    let overlay = ModelContextProfileOverride {
        hard_input_bytes: Some(effective.hard_input_bytes),
        context_window_tokens: Some(effective.context_window_tokens),
        max_output_tokens: Some(effective.max_output_tokens),
        reserved_output_tokens: Some(effective.reserved_output_tokens),
        provider_overhead_tokens: Some(effective.provider_overhead_tokens),
    };
    let relocked = resolve_model_context_profile(provider, Some(&overlay), None, false)
        .map_err(|error| model_handle_error(&error))?;
    if relocked != *profile {
        return Err(RunHandleError::Model {
            code: Arc::from(crate::MODEL_PROFILE_INVALID),
        });
    }
    Ok(())
}

fn id_source_error(_error: IdGenerationError) -> RunHandleError {
    RunHandleError::ModelSettlement {
        code: "model_settlement_id_source_failed",
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
        JournalStore, LoadRequest, LoadedSession, PortFuture, SnapshotReceipt, SnapshotRequest,
        StoreError, StoreHealth,
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
            Box::pin(async move {
                Ok(LoadedSession {
                    session_id: request.session_id,
                    head_sequence: 0,
                    committed_batches: Arc::from([]),
                    snapshot: None,
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
}
