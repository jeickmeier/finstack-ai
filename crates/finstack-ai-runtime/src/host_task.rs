//! Sequential host-driven run owner for `wasm-host` builds.
//!
//! One local task owns commit intake, inline model/tool dispatch, and the
//! local event hub. This is not a Tokio `JoinSet` port.

use std::collections::{BTreeMap, VecDeque};
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use finstack_ai_kernel::{
    EffectId, EffectInput, KernelInput, PostCommitAction, ReducerStageOutcome, RunPhase,
    ToolCallPlan, TransitionEnv,
};

use crate::coordinator::{
    CommitCoordinator, DispatchError, ModelDispatchSeed, PostCommitDispatcher, RuntimeDispatch,
    ToolDispatchSeed,
};
use crate::event_hub::{EventHubHandle, event_hub};
use crate::host_driver::{self, Signal};
use crate::run_types::{
    ModelTaskConfig, RunHandleError, RunStatus, RunTaskConfig, ShutdownOutcome, ShutdownReport,
    TimerDiagnostics, ToolTaskConfig,
};
use crate::settlement::{
    ModelDriverResult, SettlementSources, ToolDriverResult, apply_interaction_resume,
    drain_idle_cancellation, model_handle_error, prepare_tool_batch_if_ready,
    process_model_progress, process_model_result, process_tool_progress, process_tool_result,
    resume_pending_model_effect, resume_pending_tool_effects, validate_model_binding,
};
use crate::tool::AssembledToolTerminal;
use crate::{
    CancellationSignal, Clock, CommitCoordinatorError, CommitOutcome, EventSubscription,
    EventSubscriptionConfig, EventSubscriptionError, LockedModelContextProfile,
    MODEL_RECONCILIATION_UNSUPPORTED, Model, ModelCallContext, ModelError, ModelRequest,
    ModelRequestDraft, ModelResumeAction, ModelStreamAssembler, ModelTerminal, ModelWarmupContext,
    PortFuture, RandomSource, ResolvedTool, ResolvedToolCatalog, RunCallContext,
    TOOL_RECONCILIATION_UNSUPPORTED, ToolCallContext, ToolError, ToolResumeAction,
    ToolStreamAssembler, validate_model_request,
};

/// Cloneable bounded command/status/shutdown handle.
#[derive(Clone)]
pub struct RunHandle {
    shared: Arc<Shared>,
}

impl RunHandle {
    /// Register an interactive subscription before publishing later run events.
    ///
    /// # Errors
    ///
    /// Rejects invalid configuration, exhausted subscriber capacity, or a closed hub.
    #[expect(
        clippy::unused_async,
        reason = "matches native RunHandle so Agent can await both owners"
    )]
    pub async fn subscribe_events(
        &self,
        config: EventSubscriptionConfig,
    ) -> Result<EventSubscription, EventSubscriptionError> {
        self.shared.events.subscribe_interactive(config)
    }

    /// Register a read-only observer subscription.
    ///
    /// The wasm-host hub has a single subscriber set; isolation is enforced on
    /// the native Tokio hub.
    ///
    /// # Errors
    ///
    /// Rejects invalid configuration, exhausted subscriber capacity, or a closed hub.
    #[expect(
        clippy::unused_async,
        reason = "matches native RunHandle so Agent can await both owners"
    )]
    pub async fn subscribe_observer(
        &self,
        config: EventSubscriptionConfig,
    ) -> Result<EventSubscription, EventSubscriptionError> {
        self.shared.events.subscribe_observer(config)
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
        let intake = self
            .shared
            .intake
            .lock()
            .map_err(|_| RunHandleError::IntakeClosed)?
            .clone()
            .ok_or(RunHandleError::ShuttingDown)?;
        let (reply, receive) = oneshot();
        intake.push(RunCommand { env, input, reply }).await?;
        receive.await
    }

    /// Initiate idempotent shutdown and close shared intake.
    pub fn shutdown(&self) {
        if !self.shared.shutting_down.swap(true, Ordering::AcqRel) {
            if let Ok(intake) = self.shared.intake.lock()
                && let Some(intake) = intake.as_ref()
            {
                intake.close();
            }
            if !matches!(
                self.status(),
                RunStatus::Faulted { .. } | RunStatus::Stopped
            ) {
                self.set_status(RunStatus::ShuttingDown);
            }
            self.shared.work.notify_waiters();
        }
    }

    /// Read the latest lifecycle state without blocking.
    #[must_use]
    pub fn status(&self) -> RunStatus {
        self.shared.status.lock().map_or(
            RunStatus::Faulted {
                code: "run_status_lock_poisoned",
            },
            |status| *status,
        )
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

    fn set_status(&self, status: RunStatus) {
        if let Ok(mut current) = self.shared.status.lock() {
            *current = status;
        }
        self.shared.status_changed.notify_waiters();
    }
}

/// Single owner of the sequential host-driven run worker.
pub struct RunTaskOwner {
    handle: RunHandle,
    run_cancellation: CancellationSignal,
    shutdown_deadline: Duration,
    joined: bool,
}

impl RunTaskOwner {
    /// Spawn one bounded run worker on the host driver.
    ///
    /// # Errors
    ///
    /// Returns [`RunHandleError::InvalidConfiguration`] for zero bounds.
    pub fn spawn(
        mut coordinator: CommitCoordinator,
        config: RunTaskConfig,
    ) -> Result<Self, RunHandleError> {
        let config = config.validate()?;
        let (event_handle, ()) =
            event_hub(config.event_hub).map_err(|_| RunHandleError::InvalidConfiguration)?;
        coordinator.install_event_publisher(Arc::new(event_handle.clone()));
        let shared = Shared::new(event_handle, config.command_capacity);
        let handle = RunHandle {
            shared: Arc::clone(&shared),
        };
        let intake = shared
            .intake
            .lock()
            .map_err(|_| RunHandleError::IntakeClosed)?
            .clone()
            .ok_or(RunHandleError::InvalidConfiguration)?;
        host_driver::spawn(Box::pin(run_worker(
            coordinator,
            intake,
            Arc::clone(&shared),
        )))
        .map_err(|_| RunHandleError::InvalidConfiguration)?;
        Ok(Self {
            handle,
            run_cancellation: CancellationSignal::new(),
            shutdown_deadline: config.shutdown_deadline,
            joined: false,
        })
    }

    /// Warm one retained model and start the sequential commit/model owner.
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
        C: Clock + crate::PortObject,
        R: RandomSource + crate::PortObject,
    {
        Self::spawn_inner(
            coordinator,
            run_config,
            model_config,
            None,
            model,
            profile,
            None,
            clock,
            random,
        )
        .await
    }

    /// Warm one retained model and start the sequential model/tool owner.
    ///
    /// Sequential tool execution is enough for the wasm-host preview. Native
    /// parallel tool scheduling is not ported.
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
        C: Clock + crate::PortObject,
        R: RandomSource + crate::PortObject,
    {
        Self::spawn_inner(
            coordinator,
            run_config,
            model_config,
            Some(tool_config),
            model,
            profile,
            Some(catalog),
            clock,
            random,
        )
        .await
    }

    #[expect(
        clippy::too_many_arguments,
        clippy::too_many_lines,
        reason = "internal constructor keeps model, tool, and interaction resume wiring contiguous"
    )]
    async fn spawn_inner<C, R>(
        mut coordinator: CommitCoordinator,
        run_config: RunTaskConfig,
        model_config: ModelTaskConfig,
        tool_config: Option<ToolTaskConfig>,
        model: Arc<dyn Model>,
        profile: LockedModelContextProfile,
        catalog: Option<Arc<ResolvedToolCatalog>>,
        clock: C,
        random: R,
    ) -> Result<Self, RunHandleError>
    where
        C: Clock + crate::PortObject,
        R: RandomSource + crate::PortObject,
    {
        let run_config = run_config.validate()?;
        let (event_handle, ()) =
            event_hub(run_config.event_hub).map_err(|_| RunHandleError::InvalidConfiguration)?;
        coordinator.install_event_publisher(Arc::new(event_handle.clone()));
        let model_assembler = model_config.validate()?;
        let tool_assembler = tool_config
            .map(|config| {
                config
                    .validate()
                    .map_err(|_| RunHandleError::InvalidConfiguration)?;
                Ok(ToolStreamAssembler::new(config.stream_limits))
            })
            .transpose()?;
        validate_model_binding(model.as_ref(), &profile)?;
        let sources = SettlementSources::try_new(clock, random)?;
        let run_cancellation = CancellationSignal::new();
        let parent = run_cancellation.child();
        model
            .warmup(ModelWarmupContext {
                cancellation: parent.child(),
                deadline: model_config.warmup_deadline,
                metadata: model_config.warmup_metadata.clone(),
            })
            .await
            .map_err(|error| model_handle_error(&error))?;

        let pending = Arc::new(Mutex::new(VecDeque::new()));
        let active = Arc::new(Mutex::new(BTreeMap::new()));
        let dispatcher = Arc::new(HostDispatcher {
            model: Arc::clone(&model),
            profile,
            catalog: catalog.clone(),
            pending: Arc::clone(&pending),
            active: Arc::clone(&active),
            parent: parent.clone(),
        });
        let cancelling = coordinator.state().cancellation.is_some();
        if cancelling {
            drain_idle_cancellation(&mut coordinator, &sources, true).await?;
        } else {
            let action =
                resume_pending_model_effect(&mut coordinator, model.as_ref(), &sources, &parent)
                    .await?;
            match action {
                ModelResumeAction::Retry => {
                    let seed = coordinator.pending_model_seed().ok_or(
                        RunHandleError::ModelSettlement {
                            code: "model_resume_seed_missing",
                        },
                    )?;
                    dispatcher
                        .resume_request(seed)
                        .map_err(|error| RunHandleError::Model {
                            code: Arc::from(error.code),
                        })?;
                }
                ModelResumeAction::SuspendUncertain => {
                    return Err(RunHandleError::Model {
                        code: Arc::from(MODEL_RECONCILIATION_UNSUPPORTED),
                    });
                }
                ModelResumeAction::NoOutstanding
                | ModelResumeAction::UseRecorded
                | ModelResumeAction::Reconcile
                | ModelResumeAction::WaitExternal => {}
            }
        }
        coordinator.install_dispatcher(Arc::clone(&dispatcher) as Arc<dyn PostCommitDispatcher>);
        if !cancelling {
            apply_interaction_resume(&mut coordinator, &sources).await?;
        }
        if !cancelling && let Some(catalog) = catalog.as_ref() {
            let opened_tool_batch =
                prepare_tool_batch_if_ready(&mut coordinator, catalog, &sources).await?;
            let action = if opened_tool_batch {
                ToolResumeAction::NoOutstanding
            } else {
                resume_pending_tool_effects(&mut coordinator, catalog, &sources, &parent).await?
            };
            match action {
                ToolResumeAction::Retry => {
                    for seed in coordinator.pending_tool_seeds() {
                        dispatcher
                            .resume_call(seed)
                            .map_err(|error| RunHandleError::Tool {
                                code: Arc::from(error.code),
                            })?;
                    }
                }
                ToolResumeAction::SuspendUncertain => {
                    return Err(RunHandleError::Tool {
                        code: Arc::from(TOOL_RECONCILIATION_UNSUPPORTED),
                    });
                }
                ToolResumeAction::NoOutstanding
                | ToolResumeAction::UseRecorded
                | ToolResumeAction::Reconcile
                | ToolResumeAction::WaitExternal => {}
            }
        }

        let shared = Shared::new(event_handle, run_config.command_capacity);
        let handle = RunHandle {
            shared: Arc::clone(&shared),
        };
        let intake = shared
            .intake
            .lock()
            .map_err(|_| RunHandleError::IntakeClosed)?
            .clone()
            .ok_or(RunHandleError::InvalidConfiguration)?;
        host_driver::spawn(Box::pin(run_worker_with_effects(
            coordinator,
            intake,
            Arc::clone(&shared),
            model,
            model_assembler,
            tool_assembler,
            catalog,
            pending,
            active,
            sources,
        )))
        .map_err(|_| RunHandleError::InvalidConfiguration)?;
        Ok(Self {
            handle,
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

    /// Close intake and wait for the local worker to stop.
    pub async fn shutdown(&mut self) -> ShutdownReport {
        if self.joined {
            return self.handle.shutdown_report().unwrap_or(ShutdownReport {
                outcome: ShutdownOutcome::Graceful,
                signalled_effects: 0,
                aborted_tasks: 0,
            });
        }
        self.handle.shutdown();
        self.run_cancellation.cancel();
        let deadline = self.shutdown_deadline;
        let stopped = host_driver::timeout(deadline, wait_until_stopped(&self.handle))
            .await
            .is_ok();
        if !stopped && !matches!(self.handle.status(), RunStatus::Faulted { .. }) {
            self.handle.set_status(RunStatus::Stopped);
        }
        let report = ShutdownReport {
            outcome: if stopped {
                ShutdownOutcome::Graceful
            } else {
                ShutdownOutcome::Forced
            },
            signalled_effects: 0,
            aborted_tasks: usize::from(!stopped),
        };
        if let Ok(mut value) = self.handle.shared.shutdown_report.lock() {
            *value = Some(report);
        }
        self.joined = true;
        report
    }
}

impl Drop for RunTaskOwner {
    fn drop(&mut self) {
        if !self.joined {
            self.handle.shutdown();
            self.run_cancellation.cancel();
            if !matches!(self.handle.status(), RunStatus::Faulted { .. }) {
                self.handle.set_status(RunStatus::Stopped);
            }
            if let Ok(mut value) = self.handle.shared.shutdown_report.lock() {
                *value = Some(ShutdownReport {
                    outcome: ShutdownOutcome::OwnerDropped,
                    signalled_effects: 0,
                    aborted_tasks: 1,
                });
            }
        }
    }
}

struct Shared {
    intake: Mutex<Option<Arc<CommandIntake>>>,
    shutting_down: AtomicBool,
    status: Mutex<RunStatus>,
    status_changed: Signal,
    events: EventHubHandle,
    shutdown_report: Mutex<Option<ShutdownReport>>,
    timer_already_due: AtomicU64,
    timer_backward_clock_clamped: AtomicU64,
    work: Signal,
}

impl Shared {
    fn new(events: EventHubHandle, capacity: usize) -> Arc<Self> {
        Arc::new(Self {
            intake: Mutex::new(Some(Arc::new(CommandIntake::new(capacity)))),
            shutting_down: AtomicBool::new(false),
            status: Mutex::new(RunStatus::Running),
            status_changed: Signal::new(),
            events,
            shutdown_report: Mutex::new(None),
            timer_already_due: AtomicU64::new(0),
            timer_backward_clock_clamped: AtomicU64::new(0),
            work: Signal::new(),
        })
    }
}

struct CommandIntake {
    capacity: usize,
    queue: Mutex<VecDeque<RunCommand>>,
    available: Signal,
    space: Signal,
    closed: AtomicBool,
}

impl CommandIntake {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            queue: Mutex::new(VecDeque::new()),
            available: Signal::new(),
            space: Signal::new(),
            closed: AtomicBool::new(false),
        }
    }

    fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.available.notify_waiters();
        self.space.notify_waiters();
    }

    async fn push(&self, command: RunCommand) -> Result<(), RunHandleError> {
        loop {
            if self.closed.load(Ordering::Acquire) {
                return Err(RunHandleError::ShuttingDown);
            }
            {
                let mut queue = self
                    .queue
                    .lock()
                    .map_err(|_| RunHandleError::IntakeClosed)?;
                if queue.len() < self.capacity {
                    queue.push_back(command);
                    self.available.notify_waiters();
                    return Ok(());
                }
            }
            self.space.notified().await;
        }
    }

    async fn recv(&self) -> Option<RunCommand> {
        loop {
            let notified = self.available.notified();
            if let Ok(mut queue) = self.queue.lock()
                && let Some(command) = queue.pop_front()
            {
                self.space.notify_waiters();
                return Some(command);
            }
            if self.closed.load(Ordering::Acquire) {
                return None;
            }
            notified.await;
        }
    }
}

struct RunCommand {
    env: TransitionEnv,
    input: KernelInput,
    reply: OneshotSender<Result<CommitOutcome, RunHandleError>>,
}

enum HostWork {
    Model {
        seed: ModelDispatchSeed,
        request: ModelRequest,
    },
    Tool {
        seed: ToolDispatchSeed,
        context: ToolCallContext,
        resolved: Arc<ResolvedTool>,
    },
}

struct HostDispatcher {
    model: Arc<dyn Model>,
    profile: LockedModelContextProfile,
    catalog: Option<Arc<ResolvedToolCatalog>>,
    pending: Arc<Mutex<VecDeque<HostWork>>>,
    active: Arc<Mutex<BTreeMap<EffectId, CancellationSignal>>>,
    parent: CancellationSignal,
}

impl HostDispatcher {
    fn enqueue(&self, work: HostWork) -> Result<(), DispatchError> {
        self.pending
            .lock()
            .map_err(|_| DispatchError {
                code: "host_work_queue_unavailable",
            })?
            .push_back(work);
        Ok(())
    }

    fn parse_and_validate(
        &self,
        raw: &finstack_ai_kernel::RawJson,
    ) -> Result<ModelRequestDraft, ModelError> {
        let draft: ModelRequestDraft = serde_json::from_slice(raw.as_bytes()).map_err(|_| {
            ModelError::try_new(
                crate::MODEL_REQUEST_INVALID,
                finstack_ai_kernel::ErrorCategory::Validation,
                false,
                "committed model request draft is invalid",
                finstack_ai_kernel::Metadata::empty(),
            )
            .expect("frozen model request error")
        })?;
        if draft.canonical_bytes()?.as_slice() != raw.as_bytes() {
            return Err(ModelError::try_new(
                crate::MODEL_REQUEST_INVALID,
                finstack_ai_kernel::ErrorCategory::Validation,
                false,
                "model request draft is not the canonical committed DTO",
                finstack_ai_kernel::Metadata::empty(),
            )
            .expect("frozen model request error"));
        }
        validate_model_request(self.model.as_ref(), &draft, &self.profile)?;
        Ok(draft)
    }

    fn resolved_for_seed(
        &self,
        seed: &ToolDispatchSeed,
    ) -> Result<Arc<ResolvedTool>, DispatchError> {
        let catalog = self.catalog.as_ref().ok_or(DispatchError {
            code: "unsupported_effect_driver",
        })?;
        let resolved = catalog
            .by_id(&seed.call.tool_id)
            .cloned()
            .ok_or(DispatchError {
                code: "tool_resolution_missing",
            })?;
        let EffectInput::Tool { call: committed } = seed.requested.input() else {
            return Err(DispatchError {
                code: "tool_dispatch_contract_mismatch",
            });
        };
        if seed.call.call.tool_name() != resolved.spec.model_name.as_ref()
            || committed != &seed.call.call
            || seed.call.output_contract != resolved.output_contract
            || seed.call.execution != resolved.spec.execution
            || seed.call.retry_safety != resolved.spec.retry_safety
            || seed.call.failure_policy != resolved.policy.failure_policy
        {
            return Err(DispatchError {
                code: "tool_dispatch_contract_mismatch",
            });
        }
        Ok(resolved)
    }

    fn resume_request(&self, seed: ModelDispatchSeed) -> Result<(), DispatchError> {
        self.enqueue_model(seed.pending.requested.effect_id(), seed)
    }

    fn enqueue_model(
        &self,
        effect_id: EffectId,
        seed: ModelDispatchSeed,
    ) -> Result<(), DispatchError> {
        let EffectInput::Model { request: raw } = seed.pending.requested.input() else {
            return Err(DispatchError {
                code: "model_request_invalid",
            });
        };
        let draft = self
            .parse_and_validate(raw)
            .map_err(|error| DispatchError {
                code: stable_dispatch_code(error.code()),
            })?;
        let cancellation = self.parent.child();
        {
            let Ok(mut active) = self.active.lock() else {
                return Err(DispatchError {
                    code: "model_effect_registry_unavailable",
                });
            };
            if active.insert(effect_id, cancellation.clone()).is_some() {
                return Err(DispatchError {
                    code: "model_effect_already_active",
                });
            }
        }
        let request = ModelRequest {
            call: ModelCallContext {
                run: RunCallContext {
                    locator: seed.locator.clone(),
                    authorization: seed.authorization.clone(),
                    effect_id,
                    attempt: seed.attempt,
                    deadline: seed.pending.requested.deadline(),
                    budget_scope_id: seed.budget_scope_id,
                    cancellation,
                },
                request_id: seed.pending.model_request_id,
            },
            draft,
            continuation_state: None,
        };
        self.enqueue(HostWork::Model { seed, request })
    }

    fn resume_call(&self, seed: ToolDispatchSeed) -> Result<(), DispatchError> {
        self.enqueue_tool(seed.requested.effect_id(), seed)
    }

    fn dispatch_model(
        &self,
        effect_id: EffectId,
        seed: ModelDispatchSeed,
    ) -> PortFuture<Result<(), DispatchError>> {
        let result = self.enqueue_model(effect_id, seed);
        Box::pin(async move { result })
    }

    fn enqueue_tool(
        &self,
        effect_id: EffectId,
        seed: ToolDispatchSeed,
    ) -> Result<(), DispatchError> {
        let resolved = self.resolved_for_seed(&seed)?;
        let cancellation = self.parent.child();
        {
            let Ok(mut active) = self.active.lock() else {
                return Err(DispatchError {
                    code: "tool_effect_registry_unavailable",
                });
            };
            if active.insert(effect_id, cancellation.clone()).is_some() {
                return Err(DispatchError {
                    code: "tool_effect_already_active",
                });
            }
        }
        let context = ToolCallContext {
            run: RunCallContext {
                locator: seed.locator.clone(),
                authorization: seed.authorization.clone(),
                effect_id,
                attempt: seed.attempt,
                deadline: seed.requested.deadline(),
                budget_scope_id: seed.budget_scope_id,
                cancellation,
            },
            tool_batch_id: seed.tool_batch_id,
            tool_call_id: seed.tool_call_id,
        };
        self.enqueue(HostWork::Tool {
            seed,
            context,
            resolved,
        })
    }

    fn dispatch_tool(
        &self,
        effect_id: EffectId,
        seed: ToolDispatchSeed,
    ) -> PortFuture<Result<(), DispatchError>> {
        let result = self.enqueue_tool(effect_id, seed);
        Box::pin(async move { result })
    }
}

impl PostCommitDispatcher for HostDispatcher {
    fn validate_before_commit(&self, input: &KernelInput) -> Result<(), DispatchError> {
        if let KernelInput::StageSettled(settled) = input {
            if let ReducerStageOutcome::ModelRequestPrepared { request, .. } = &settled.outcome {
                self.parse_and_validate(request)
                    .map(|_| ())
                    .map_err(|error| DispatchError {
                        code: stable_dispatch_code(error.code()),
                    })?;
            }
            if let ReducerStageOutcome::ToolBatchPrepared { calls, .. } = &settled.outcome {
                let catalog = self.catalog.as_ref().ok_or(DispatchError {
                    code: "unsupported_effect_driver",
                })?;
                for plan in calls.iter() {
                    let ToolCallPlan::Execute(call) = plan else {
                        continue;
                    };
                    let resolved = catalog.by_id(&call.tool_id).ok_or(DispatchError {
                        code: "tool_resolution_missing",
                    })?;
                    if call.call.tool_name() != resolved.spec.model_name.as_ref()
                        || call.output_contract != resolved.output_contract
                        || call.execution != resolved.spec.execution
                        || call.retry_safety != resolved.spec.retry_safety
                        || call.failure_policy != resolved.policy.failure_policy
                    {
                        return Err(DispatchError {
                            code: "tool_dispatch_contract_mismatch",
                        });
                    }
                }
            }
        }
        Ok(())
    }

    fn dispatch(&self, dispatch: RuntimeDispatch) -> PortFuture<Result<(), DispatchError>> {
        match dispatch.action {
            PostCommitAction::CancelEffect { effect_id } => {
                match crate::coordinator::cancel_registered_effect(
                    self.active.as_ref(),
                    effect_id,
                    "host_effect_registry_unavailable",
                ) {
                    Ok(Some(signal)) => signal.cancel(),
                    Ok(None) => {}
                    Err(error) => return Box::pin(async move { Err(error) }),
                }
                Box::pin(async { Ok(()) })
            }
            PostCommitAction::ExecuteEffect { effect_id } => {
                if let Some(seed) = dispatch.model {
                    return self.dispatch_model(effect_id, seed);
                }
                if let Some(seed) = dispatch.tool {
                    return self.dispatch_tool(effect_id, seed);
                }
                Box::pin(async {
                    Err(DispatchError {
                        code: "unsupported_effect_driver",
                    })
                })
            }
        }
    }
}

async fn run_worker(
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
async fn run_worker_with_effects<C, R>(
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
) where
    C: Clock + crate::PortObject,
    R: RandomSource + crate::PortObject,
{
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
        if submit_and_reply(&mut coordinator, &shared, command).await {
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
        if let Err(error) = drain_effects_accepting_commands(
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
        )
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

async fn submit_and_reply(
    coordinator: &mut CommitCoordinator,
    shared: &Arc<Shared>,
    command: RunCommand,
) -> bool {
    let result = coordinator
        .submit(command.env, command.input)
        .await
        .map_err(RunHandleError::Coordinator);
    let fault_code = result_fault_code(&result);
    command.reply.send(result);
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
                prepare_tool_batch_if_ready(coordinator, catalog, sources).await?;
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
                settle_driven_model(
                    coordinator,
                    intake,
                    shared,
                    model,
                    model_assembler,
                    catalog,
                    active,
                    sources,
                    seed,
                    request,
                )
                .await?;
            }
            HostWork::Tool {
                seed,
                context,
                resolved,
            } => {
                settle_driven_tool(
                    coordinator,
                    intake,
                    shared,
                    tool_assembler,
                    active,
                    sources,
                    seed,
                    context,
                    resolved,
                )
                .await?;
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
        prepare_tool_batch_if_ready(coordinator, catalog, sources).await?;
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
    tool_assembler: Option<ToolStreamAssembler>,
    active: &Mutex<BTreeMap<EffectId, CancellationSignal>>,
    sources: &SettlementSources<C, R>,
    seed: ToolDispatchSeed,
    context: ToolCallContext,
    resolved: Arc<ResolvedTool>,
) -> Result<(), RunHandleError>
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
    process_tool_result(coordinator, ToolDriverResult { seed, result }, sources).await?;
    Ok(())
}

async fn drive_accepting_commands<T>(
    coordinator: &mut CommitCoordinator,
    intake: &CommandIntake,
    shared: &Arc<Shared>,
    drive: impl Future<Output = T>,
) -> Result<T, RunHandleError> {
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
                if submit_and_reply(coordinator, shared, *command).await {
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
            |item| {
                progress.push(item);
                core::future::ready(Ok(()))
            },
        )
        .await;
    (progress, result)
}

fn model_cancellation_error(message: &'static str) -> ModelError {
    ModelError::try_new(
        "model_cancelled",
        finstack_ai_kernel::ErrorCategory::Cancellation,
        false,
        message,
        finstack_ai_kernel::Metadata::empty(),
    )
    .expect("frozen cancellation error")
}

fn stable_dispatch_code(code: &str) -> &'static str {
    match code {
        crate::MODEL_REQUEST_INVALID => crate::MODEL_REQUEST_INVALID,
        crate::MODEL_PROFILE_INVALID => crate::MODEL_PROFILE_INVALID,
        crate::MODEL_PROFILE_RELAXATION => crate::MODEL_PROFILE_RELAXATION,
        crate::MODEL_PROFILE_OVERRIDE_NOT_ALLOWED => crate::MODEL_PROFILE_OVERRIDE_NOT_ALLOWED,
        crate::MODEL_ESTIMATOR_MISMATCH => crate::MODEL_ESTIMATOR_MISMATCH,
        crate::MODEL_CONTEXT_LIMIT_EXCEEDED => crate::MODEL_CONTEXT_LIMIT_EXCEEDED,
        _ => "model_request_invalid",
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

fn fault_shared(shared: &Shared, code: &'static str) {
    shared.shutting_down.store(true, Ordering::Release);
    if let Ok(mut intake) = shared.intake.lock() {
        intake.take();
    }
    if let Ok(mut status) = shared.status.lock() {
        *status = RunStatus::Faulted { code };
    }
    shared.status_changed.notify_waiters();
    shared.work.notify_waiters();
}

fn finish_worker(shared: &Shared) {
    if !matches!(
        shared
            .status
            .lock()
            .map_or(RunStatus::Stopped, |status| *status),
        RunStatus::Faulted { .. }
    ) {
        if let Ok(mut status) = shared.status.lock() {
            *status = RunStatus::Stopped;
        }
        shared.status_changed.notify_waiters();
    }
    shared.events.close();
}

async fn wait_until_stopped(handle: &RunHandle) {
    loop {
        if matches!(
            handle.status(),
            RunStatus::Stopped | RunStatus::Faulted { .. }
        ) {
            return;
        }
        handle.shared.status_changed.notified().await;
    }
}

fn oneshot<T>() -> (OneshotSender<T>, OneshotReceiver<T>) {
    let inner = Arc::new(OneshotInner {
        value: Mutex::new(None),
        signal: Signal::new(),
    });
    (
        OneshotSender {
            inner: Arc::clone(&inner),
        },
        OneshotReceiver { inner },
    )
}

struct OneshotInner<T> {
    value: Mutex<Option<T>>,
    signal: Signal,
}

struct OneshotSender<T> {
    inner: Arc<OneshotInner<T>>,
}

impl<T> OneshotSender<T> {
    fn send(self, value: T) {
        if let Ok(mut slot) = self.inner.value.lock() {
            *slot = Some(value);
        }
        self.inner.signal.notify_waiters();
    }
}

struct OneshotReceiver<T> {
    inner: Arc<OneshotInner<T>>,
}

impl<T> Future for OneshotReceiver<T> {
    type Output = T;

    fn poll(self: std::pin::Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if let Ok(mut value) = self.inner.value.lock()
            && let Some(value) = value.take()
        {
            return Poll::Ready(value);
        }
        let wait = self.inner.signal.notified();
        let mut wait = std::pin::pin!(wait);
        match wait.as_mut().poll(cx) {
            Poll::Ready(()) => {
                if let Ok(mut value) = self.inner.value.lock()
                    && let Some(value) = value.take()
                {
                    Poll::Ready(value)
                } else {
                    cx.waker().wake_by_ref();
                    Poll::Pending
                }
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet, VecDeque};
    use std::sync::atomic::AtomicU64;
    use std::task::{Context, Poll, Waker};

    use finstack_ai_kernel::{
        AcceptRun, AllocatedIds, AppendRequest, BudgetPropagation, CancellationPropagation,
        CommittedBatch, ContentBlock, Digest, Id, IdTag, KernelInput, Message, MessageRole,
        Metadata, OutputSpec, PrincipalPropagation, PrincipalRef, ProviderIds, RawJson,
        ReducerStageOutcome, RunAccepted, RunLimits, RunPropagationPolicy, RunRelation,
        RunSecurityContext, Stage, StageCursor, StageSettled, TextBlock, Timestamp, TransitionEnv,
    };
    use futures_core::Stream;

    use super::*;
    use crate::event_hub::{
        EventBatchConfig, EventFilter, EventHubConfig, EventLagPolicy, EventSubscriptionConfig,
        ProgressCoalescing,
    };
    use crate::{
        InputCapabilities, JournalStore, LoadRequest, LoadedSession, ModelCapabilities,
        ModelContextProfile, ModelDescriptor, ModelEventStream, ModelName, ModelProgress,
        ModelResponse, ModelSettings, ModelStreamItem, ModelTokenEstimate, ModelWarmupContext,
        PortFuture, SnapshotReceipt, SnapshotRequest, StoreError, StoreHealth,
        StructuredOutputCapability, TextDelta, TokenEstimatorRef, TokenEstimatorSource, Usage,
        resolve_model_context_profile,
    };

    fn block_on<F: Future>(future: F) -> F::Output {
        let mut future = std::pin::pin!(future);
        let waker = Waker::noop().clone();
        let mut cx = Context::from_waker(&waker);
        loop {
            host_driver::drive_local();
            if let Poll::Ready(output) = future.as_mut().poll(&mut cx) {
                return output;
            }
            host_driver::drive_local();
        }
    }

    struct MemoryStore {
        batches: Mutex<Vec<CommittedBatch>>,
        requests: Mutex<BTreeMap<finstack_ai_kernel::AppendBatchId, AppendRequest>>,
    }

    impl MemoryStore {
        fn new() -> Self {
            Self {
                batches: Mutex::new(Vec::new()),
                requests: Mutex::new(BTreeMap::new()),
            }
        }
    }

    impl JournalStore for MemoryStore {
        fn append(&self, request: AppendRequest) -> PortFuture<Result<CommittedBatch, StoreError>> {
            let mut requests = self.requests.lock().expect("requests");
            if let Some(existing) = requests.get(&request.batch_id()) {
                if existing != &request {
                    return Box::pin(async {
                        Err(StoreError::Corruption {
                            reason_code: "batch_reuse",
                        })
                    });
                }
                let committed = self
                    .batches
                    .lock()
                    .expect("batches")
                    .iter()
                    .find(|batch| batch.batch_id == request.batch_id())
                    .cloned()
                    .expect("indexed batch");
                return Box::pin(async move { Ok(committed) });
            }
            let committed = commit_request(&request);
            requests.insert(request.batch_id(), request);
            self.batches
                .lock()
                .expect("batches")
                .push(committed.clone());
            Box::pin(async move { Ok(committed) })
        }

        fn load(&self, request: LoadRequest) -> PortFuture<Result<LoadedSession, StoreError>> {
            let batches = self
                .batches
                .lock()
                .expect("batches")
                .iter()
                .filter(|batch| {
                    batch
                        .records
                        .first()
                        .is_some_and(|record| record.session_id() == request.session_id)
                })
                .cloned()
                .collect::<Vec<_>>();
            let head_sequence = batches.last().map_or(0, |batch| batch.last_sequence);
            Box::pin(async move {
                Ok(LoadedSession {
                    session_id: request.session_id,
                    head_sequence,
                    head_checksum: batches
                        .last()
                        .and_then(|batch| batch.records.last().map(|record| record.checksum())),
                    metadata: finstack_ai_kernel::Metadata::empty(),
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

    struct CompletingModel {
        profile: ModelContextProfile,
        warmup: AtomicU64,
        requests: AtomicU64,
    }

    impl CompletingModel {
        fn new(profile: ModelContextProfile) -> Self {
            Self {
                profile,
                warmup: AtomicU64::new(0),
                requests: AtomicU64::new(0),
            }
        }
    }

    impl Model for CompletingModel {
        fn descriptor(&self) -> ModelDescriptor {
            ModelDescriptor {
                provider: Arc::clone(&self.profile.provider),
                models: Arc::from([self.profile.model.clone()]),
                metadata: Metadata::empty(),
            }
        }

        fn capabilities(&self, _model: &ModelName) -> ModelCapabilities {
            ModelCapabilities {
                input: InputCapabilities {
                    text: true,
                    json: true,
                    images: false,
                    audio: false,
                    files: false,
                },
                context_profile: self.profile.clone(),
                native_tool_calls: false,
                parallel_tool_calls: false,
                structured_output: StructuredOutputCapability::Unsupported,
                reasoning: false,
                prompt_cache: false,
                resumable_stream: false,
                idempotent_requests: true,
                native_capabilities: BTreeSet::new(),
            }
        }

        fn estimate_input_tokens(
            &self,
            _model: &ModelName,
            canonical_request: &[u8],
        ) -> Result<ModelTokenEstimate, ModelError> {
            Ok(ModelTokenEstimate {
                input_tokens: u64::try_from(canonical_request.len()).unwrap_or(1).max(1),
                estimator: self.profile.estimator.clone(),
            })
        }

        fn warmup(&self, _ctx: ModelWarmupContext) -> PortFuture<Result<(), ModelError>> {
            self.warmup.fetch_add(1, Ordering::AcqRel);
            Box::pin(async { Ok(()) })
        }

        fn request(
            &self,
            _request: ModelRequest,
        ) -> PortFuture<Result<ModelEventStream, ModelError>> {
            self.requests.fetch_add(1, Ordering::AcqRel);
            Box::pin(async {
                Ok(Box::pin(OnceStream::new(vec![
                    Ok(ModelStreamItem::TextDelta(TextDelta {
                        text: Arc::from("hello"),
                    })),
                    Ok(ModelStreamItem::Completed(ModelResponse {
                        assistant_content: Arc::from([ContentBlock::Text(
                            TextBlock::try_new("hello").expect("text"),
                        )]),
                        tool_calls: Arc::from([]),
                        usage: Usage::empty(),
                        provider_ids: ProviderIds::empty(),
                        completion_id: Arc::from("host-completion"),
                        continuation_state: None,
                    })),
                ])) as ModelEventStream)
            })
        }
    }

    struct OnceStream<T> {
        items: VecDeque<T>,
    }

    impl<T> Unpin for OnceStream<T> {}

    impl<T> OnceStream<T> {
        fn new(items: Vec<T>) -> Self {
            Self {
                items: items.into(),
            }
        }
    }

    impl<T> Stream for OnceStream<T> {
        type Item = T;

        fn poll_next(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Option<Self::Item>> {
            Poll::Ready(self.get_mut().items.pop_front())
        }
    }

    struct TestClock {
        now: AtomicU64,
    }

    impl Clock for TestClock {
        fn now(&self) -> Result<Timestamp, crate::IdGenerationError> {
            let ms = self.now.fetch_add(1, Ordering::AcqRel);
            Ok(Timestamp::from_unix_ms(i64::try_from(ms).expect("ms")).expect("timestamp"))
        }
    }

    struct TestRandom {
        next: AtomicU64,
    }

    impl RandomSource for TestRandom {
        fn fill_bytes(&self, buf: &mut [u8]) -> Result<(), crate::IdGenerationError> {
            for byte in buf {
                *byte =
                    u8::try_from(self.next.fetch_add(1, Ordering::AcqRel) & 0xff).expect("byte");
            }
            Ok(())
        }
    }

    fn id<T: IdTag>(ordinal: u64) -> Id<T> {
        let mut bytes = [0_u8; 16];
        bytes[6] = 0x70;
        bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        Id::from_bytes(bytes)
    }

    fn profile() -> ModelContextProfile {
        ModelContextProfile {
            provider: Arc::from("scripted"),
            model: ModelName::try_new("scripted-1").expect("model"),
            hard_input_bytes: 1_048_576,
            context_window_tokens: 1_048_576,
            max_output_tokens: 256,
            reserved_output_tokens: 256,
            provider_overhead_tokens: 32,
            estimator: TokenEstimatorRef {
                id: Arc::from("bytes-upper-bound"),
                version: Arc::from("1"),
                source: TokenEstimatorSource::ConservativeUpperBound,
            },
        }
    }

    fn env(
        ordinal: u64,
        records: usize,
        events: usize,
        effects: usize,
        turns: usize,
        model_requests: usize,
        messages: usize,
    ) -> TransitionEnv {
        let mut next = ordinal;
        fn take<T: IdTag>(next: &mut u64, count: usize) -> Vec<Id<T>> {
            (0..count)
                .map(|_| {
                    let value = id(*next);
                    *next += 1;
                    value
                })
                .collect()
        }
        TransitionEnv {
            now: Timestamp::from_unix_ms(1_000).expect("timestamp"),
            ids: AllocatedIds::try_new(
                take(&mut next, records),
                take(&mut next, events),
                take(&mut next, effects),
                Vec::new(),
                take(&mut next, messages),
                take(&mut next, turns),
                take(&mut next, model_requests),
                Vec::new(),
                Vec::new(),
                take(&mut next, 1),
                Vec::new(),
            )
            .expect("ids"),
        }
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

    fn run_config() -> RunTaskConfig {
        RunTaskConfig {
            command_capacity: 8,
            event_hub: EventHubConfig {
                source_capacity: 8,
                max_subscribers: 4,
            },
            shutdown_deadline: Duration::from_millis(50),
        }
    }

    fn model_config() -> ModelTaskConfig {
        ModelTaskConfig {
            job_capacity: 8,
            result_capacity: 8,
            stream_limits: crate::ModelStreamLimits::default(),
            warmup_deadline: None,
            warmup_metadata: Metadata::empty(),
        }
    }

    fn commit_request(request: &AppendRequest) -> CommittedBatch {
        let records = request
            .records()
            .iter()
            .enumerate()
            .map(|(offset, draft)| {
                let sequence = request.expected_sequence() + u64::try_from(offset).expect("offset");
                finstack_ai_kernel::RecordEnvelope::try_new(
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
        .expect("batch")
    }

    fn subscription() -> EventSubscriptionConfig {
        EventSubscriptionConfig {
            queue_capacity: 8,
            filter: EventFilter {
                include_durable: true,
                include_transient: true,
                kinds: Arc::from([]),
                max_sensitivity: finstack_ai_kernel::Sensitivity::Confidential,
            },
            batching: EventBatchConfig {
                flush_count: 32,
                flush_bytes: 64 * 1_024,
                flush_interval: Duration::from_millis(10),
            },
            progress_coalescing: ProgressCoalescing::Enabled,
            lag_policy: EventLagPolicy::DropProgress {
                durable_timeout: Duration::from_secs(2),
            },
        }
    }

    #[test]
    fn submit_dispatches_model_and_publishes_one_event_batch() {
        let clock: Arc<dyn Clock> = Arc::new(TestClock {
            now: AtomicU64::new(1_700_000_000_000),
        });
        let random: Arc<dyn RandomSource> = Arc::new(TestRandom {
            next: AtomicU64::new(1),
        });
        host_driver::install_clock(clock);
        host_driver::install_random(random);
        let store = Arc::new(MemoryStore::new());
        let model_profile = profile();
        let locked = resolve_model_context_profile(model_profile.clone(), None, None, false)
            .expect("profile");
        let model = Arc::new(CompletingModel::new(model_profile));
        let mut owner = block_on(RunTaskOwner::spawn_with_model(
            CommitCoordinator::new(store),
            run_config(),
            model_config(),
            model.clone(),
            locked.clone(),
            host_driver::InstalledClock,
            host_driver::InstalledRandom,
        ))
        .expect("owner");
        assert_eq!(model.warmup.load(Ordering::Acquire), 1);
        let handle = owner.handle();
        let mut events = block_on(handle.subscribe_events(subscription())).expect("subscribe");

        let session_id = id(1);
        let lane_id = id(2);
        block_on(handle.submit(
            env(10, 1, 1, 0, 0, 0, 0),
            KernelInput::AcceptRun(AcceptRun {
                session_id,
                lane_id,
                accepted: accepted(),
            }),
        ))
        .expect("accept");
        block_on(handle.submit(
            env(20, 1, 0, 0, 0, 0, 0),
            KernelInput::StageSettled(StageSettled {
                cursor: StageCursor {
                    cycle: 0,
                    stage: Stage::BeforeRun,
                },
                outcome: ReducerStageOutcome::Continue,
            }),
        ))
        .expect("before run");

        let message = Message::try_new(
            id(30),
            MessageRole::User,
            vec![ContentBlock::Text(TextBlock::try_new("hi").expect("text"))],
            Timestamp::from_unix_ms(1_000).expect("timestamp"),
            None,
            ProviderIds::empty(),
            Metadata::empty(),
        )
        .expect("message");
        block_on(handle.submit(
            env(40, 2, 0, 0, 1, 0, 0),
            KernelInput::StageSettled(StageSettled {
                cursor: StageCursor {
                    cycle: 0,
                    stage: Stage::PrepareContext,
                },
                outcome: ReducerStageOutcome::ContextPrepared {
                    messages: Arc::from([message]),
                },
            }),
        ))
        .expect("context");

        let draft = ModelRequestDraft {
            model: locked.profile.model.clone(),
            messages: Arc::from([]),
            tools: Arc::from([]),
            output: OutputSpec::PlainText,
            settings: ModelSettings {
                values: RawJson::parse(b"{}").expect("settings"),
            },
            limits: crate::ModelRequestLimits {
                max_input_bytes: locked.profile.hard_input_bytes,
                max_input_tokens: locked
                    .profile
                    .context_window_tokens
                    .saturating_sub(locked.profile.reserved_output_tokens)
                    .saturating_sub(locked.profile.provider_overhead_tokens),
                max_output_tokens: locked.profile.reserved_output_tokens,
            },
        };
        let request_json =
            RawJson::parse(draft.canonical_bytes().expect("canonical")).expect("json");
        block_on(handle.submit(
            env(50, 2, 1, 1, 0, 1, 0),
            KernelInput::StageSettled(StageSettled {
                cursor: StageCursor {
                    cycle: 0,
                    stage: Stage::BeforeModel,
                },
                outcome: ReducerStageOutcome::ModelRequestPrepared {
                    request: request_json,
                    component: None,
                    output_contract: crate::EffectOutputContract {
                        kind: crate::EffectOutputKind::ModelResponse,
                        schema_version: 1,
                        schema_digest: Digest::raw_json(
                            b"{\"kind\":\"model_response\",\"schema_version\":1}",
                        ),
                    },
                    retry_safety: finstack_ai_kernel::RetrySafety::SafeToRetry,
                    deadline: None,
                },
            }),
        ))
        .expect("model request");
        assert_eq!(model.requests.load(Ordering::Acquire), 1);

        let batch = block_on(events.next_batch()).expect("event batch");
        assert!(!batch.events().is_empty());
        assert!(batch.first_sequence() <= batch.last_sequence());
        let _ = block_on(owner.shutdown());
        let _ = ModelProgress::Text(Arc::from("keep"));
    }
}
