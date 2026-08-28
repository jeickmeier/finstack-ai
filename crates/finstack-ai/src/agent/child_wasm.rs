//! Portable wasm-host child execution for local placements.

use std::sync::Arc;

use finstack_ai_kernel::{
    AppendBatchTag, ChildPlacement, ChildRunLocator, ChildRunPrepared, EffectTag, OperationLocator,
    RecordTag, RunTag,
};
use finstack_ai_runtime::child::{AgentInvoker, ChildCoordinationIds, ChildRunCoordinator};
use finstack_ai_runtime::commit::CommitCoordinator;

use super::child::{
    RecordingChildInvoker, child_acceptance, child_depth, child_run_context, child_run_request,
    child_session, enforce_child_run_policy,
};
use super::child_route::RemoteChildRouteSpec;
use super::handle::Agent;
use super::prepare::NativeIds;
use super::run::AgentRun;
use super::types::{
    AGENT_RUN_INVALID_CONFIGURATION, AGENT_RUN_UNSUPPORTED_PLAN, AgentRunError, AgentRunRequest,
};

impl AgentRun {
    /// Commit a local child mapping through the canonical coordinator.
    ///
    /// Browser WASM supports isolated sessions and compatible lanes. Native
    /// endpoint-based remote placement remains outside the portable contract.
    ///
    /// # Errors
    ///
    /// Returns a stable configuration or runtime failure when validation,
    /// policy, allocation, or durable preparation fails.
    pub async fn prepare_child(
        &self,
        child: &Agent,
        request: AgentRunRequest,
        placement: ChildPlacement,
        remote: Option<RemoteChildRouteSpec>,
    ) -> Result<ChildRunPrepared, AgentRunError> {
        if remote.is_some() || matches!(placement, ChildPlacement::RemoteChildSession) {
            return Err(AgentRunError::configuration(
                AGENT_RUN_UNSUPPORTED_PLAN,
                "remote child placement is not portable to wasm-host",
            ));
        }
        request.validate()?;
        let parent_accepted = self.wait_accepted().await?;
        if matches!(placement, ChildPlacement::CompatibleLaneInParentSession)
            && self.parent_turn_is_open()?
        {
            return Err(AgentRunError::configuration(
                AGENT_RUN_INVALID_CONFIGURATION,
                "compatible child placement is not allowed while the parent turn is open",
            ));
        }
        enforce_child_run_policy(
            self.inner.child_runs,
            child_depth(parent_accepted.relation().depth())?,
        )?;
        let parent = self.inner.locator.clone();
        let locator = allocate_local_child_locator(self, placement).await?;
        let parent_effect_id = NativeIds::generate::<EffectTag>()?;
        let child_request = child_run_request(child, &request, placement, locator)?;
        let context = child_run_context(&parent, parent_effect_id, &request);
        let ids = ChildCoordinationIds {
            preparation_batch_id: NativeIds::generate::<AppendBatchTag>()?,
            preparation_record_id: NativeIds::generate::<RecordTag>()?,
            reservation_request_record_id: None,
            reservation_settlement: None,
        };
        let mut commit = CommitCoordinator::recover_run(
            Arc::clone(&self.inner.store),
            parent.session_id,
            Some(parent.run_id),
        )
        .await
        .map_err(|error| AgentRunError::runtime_message(error.to_string()))?;
        let invoker: Arc<dyn AgentInvoker> = Arc::new(RecordingChildInvoker);
        ChildRunCoordinator::new(invoker)
            .start_or_attach(
                &mut commit,
                context,
                child_request,
                None,
                ids,
                NativeIds::now()?,
            )
            .await
            .map_err(|error| AgentRunError::runtime_message(error.to_string()))?;
        commit
            .session()
            .child_mapping(parent.run_id, parent_effect_id)
            .cloned()
            .ok_or_else(|| {
                AgentRunError::runtime_message("child preparation did not become durable")
            })
    }

    /// Start a prepared local child on its frozen locator.
    ///
    /// # Errors
    ///
    /// Returns a stable failure when the mapping is absent, conflicts, or the
    /// child cannot start.
    pub async fn accept_child(
        &self,
        prepared: &ChildRunPrepared,
        child: &Agent,
        request: AgentRunRequest,
    ) -> Result<Self, AgentRunError> {
        if matches!(prepared.placement, ChildPlacement::RemoteChildSession) {
            return Err(AgentRunError::configuration(
                AGENT_RUN_UNSUPPORTED_PLAN,
                "remote child placement is not portable to wasm-host",
            ));
        }
        request.validate()?;
        let parent_accepted = self.wait_accepted().await?;
        let commit = CommitCoordinator::recover_run(
            Arc::clone(&self.inner.store),
            self.inner.locator.session_id,
            Some(self.inner.locator.run_id),
        )
        .await
        .map_err(|error| AgentRunError::runtime_message(error.to_string()))?;
        let mapped = commit
            .session()
            .child_mapping(prepared.parent_run_id, prepared.parent_effect_id)
            .ok_or_else(|| {
                AgentRunError::configuration(
                    AGENT_RUN_INVALID_CONFIGURATION,
                    "child preparation is not committed on the parent journal",
                )
            })?;
        if mapped != prepared {
            return Err(AgentRunError::configuration(
                AGENT_RUN_INVALID_CONFIGURATION,
                "child preparation does not match the committed mapping",
            ));
        }
        let accepted = child_acceptance(child, &request, prepared, &parent_accepted)?;
        let session = child_session(self, prepared).await?;
        child.start_prepared(request, prepared.child.operation.clone(), accepted, session)
    }

    /// Prepare then accept one portable local child.
    ///
    /// # Errors
    ///
    /// Returns the preparation or acceptance failure.
    pub async fn start_child(
        &self,
        child: &Agent,
        request: AgentRunRequest,
        placement: ChildPlacement,
        remote: Option<RemoteChildRouteSpec>,
    ) -> Result<Self, AgentRunError> {
        let prepared =
            Box::pin(self.prepare_child(child, request.clone(), placement, remote)).await?;
        let accepted = self.accept_child(&prepared, child, request).await?;
        if let Ok(mut children) = self.inner.children.lock() {
            children.push(accepted.clone());
        }
        Ok(accepted)
    }

    /// Cancel a previously prepared portable child.
    ///
    /// # Errors
    ///
    /// Returns a stable failure when the locator is remote or cancellation
    /// cannot be committed safely.
    pub async fn cancel_child_locator(
        &self,
        locator: &ChildRunLocator,
    ) -> Result<(), AgentRunError> {
        if locator.remote.is_some() {
            return Err(AgentRunError::configuration(
                AGENT_RUN_UNSUPPORTED_PLAN,
                "remote child cancellation is not portable to wasm-host",
            ));
        }
        let children = self
            .inner
            .children
            .lock()
            .map_err(|_| AgentRunError::runtime_message("run child lock is poisoned"))?
            .clone();
        if let Some(child) = children
            .into_iter()
            .find(|child| child.inner.locator.run_id == locator.operation.run_id)
        {
            return child.cancel().await;
        }
        if locator.operation.session_id == self.inner.locator.session_id
            && self.parent_turn_is_open()?
        {
            return Err(AgentRunError::configuration(
                AGENT_RUN_INVALID_CONFIGURATION,
                "compatible child cancel cannot open a second parent coordinator",
            ));
        }
        let session = finstack_ai_runtime::session::SessionRuntime::open(
            Arc::clone(&self.inner.store),
            locator.operation.session_id,
            Arc::clone(&self.inner.locator.tenant_scope),
        )
        .await
        .map_err(|error| AgentRunError::session(&error))?;
        session
            .cancel_run(
                locator.operation.run_id,
                self.inner.cancellation_initiator.clone(),
                &mut || {
                    NativeIds::cancellation_environment().map_err(|error| {
                        finstack_ai_runtime::session::SessionError::Commit {
                            code: error.owned_code(),
                        }
                    })
                },
            )
            .await
            .map_err(|error| AgentRunError::session(&error))
    }
}

async fn allocate_local_child_locator(
    parent: &AgentRun,
    placement: ChildPlacement,
) -> Result<ChildRunLocator, AgentRunError> {
    let run_id = NativeIds::generate::<RunTag>()?;
    let (session, lane) = match placement {
        ChildPlacement::CompatibleLaneInParentSession => {
            let session = parent.session();
            let lane = session
                .create_lane(format!("child-{run_id}"), None)
                .await
                .map_err(|error| AgentRunError::runtime_message(error.to_string()))?;
            (session, lane)
        }
        ChildPlacement::IsolatedChildSession => {
            let session = crate::Session::create(
                Arc::clone(&parent.inner.store),
                Arc::clone(&parent.inner.locator.tenant_scope),
            )
            .await
            .map_err(|error| AgentRunError::runtime_message(error.to_string()))?;
            let lane = session
                .lane("main")
                .await
                .map_err(|error| AgentRunError::runtime_message(error.to_string()))?;
            (session, lane)
        }
        ChildPlacement::RemoteChildSession => {
            return Err(AgentRunError::configuration(
                AGENT_RUN_UNSUPPORTED_PLAN,
                "remote child placement is not portable to wasm-host",
            ));
        }
    };
    Ok(ChildRunLocator {
        operation: OperationLocator::try_new(
            parent.inner.locator.tenant_scope.as_ref(),
            session.session_id(),
            lane.lane_id(),
            run_id,
        )
        .map_err(|error| {
            AgentRunError::configuration(AGENT_RUN_INVALID_CONFIGURATION, error.to_string())
        })?,
        remote: None,
    })
}
