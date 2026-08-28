//! Stateful `Lane` run, suspend, and resume orchestration.

use super::handle::Agent;
use super::run::AgentRun;
use super::types::{AGENT_RUN_INVALID_CONFIGURATION, AgentRunError, AgentRunRequest};

#[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
mod native {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    use finstack_ai_kernel::OperationLocator;
    use finstack_ai_runtime::ids::ExternalClock;
    use finstack_ai_runtime::ports::context::ContextProvider;
    use finstack_ai_runtime::ports::model::{
        LockedModelContextProfile, ModelContextProfileOverride, resolve_model_context_profile,
    };
    use finstack_ai_runtime::ports::tool::ResolvedToolCatalog;
    use finstack_ai_runtime::session::SessionError;
    use finstack_ai_runtime::workflow::WorkflowSession;

    use super::{AGENT_RUN_INVALID_CONFIGURATION, Agent, AgentRun, AgentRunError};
    use crate::agent::prepare::NativeIds;

    #[derive(Clone, Copy, PartialEq, Eq)]
    pub(super) enum LaneState {
        Running,
        Suspending,
        Suspended,
        Resuming,
    }

    pub(crate) struct LaneLive {
        pub(super) generation: u64,
        pub(super) state: LaneState,
        pub(super) run: Option<AgentRun>,
        pub(super) workflow: Option<WorkflowSession>,
    }

    fn next_generation() -> u64 {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        NEXT.fetch_add(1, Ordering::Relaxed)
    }

    fn run_is_settled(run: &AgentRun) -> bool {
        run.inner.result.lock().is_ok_and(|result| result.is_some())
    }

    fn live_is_releasable(live: &LaneLive) -> bool {
        live.state == LaneState::Running
            && live.run.as_ref().is_some_and(run_is_settled)
            && live
                .workflow
                .as_ref()
                .is_none_or(|workflow| !workflow.owner_is_live())
    }

    pub(super) fn reserve_run(lane: &crate::Lane) -> Result<u64, AgentRunError> {
        let mut lanes = lane
            .session()
            .live_lanes()
            .lock()
            .map_err(|_| AgentRunError::runtime_message("lane driver lock is poisoned"))?;
        if lanes.get(&lane.lane_id()).is_some_and(live_is_releasable) {
            lanes.remove(&lane.lane_id());
        }
        if lanes.contains_key(&lane.lane_id()) {
            return Err(session_error(&SessionError::LaneBusy));
        }
        let generation = next_generation();
        lanes.insert(
            lane.lane_id(),
            LaneLive {
                generation,
                state: LaneState::Running,
                run: None,
                workflow: None,
            },
        );
        Ok(generation)
    }

    pub(super) fn attach_run(
        lane: &crate::Lane,
        generation: u64,
        run: AgentRun,
    ) -> Result<(), AgentRunError> {
        let mut lanes = lane
            .session()
            .live_lanes()
            .lock()
            .map_err(|_| AgentRunError::runtime_message("lane driver lock is poisoned"))?;
        let live = lanes
            .get_mut(&lane.lane_id())
            .filter(|live| live.generation == generation)
            .ok_or_else(|| AgentRunError::runtime_message("lane reservation was superseded"))?;
        live.run = Some(run);
        Ok(())
    }

    pub(super) fn live_run(lane: &crate::Lane) -> Result<Option<AgentRun>, SessionError> {
        let lanes = lane
            .session()
            .live_lanes()
            .lock()
            .map_err(|_| SessionError::Poisoned)?;
        Ok(lanes
            .get(&lane.lane_id())
            .filter(|live| live.state == LaneState::Running)
            .and_then(|live| live.run.clone()))
    }

    pub(super) fn abandon(lane: &crate::Lane, generation: u64) {
        if let Ok(mut lanes) = lane.session().live_lanes().lock()
            && lanes
                .get(&lane.lane_id())
                .is_some_and(|live| live.generation == generation)
        {
            lanes.remove(&lane.lane_id());
        }
    }

    pub(super) fn begin_suspend(
        lane: &crate::Lane,
    ) -> Result<Option<(u64, Option<AgentRun>)>, SessionError> {
        let mut lanes = lane
            .session()
            .live_lanes()
            .lock()
            .map_err(|_| SessionError::Poisoned)?;
        let Some(live) = lanes.get_mut(&lane.lane_id()) else {
            return Ok(None);
        };
        match live.state {
            LaneState::Suspended => return Ok(None),
            LaneState::Running => {}
            LaneState::Suspending | LaneState::Resuming => return Err(SessionError::LaneBusy),
        }
        if live.run.is_none() {
            return Err(SessionError::LaneBusy);
        }
        live.state = LaneState::Suspending;
        if let Some(workflow) = live.workflow.as_mut() {
            workflow.abort_owner();
        }
        Ok(Some((live.generation, live.run.clone())))
    }

    pub(super) fn finish_suspend(lane: &crate::Lane, generation: u64) -> Result<(), SessionError> {
        let mut lanes = lane
            .session()
            .live_lanes()
            .lock()
            .map_err(|_| SessionError::Poisoned)?;
        if let Some(live) = lanes
            .get_mut(&lane.lane_id())
            .filter(|live| live.generation == generation)
        {
            live.state = LaneState::Suspended;
        }
        Ok(())
    }

    pub(super) fn begin_resume(
        lane: &crate::Lane,
    ) -> Result<(u64, Option<AgentRun>), AgentRunError> {
        let mut lanes = lane
            .session()
            .live_lanes()
            .lock()
            .map_err(|_| AgentRunError::runtime_message("lane driver lock is poisoned"))?;
        if let Some(live) = lanes.get_mut(&lane.lane_id()) {
            if live.state != LaneState::Suspended {
                return Err(session_error(&SessionError::LaneBusy));
            }
            live.state = LaneState::Resuming;
            return Ok((live.generation, live.run.clone()));
        }
        let generation = next_generation();
        lanes.insert(
            lane.lane_id(),
            LaneLive {
                generation,
                state: LaneState::Resuming,
                run: None,
                workflow: None,
            },
        );
        Ok((generation, None))
    }

    pub(super) fn finish_resume(
        lane: &crate::Lane,
        generation: u64,
        workflow: WorkflowSession,
    ) -> Result<(), AgentRunError> {
        let mut lanes = lane
            .session()
            .live_lanes()
            .lock()
            .map_err(|_| AgentRunError::runtime_message("lane driver lock is poisoned"))?;
        let live = lanes
            .get_mut(&lane.lane_id())
            .filter(|live| live.generation == generation)
            .ok_or_else(|| AgentRunError::runtime_message("lane resume was superseded"))?;
        live.workflow = Some(workflow);
        live.state = LaneState::Running;
        Ok(())
    }

    pub(super) fn rollback_resume(lane: &crate::Lane, generation: u64) {
        if let Ok(mut lanes) = lane.session().live_lanes().lock()
            && let Some(live) = lanes
                .get_mut(&lane.lane_id())
                .filter(|live| live.generation == generation)
        {
            live.state = LaneState::Suspended;
        }
    }

    type WorkflowPorts = (
        Arc<finstack_ai_runtime::ports::model::ReadyModel>,
        LockedModelContextProfile,
        Option<Arc<ResolvedToolCatalog>>,
    );

    pub(super) fn workflow_ports(agent: &Agent) -> Result<WorkflowPorts, AgentRunError> {
        let ready_model = Arc::clone(agent.resolved.run_plan().model().handle());
        let model = ready_model.shared_model();
        let descriptor = model.descriptor();
        descriptor.validate().map_err(AgentRunError::model)?;
        let name = descriptor.models.first().ok_or_else(|| {
            AgentRunError::configuration(
                AGENT_RUN_INVALID_CONFIGURATION,
                "resolved model advertises no names",
            )
        })?;
        let capabilities = model.capabilities(name);
        let profile = resolve_model_context_profile(
            capabilities.context_profile,
            None,
            None::<&ModelContextProfileOverride>,
            false,
        )
        .map_err(AgentRunError::model)?;
        let catalog = (!agent.tools.is_empty()).then(|| Arc::clone(&agent.tools));
        Ok((ready_model, profile, catalog))
    }

    pub(super) fn session_error(error: &SessionError) -> AgentRunError {
        AgentRunError::configuration(error.code(), error.to_string())
    }

    pub(super) fn workflow_error(
        error: &finstack_ai_runtime::workflow::WorkflowDriverError,
    ) -> AgentRunError {
        AgentRunError::runtime_message(error.to_string())
    }

    pub(super) async fn attach_workflow(
        lane: &crate::Lane,
        agent: &Agent,
        locator: OperationLocator,
    ) -> Result<WorkflowSession, AgentRunError> {
        let now = NativeIds::now()?;
        let (model, profile, catalog) = workflow_ports(agent)?;
        WorkflowSession::trusted(
            lane.session().journal_store(),
            locator,
            ExternalClock::new(now),
        )
        .await
        .map_err(|error| workflow_error(&error))
        .and_then(|session| {
            agent.validate_restored_mask(session.last_state().active_capabilities())?;
            let providers: Arc<[Arc<dyn ContextProvider>]> = agent
                .resolved
                .run_plan()
                .context_providers()
                .iter()
                .map(|component| Arc::clone(component.handle()))
                .collect::<Vec<_>>()
                .into();
            Ok(session
                .with_ready_ports(model, profile, catalog)
                .with_capability_owners(agent.capability_index().as_arc_owners())
                .with_middleware_chain(Arc::clone(agent.resolved.run_plan().middleware_chain()))
                .with_context_providers(providers)
                .with_approval_grant(
                    agent
                        .resolved
                        .spec()
                        .map(|spec| spec.policy.approval_grant)
                        .unwrap_or_default(),
                ))
        })
    }

    #[cfg(test)]
    pub(super) fn workflow_owner_is_live(lane: &crate::Lane) -> bool {
        let Ok(guard) = lane.session().live_lanes().lock() else {
            return false;
        };
        guard
            .get(&lane.lane_id())
            .and_then(|live| live.workflow.as_ref())
            .is_some_and(WorkflowSession::owner_is_live)
    }
}

#[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
pub(crate) use native::LaneLive;

#[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
pub(crate) fn live_run(
    lane: &crate::Lane,
) -> Result<Option<AgentRun>, finstack_ai_runtime::session::SessionError> {
    native::live_run(lane)
}

impl crate::Lane {
    /// Start a new root run on this idle lane.
    ///
    /// `request.input` is the user text. The call uses `Agent::start_on_lane`
    /// so `AcceptRun` lands on this lane rather than bootstrapping a session.
    ///
    /// # Errors
    ///
    /// Returns a tenant mismatch, busy-lane, or agent configuration/runtime
    /// failure.
    pub fn run(&self, agent: &Agent, request: AgentRunRequest) -> Result<AgentRun, AgentRunError> {
        if request.security.tenant_scope() != self.session().tenant_scope() {
            return Err(AgentRunError::configuration(
                AGENT_RUN_INVALID_CONFIGURATION,
                "run security tenant does not match the session tenant",
            ));
        }
        #[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
        {
            let generation = native::reserve_run(self)?;
            match agent.start_on_lane(self, request) {
                Ok(run) => {
                    if let Err(error) = native::attach_run(self, generation, run.clone()) {
                        native::abandon(self, generation);
                        Err(error)
                    } else {
                        Ok(run)
                    }
                }
                Err(error) => {
                    native::abandon(self, generation);
                    Err(error)
                }
            }
        }
        #[cfg(not(any(feature = "native-tokio", feature = "wasm-host")))]
        agent.start_on_lane(self, request)
    }

    /// Park the in-process driver without dropping the journal.
    ///
    /// The lane's active or suspended run remains durable. [`Self::resume`]
    /// respawns [`finstack_ai_runtime::run::RunTaskOwner`].
    ///
    /// # Errors
    ///
    /// Returns a recover or lock failure.
    #[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
    pub async fn suspend(&self) -> Result<(), finstack_ai_runtime::session::SessionError> {
        let Some((generation, run)) = native::begin_suspend(self)? else {
            return Ok(());
        };
        if let Some(run) = run
            && let Ok(handle) = run.runtime_handle().await
        {
            handle.shutdown();
        }
        native::finish_suspend(self, generation)
    }

    /// Recover the parked run and respawn [`finstack_ai_runtime::run::RunTaskOwner`].
    ///
    /// Restore uses [`finstack_ai_runtime::workflow::WorkflowSession::trusted`] plus
    /// [`finstack_ai_runtime::workflow::WorkflowSession::with_ports`],
    /// [`finstack_ai_runtime::workflow::WorkflowSession::with_middleware_chain`],
    /// [`finstack_ai_runtime::workflow::WorkflowSession::with_context_providers`],
    /// and the local-workflow restart respawn path.
    ///
    /// # Errors
    ///
    /// Returns an unknown-run, recover, port, or spawn failure.
    #[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
    pub async fn resume(&self, agent: &Agent) -> Result<(), AgentRunError> {
        Box::pin(self.resume_inner(agent)).await
    }

    #[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
    async fn resume_inner(&self, agent: &Agent) -> Result<(), AgentRunError> {
        use finstack_ai_kernel::OperationLocator;

        let (generation, run) = native::begin_resume(self)?;
        let result = Box::pin(async {
            let inspect = self
                .inspect()
                .await
                .map_err(|error| native::session_error(&error))?;
            let run_id = inspect.active_run_id.ok_or_else(|| {
                AgentRunError::configuration(
                    AGENT_RUN_INVALID_CONFIGURATION,
                    "lane has no suspended run to resume",
                )
            })?;
            let locator = OperationLocator::try_new(
                self.session().tenant_scope(),
                self.session().session_id(),
                self.lane_id(),
                run_id,
            )
            .map_err(|error| {
                AgentRunError::configuration(AGENT_RUN_INVALID_CONFIGURATION, error.to_string())
            })?;
            let mut workflow = native::attach_workflow(self, agent, locator).await?;
            workflow
                .respawn_owner()
                .await
                .map_err(|error| native::workflow_error(&error))?;
            if let Some(run) = run.as_ref() {
                run.recover_children().await?;
            }
            native::finish_resume(self, generation, workflow)
        })
        .await;
        if result.is_err() {
            native::rollback_resume(self, generation);
        }
        result
    }
}

#[cfg(all(test, feature = "native-tokio"))]
impl crate::Lane {
    pub(crate) fn workflow_owner_is_live(&self) -> bool {
        native::workflow_owner_is_live(self)
    }
}
