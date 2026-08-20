//! PR-047 minimum `Lane` verbs: `run`, `suspend`, and `resume`.

use super::handle::Agent;
use super::run::AgentRun;
use super::types::{AGENT_RUN_INVALID_CONFIGURATION, AgentRunError, AgentRunRequest};

#[cfg(feature = "native-tokio")]
mod native {
    use std::sync::Arc;

    use finstack_ai_kernel::OperationLocator;
    use finstack_ai_runtime::{
        ContextProvider, ExternalClock, LockedModelContextProfile, Model,
        ModelContextProfileOverride, ResolvedToolCatalog, SessionError, WorkflowSession,
        resolve_model_context_profile,
    };

    use super::{AGENT_RUN_INVALID_CONFIGURATION, Agent, AgentRun, AgentRunError};
    use crate::agent::prepare::NativeIds;

    pub(crate) struct LaneLive {
        pub(super) run: Option<AgentRun>,
        pub(super) workflow: Option<WorkflowSession>,
    }

    pub(super) fn remember_run(lane: &crate::Lane, run: AgentRun) -> Result<(), AgentRunError> {
        lane.session()
            .live_lanes()
            .lock()
            .map_err(|_| AgentRunError::runtime_message("lane driver lock is poisoned"))?
            .insert(
                lane.lane_id(),
                LaneLive {
                    run: Some(run),
                    workflow: None,
                },
            );
        Ok(())
    }

    pub(super) fn take_live(lane: &crate::Lane) -> Result<Option<LaneLive>, SessionError> {
        Ok(lane
            .session()
            .live_lanes()
            .lock()
            .map_err(|_| SessionError::Poisoned)?
            .remove(&lane.lane_id()))
    }

    pub(super) fn put_live(lane: &crate::Lane, live: LaneLive) -> Result<(), AgentRunError> {
        lane.session()
            .live_lanes()
            .lock()
            .map_err(|_| AgentRunError::runtime_message("lane driver lock is poisoned"))?
            .insert(lane.lane_id(), live);
        Ok(())
    }

    type WorkflowPorts = (
        Arc<dyn Model>,
        LockedModelContextProfile,
        Option<Arc<ResolvedToolCatalog>>,
    );

    pub(super) fn workflow_ports(agent: &Agent) -> Result<WorkflowPorts, AgentRunError> {
        let model = Arc::clone(agent.resolved.run_plan().model().handle());
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
        Ok((model, profile, catalog))
    }

    pub(super) fn session_error(error: &SessionError) -> AgentRunError {
        AgentRunError::configuration(error.code(), error.to_string())
    }

    pub(super) fn workflow_error(
        error: &finstack_ai_runtime::WorkflowDriverError,
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
            u64::try_from(now.as_unix_ms()).unwrap_or(1),
        )
        .await
        .map_err(|error| workflow_error(&error))
        .and_then(|session| {
            agent.validate_restored_mask(&session.last_state().active_capabilities)?;
            let providers: Arc<[Arc<dyn ContextProvider>]> = agent
                .resolved
                .run_plan()
                .context_providers()
                .iter()
                .map(|component| Arc::clone(component.handle()))
                .collect::<Vec<_>>()
                .into();
            Ok(session
                .with_ports(model, profile, catalog)
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

#[cfg(feature = "native-tokio")]
pub(crate) use native::LaneLive;

impl crate::Lane {
    /// Start a new root run on this idle lane.
    ///
    /// `request.input` is the user text. The call uses [`Agent::start_on_lane`]
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
        let run = agent.start_on_lane(self, request)?;
        #[cfg(feature = "native-tokio")]
        native::remember_run(self, run.clone())?;
        Ok(run)
    }

    /// Park the in-process driver without dropping the journal.
    ///
    /// The lane's active or suspended run remains durable. [`Self::resume`]
    /// respawns [`finstack_ai_runtime::RunTaskOwner`].
    ///
    /// # Errors
    ///
    /// Returns a recover or lock failure.
    #[cfg(feature = "native-tokio")]
    pub async fn suspend(&self) -> Result<(), finstack_ai_runtime::SessionError> {
        let Some(mut live) = native::take_live(self)? else {
            return Ok(());
        };
        if let Some(workflow) = live.workflow.as_mut() {
            workflow.abort_owner();
        }
        if let Some(run) = live.run.clone()
            && let Ok(handle) = run.runtime_handle().await
        {
            handle.shutdown();
        }
        native::put_live(self, live).map_err(|_| finstack_ai_runtime::SessionError::Poisoned)
    }

    /// Recover the parked run and respawn [`finstack_ai_runtime::RunTaskOwner`].
    ///
    /// Restore uses [`finstack_ai_runtime::WorkflowSession::trusted`] plus
    /// [`finstack_ai_runtime::WorkflowSession::with_ports`],
    /// [`finstack_ai_runtime::WorkflowSession::with_middleware_chain`],
    /// [`finstack_ai_runtime::WorkflowSession::with_context_providers`],
    /// and the local-workflow restart respawn path.
    ///
    /// # Errors
    ///
    /// Returns an unknown-run, recover, port, or spawn failure.
    #[cfg(feature = "native-tokio")]
    pub async fn resume(&self, agent: &Agent) -> Result<(), AgentRunError> {
        Box::pin(self.resume_inner(agent)).await
    }

    #[cfg(feature = "native-tokio")]
    async fn resume_inner(&self, agent: &Agent) -> Result<(), AgentRunError> {
        use finstack_ai_kernel::OperationLocator;

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
        let mut live = native::take_live(self)
            .map_err(|error| native::session_error(&error))?
            .unwrap_or(native::LaneLive {
                run: None,
                workflow: None,
            });
        live.workflow = Some(workflow);
        if let Some(run) = live.run.as_ref() {
            run.recover_children().await?;
        }
        native::put_live(self, live)
    }
}

#[cfg(all(test, feature = "native-tokio"))]
impl crate::Lane {
    pub(crate) fn workflow_owner_is_live(&self) -> bool {
        native::workflow_owner_is_live(self)
    }
}
