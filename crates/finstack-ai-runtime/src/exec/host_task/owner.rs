use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::coordinator::{CommitCoordinator, PostCommitDispatcher};
use crate::event_hub::event_hub;
use crate::host_driver;
use crate::run_types::{
    ModelTaskConfig, RunHandleError, RunStatus, RunTaskConfig, ShutdownOutcome, ShutdownReport,
    ToolTaskConfig,
};
use crate::settlement::{
    SettlementSources, apply_interaction_resume, drain_idle_cancellation, model_handle_error,
    prepare_tool_batch_if_ready, resume_pending_model_effect, resume_pending_tool_effects,
    validate_model_binding,
};
use crate::{
    CancellationSignal, Clock, LockedModelContextProfile, MODEL_RECONCILIATION_UNSUPPORTED, Model,
    ModelResumeAction, ModelWarmupContext, RandomSource, ResolvedToolCatalog,
    TOOL_RECONCILIATION_UNSUPPORTED, ToolResumeAction, ToolStreamAssembler,
};

use super::dispatcher::HostDispatcher;
use super::fault::wait_until_stopped;
use super::handle::RunHandle;
use super::shared::Shared;
use super::worker::{run_worker, run_worker_with_effects};

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
        // Cloned before the move: the worker needs the locked profile to
        // assemble `StageInput::BeforeModel`, and the dispatcher takes ownership.
        let stage_profile = profile.clone();
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
            stage_driver,
            stage_profile,
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
