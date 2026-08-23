use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use finstack_ai_kernel::OperationLocator;

use crate::context::CONTEXT_RECOVERY_UNCERTAIN;
use crate::coordinator::{CommitCoordinator, PostCommitDispatcher};
use crate::driver::host_driver;
use crate::event_hub::event_hub;
use crate::events::{EventSubscriptionConfig, EventSubscriptionError};
use crate::ids::{Clock, RandomSource};
use crate::observer::{
    OBSERVER_DELIVERY_FAILED, OBSERVER_SHUTDOWN_TIMEOUT, OBSERVER_SUBSCRIPTION_FAILED,
};
use crate::ports::context::InvocationResumeAction;
use crate::ports::model::{CancellationSignal, LockedModelContextProfile, ReadyModel};
use crate::ports::observer::{Observer, ObserverDiagnostic};
use crate::ports::tool::{ResolvedToolCatalog, ToolResumeAction, ToolStreamAssembler};
use crate::run::LiveRunState;
use crate::run_types::{
    ModelTaskConfig, RunHandleError, RunLifecycle, RunTaskConfig, ShutdownOutcome, ShutdownReport,
    ToolTaskConfig,
};
use crate::settlement::{
    NestedSamplingPorts, SettlementSources, apply_interaction_resume, drain_idle_cancellation,
    model_resume_retry_seed, prepare_tool_batch_if_ready, resume_pending_context_effects,
    resume_pending_model_effect, resume_pending_tool_effects, tool_resume_retry_seeds,
    validate_model_binding,
};

use super::dispatcher::HostDispatcher;
use super::fault::wait_until_stopped;
use super::handle::RunHandle;
use super::shared::Shared;
use super::worker::{run_worker, run_worker_with_effects};

/// Single owner of the host-driven run worker and its bounded child tasks.
pub struct RunTaskOwner {
    handle: RunHandle,
    worker: host_driver::HostTaskHandle,
    observer_tasks: Vec<host_driver::HostTaskHandle>,
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
        let shared = Shared::new(
            event_handle,
            config.command_capacity,
            LiveRunState::initial(coordinator.state()),
            coordinator.state().clone(),
        );
        coordinator.install_live_state_publisher(shared.clone());
        let handle = RunHandle {
            shared: Arc::clone(&shared),
        };
        let intake = shared
            .intake
            .lock()
            .map_err(|_| RunHandleError::IntakeClosed)?
            .clone()
            .ok_or(RunHandleError::InvalidConfiguration)?;
        let worker = host_driver::spawn(Box::pin(run_worker(
            coordinator,
            intake,
            Arc::clone(&shared),
        )))
        .map_err(|_| RunHandleError::InvalidConfiguration)?;
        Ok(Self {
            handle,
            worker,
            observer_tasks: Vec::new(),
            run_cancellation: CancellationSignal::new(),
            shutdown_deadline: config.shutdown_deadline,
            joined: false,
        })
    }

    /// Start the sequential commit/model owner with a prepared model.
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
        C: Clock + crate::ports::PortObject,
        R: RandomSource + crate::ports::PortObject,
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

    /// Start the sequential model owner with optional artifact ownership.
    ///
    /// # Errors
    ///
    /// Returns configuration errors before publishing a run handle.
    #[expect(
        clippy::too_many_arguments,
        reason = "the constructor receives explicit model, artifact, and identity dependencies"
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
        C: Clock + crate::ports::PortObject,
        R: RandomSource + crate::ports::PortObject,
    {
        Self::spawn_inner(
            coordinator,
            run_config,
            model_config,
            None,
            ready_model,
            profile,
            None,
            artifact_store,
            clock,
            random,
        )
        .await
    }

    /// Start the model/tool owner with a prepared model.
    ///
    /// Consecutive calls admitted by the kernel as parallel execute in bounded
    /// host-driver child tasks. Sequential and barrier calls remain exclusive.
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
        C: Clock + crate::ports::PortObject,
        R: RandomSource + crate::ports::PortObject,
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

    /// Start the model/tool owner with optional artifact ownership.
    ///
    /// # Errors
    ///
    /// Returns configuration or binding errors before publishing a run handle.
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
        C: Clock + crate::ports::PortObject,
        R: RandomSource + crate::ports::PortObject,
    {
        Self::spawn_inner(
            coordinator,
            run_config,
            model_config,
            Some(tool_config),
            ready_model,
            profile,
            Some(catalog),
            artifact_store,
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
        ready_model: Arc<ReadyModel>,
        profile: LockedModelContextProfile,
        catalog: Option<Arc<ResolvedToolCatalog>>,
        artifact_store: Option<(Arc<dyn crate::artifact::ArtifactStore>, OperationLocator)>,
        clock: C,
        random: R,
    ) -> Result<Self, RunHandleError>
    where
        C: Clock + crate::ports::PortObject,
        R: RandomSource + crate::ports::PortObject,
    {
        let run_config = run_config.validate()?;
        let (event_handle, ()) =
            event_hub(run_config.event_hub).map_err(|_| RunHandleError::InvalidConfiguration)?;
        coordinator.install_event_publisher(Arc::new(event_handle.clone()));
        let model_assembler = model_config.validate()?;
        let retry_policy = model_config.same_identity_retry;
        let tool_runtime = tool_config
            .map(|config| {
                config
                    .validate()
                    .map_err(|_| RunHandleError::InvalidConfiguration)?;
                Ok((config, ToolStreamAssembler::new(config.stream_limits)))
            })
            .transpose()?;
        let (tool_config, tool_assembler) = tool_runtime
            .map_or((None, None), |(config, assembler)| {
                (Some(config), Some(assembler))
            });
        let model = ready_model.shared_model();
        validate_model_binding(model.as_ref(), &profile)?;
        let mut sources = SettlementSources::try_new(clock, random)?;
        if let Some((store, locator)) = artifact_store {
            sources.attach_artifact_store(store, locator);
        }
        sources.reconcile_recovered_artifacts(&coordinator).await?;
        sources.set_approval_grant(run_config.approval_grant);
        let run_cancellation = CancellationSignal::new();
        let parent = run_cancellation.child();
        if let Some(catalog) = catalog.clone() {
            sources.attach_nested_sampling(NestedSamplingPorts {
                model: Arc::clone(&model),
                profile: profile.clone(),
                catalog,
                cancellation: parent.child(),
            });
        }
        // The wasm host is deliberately single-threaded, but the same owned
        // scheduler graph uses `Arc` on native targets where it is `Send`.
        #[cfg_attr(
            target_arch = "wasm32",
            expect(
                clippy::arc_with_non_send_sync,
                reason = "the target-neutral owned scheduler graph is single-threaded on wasm32"
            )
        )]
        let pending = Arc::new(Mutex::new(VecDeque::new()));
        let active = Arc::new(Mutex::new(BTreeMap::new()));
        // Cloned before the move: the worker needs the locked profile to
        // assemble `StageInput::BeforeModel`, and the dispatcher takes ownership.
        let stage_profile = profile.clone();
        #[cfg_attr(
            target_arch = "wasm32",
            expect(
                clippy::arc_with_non_send_sync,
                reason = "the target-neutral owned scheduler graph is single-threaded on wasm32"
            )
        )]
        let dispatcher = Arc::new(HostDispatcher {
            model: Arc::clone(&model),
            profile,
            catalog: catalog.clone(),
            pending: Arc::clone(&pending),
            active: Arc::clone(&active),
            parent: parent.clone(),
        });
        let cancelling = coordinator.state().cancellation().is_some();
        if cancelling {
            drain_idle_cancellation(&mut coordinator, &sources, true).await?;
        } else {
            let action =
                resume_pending_model_effect(&mut coordinator, model.as_ref(), &sources, &parent)
                    .await?;
            if let Some(seed) = model_resume_retry_seed(action, coordinator.pending_model_seed())? {
                dispatcher
                    .resume_request(seed)
                    .map_err(|error| RunHandleError::Model {
                        code: Arc::from(error.code),
                    })?;
            }
            crate::compaction_driver::resume_pending_compaction_model(
                &mut coordinator,
                &sources,
                &stage_profile,
                model.as_ref(),
                &parent,
            )
            .await?;
            if let Some(providers) = coordinator.context_providers().cloned() {
                match resume_pending_context_effects(
                    &coordinator,
                    providers.as_ref(),
                    &sources,
                    &parent,
                )
                .await?
                {
                    InvocationResumeAction::SuspendUncertain => {
                        return Err(RunHandleError::Middleware {
                            code: Arc::from(CONTEXT_RECOVERY_UNCERTAIN),
                        });
                    }
                    InvocationResumeAction::UseRecorded
                    | InvocationResumeAction::Recompute
                    | InvocationResumeAction::Reconcile => {}
                }
            }
        }
        coordinator.install_dispatcher(Arc::clone(&dispatcher) as Arc<dyn PostCommitDispatcher>);
        // Built before the resume-path tool batch, not after it: a resumed run
        // must route `BeforeToolBatch` through the same chain a steady-state
        // one does, or middleware would apply on some resume paths only.
        let stage_driver = crate::stage_settlement::stage_driver(&coordinator, &run_cancellation);
        if !cancelling {
            apply_interaction_resume(&mut coordinator, &sources).await?;
        }
        if !cancelling && let Some(catalog) = catalog.as_ref() {
            let opened_tool_batch = prepare_tool_batch_if_ready(
                &mut coordinator,
                catalog,
                &sources,
                stage_driver.as_ref(),
            )
            .await?;
            let action = if opened_tool_batch {
                ToolResumeAction::NoOutstanding
            } else {
                resume_pending_tool_effects(&mut coordinator, catalog, &sources, &parent).await?
            };
            if let Some(seeds) = tool_resume_retry_seeds(action, coordinator.pending_tool_seeds())?
            {
                for seed in seeds {
                    dispatcher
                        .resume_call(seed)
                        .map_err(|error| RunHandleError::Tool {
                            code: Arc::from(error.code),
                        })?;
                }
            }
        }

        let shared = Shared::new(
            event_handle,
            run_config.command_capacity,
            LiveRunState::initial(coordinator.state()),
            coordinator.state().clone(),
        );
        coordinator.install_live_state_publisher(shared.clone());
        let handle = RunHandle {
            shared: Arc::clone(&shared),
        };
        let intake = shared
            .intake
            .lock()
            .map_err(|_| RunHandleError::IntakeClosed)?
            .clone()
            .ok_or(RunHandleError::InvalidConfiguration)?;
        let worker = host_driver::spawn(Box::pin(run_worker_with_effects(
            coordinator,
            intake,
            Arc::clone(&shared),
            model,
            model_assembler,
            tool_assembler,
            tool_config,
            catalog,
            pending,
            active,
            sources,
            stage_driver,
            stage_profile,
            retry_policy,
        )))
        .map_err(|_| RunHandleError::InvalidConfiguration)?;
        Ok(Self {
            handle,
            worker,
            observer_tasks: Vec::new(),
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
        let task = host_driver::spawn(Box::pin(async move {
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
        }))
        .map_err(|_| {
            self.handle.record_observer_diagnostic(ObserverDiagnostic {
                code: OBSERVER_SUBSCRIPTION_FAILED,
                detail: "observer task failed to start",
            });
            EventSubscriptionError::HubClosed
        })?;
        self.observer_tasks.push(task);
        Ok(())
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
        let mut aborted_tasks = 0_usize;
        if !stopped {
            if !self.worker.is_completed() {
                self.worker.abort();
                aborted_tasks = 1;
            }
            host_driver::yield_now().await;
            self.worker.completed().await;
            self.handle.shared.publish_stopped_unless_faulted();
        }
        let observers_stopped = host_driver::timeout(deadline, async {
            for task in &self.observer_tasks {
                task.completed().await;
            }
        })
        .await
        .is_ok();
        if !observers_stopped {
            self.handle.record_observer_diagnostic(ObserverDiagnostic {
                code: OBSERVER_SHUTDOWN_TIMEOUT,
                detail: "observer shutdown timed out",
            });
            for task in &self.observer_tasks {
                if !task.is_completed() {
                    task.abort();
                    aborted_tasks = aborted_tasks.saturating_add(1);
                }
            }
            host_driver::yield_now().await;
            for task in &self.observer_tasks {
                task.completed().await;
            }
        }
        let graceful = stopped && observers_stopped;
        let report = ShutdownReport {
            outcome: if graceful {
                ShutdownOutcome::Graceful
            } else {
                ShutdownOutcome::Forced
            },
            signalled_effects: 0,
            aborted_tasks,
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
            self.worker.abort();
            for task in &self.observer_tasks {
                task.abort();
            }
            self.handle.shared.publish_stopped_unless_faulted();
            if let Ok(mut value) = self.handle.shared.shutdown_report.lock() {
                *value = Some(ShutdownReport {
                    outcome: ShutdownOutcome::OwnerDropped,
                    signalled_effects: 0,
                    aborted_tasks: 0,
                });
            }
        }
    }
}
