//! `AgentRun` child-run prepare/accept and external-completion routing.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, OnceLock};

use crate::ChildRunPolicy;
use finstack_ai_kernel::ExternalEffectCompletionCommand;
use finstack_ai_kernel::{
    AppendBatchTag, BudgetPropagation, BudgetRequest, CancellationPropagation, ChildPlacement,
    ChildRunLocator, ChildRunPrepared, ContentBlock, DeadlinePropagation, Digest, EffectId,
    EffectTag, Metadata, OperationLocator, PrincipalPropagation, RecordTag, RunAccepted,
    RunPropagationPolicy, RunRelation, RunRelationKind, RunTag, TextBlock, Timestamp,
};
use finstack_ai_runtime::child::{
    AGENT_INVOKE_INVALID_ACCEPTANCE, AgentInvokeError, AgentInvoker, AgentRef,
    ChildCoordinationIds, ChildRunContext, ChildRunCoordinator, ChildRunHandle, ChildRunRequest,
    child_relation_digest,
};
use finstack_ai_runtime::commit::CommitCoordinator;
#[cfg(feature = "native-tokio")]
use finstack_ai_runtime::ingress::{ExternalCompletionRouter, ExternalRouteOutcome};
use finstack_ai_runtime::ports::PortFuture;
use finstack_ai_runtime::ports::model::AuthorizationContext;

#[cfg(all(feature = "wasm-host", not(feature = "native-tokio")))]
use finstack_ai_runtime::host_driver as driver;
#[cfg(feature = "native-tokio")]
use finstack_ai_runtime::native_driver as driver;

use super::child_route::RemoteChildRouteSpec;
use super::handle::Agent;
use super::prepare::NativeIds;
use super::run::{AgentRun, AgentRunInner, CancellationState, EventStreamState};
use super::types::{AGENT_RUN_INVALID_CONFIGURATION, AgentRunError, AgentRunRequest};

struct RecordingChildInvoker;

impl AgentInvoker for RecordingChildInvoker {
    fn start_or_attach(
        &self,
        context: ChildRunContext,
        request: ChildRunRequest,
    ) -> PortFuture<Result<ChildRunHandle, AgentInvokeError>> {
        let relation_digest = match child_relation_digest(&context, &request) {
            Ok(digest) => digest,
            Err(error) => {
                return Box::pin(async move {
                    Err(AgentInvokeError::InvalidRequest {
                        message: Arc::from(error.to_string()),
                    })
                });
            }
        };
        let locator = request.locator;
        Box::pin(async move {
            Ok(ChildRunHandle {
                locator,
                relation_digest,
            })
        })
    }
}

impl AgentRun {
    /// Start or attach a child run for a parent effect via [`ChildRunCoordinator`].
    ///
    /// This is the effect-binding facade used by deferred child settlement.
    /// It is named `start_or_attach_child` because [`Self::start_child`] is
    /// the composition API (`&Agent`, [`AgentRunRequest`], [`ChildPlacement`],
    /// optional remote route → [`AgentRun`]). Those signatures cannot share a
    /// name.
    /// Authorization is copied from the recovered accepted run, matching
    /// runtime dispatch security context.
    ///
    /// Equal retries attach to the committed mapping. A conflicting
    /// `request_digest` fails closed.
    ///
    /// # Arguments
    ///
    /// * `invoker` - Idempotent child start-or-attach service.
    /// * `parent_effect_id` - Parent effect that authorizes this child.
    /// * `request` - Frozen child request including locator and digest.
    ///
    /// # Errors
    ///
    /// Returns a runtime failure when the parent is not accepted, the durable
    /// mapping conflicts, or the invoker rejects the child.
    #[cfg(feature = "native-tokio")]
    pub async fn start_or_attach_child(
        &self,
        invoker: Arc<dyn AgentInvoker>,
        parent_effect_id: EffectId,
        request: ChildRunRequest,
    ) -> Result<ChildRunHandle, AgentRunError> {
        let mut commit = CommitCoordinator::recover_run(
            Arc::clone(self.journal_store()),
            self.locator().session_id,
            Some(self.locator().run_id),
        )
        .await
        .map_err(|error| AgentRunError::runtime_message(error.to_string()))?;
        let accepted = commit
            .state()
            .accepted()
            .ok_or_else(|| AgentRunError::runtime_message("parent run is not accepted"))?;
        if accepted.run_id() != self.locator().run_id {
            return Err(AgentRunError::runtime_message(
                "recovered acceptance does not match this run",
            ));
        }
        let security = accepted.security();
        let context = ChildRunContext {
            parent: self.locator().clone(),
            parent_effect_id,
            authorization: AuthorizationContext {
                principal: security.principal().clone(),
                authentication_method: Arc::from(security.authentication_method()),
                assurance_level: Arc::from(security.assurance_level()),
                roles: Arc::from([]),
                permitted_scopes: Arc::from([Arc::from(security.tenant_scope())]),
                safe_claims: Metadata::empty(),
                policy_version: Arc::from(security.authorization_policy_version()),
                decision_id: Arc::from(security.authorization_decision_id()),
            },
        };
        let ids = ChildCoordinationIds {
            preparation_batch_id: NativeIds::generate::<AppendBatchTag>()?,
            preparation_record_id: NativeIds::generate::<RecordTag>()?,
            reservation_request_record_id: None,
            reservation_settlement: None,
        };
        ChildRunCoordinator::new(invoker)
            .start_or_attach(&mut commit, context, request, None, ids, NativeIds::now()?)
            .await
            .map_err(|error| AgentRunError::runtime_message(error.to_string()))
    }

    /// Commit one [`ChildRunPrepared`] mapping through [`ChildRunCoordinator`].
    ///
    /// Enforces the parent [`crate::ChildRunPolicy`] before allocating a
    /// locator or invoking [`AgentInvoker::start_or_attach`]. Deny and
    /// over-depth Allow reject with [`AGENT_INVOKE_INVALID_ACCEPTANCE`] and
    /// write no child journal records. The invoker used here only returns the
    /// acceptance handle; [`Self::accept_child`] starts the child agent on the
    /// frozen locator.
    ///
    /// Remote placement requires an explicit `remote` route. Missing routes
    /// stay fail-closed.
    ///
    /// # Arguments
    ///
    /// * `child` - Child agent composition. The parent journal store is
    ///   authoritative.
    /// * `request` - Bounded child run input.
    /// * `placement` - Isolated session (preferred) or compatible parent lane.
    ///   Remote placement fails closed without `remote`.
    /// * `remote` - Optional remote route for [`ChildPlacement::RemoteChildSession`].
    ///
    /// # Errors
    ///
    /// Returns a configuration or runtime failure when the parent is not yet
    /// accepted, placement is remote without a route, the parent turn is still
    /// open for compatible-lane placement, or the durable mapping conflicts.
    #[cfg(feature = "native-tokio")]
    pub async fn prepare_child(
        &self,
        child: &Agent,
        request: AgentRunRequest,
        placement: ChildPlacement,
        remote: Option<RemoteChildRouteSpec>,
    ) -> Result<ChildRunPrepared, AgentRunError> {
        if matches!(placement, ChildPlacement::RemoteChildSession) && remote.is_none() {
            return Err(AgentRunError::configuration(
                AGENT_RUN_INVALID_CONFIGURATION,
                "remote child placement requires an explicit route",
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
        let remote_invoker = remote.map(build_remote_invoker).transpose()?;
        let (locator, session) = self
            .allocate_child_locator(placement, remote_invoker.as_ref())
            .await?;
        let parent_effect_id = NativeIds::generate::<EffectTag>()?;
        let child_request = child_run_request(child, &request, placement, locator.clone())?;
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
        let invoker: Arc<dyn AgentInvoker> = match remote_invoker {
            Some(invoker) => {
                let invoker = Arc::new(invoker);
                let mut slot = self.inner.remote_invoker.lock().map_err(|_| {
                    AgentRunError::runtime_message("run remote invoker lock is poisoned")
                })?;
                *slot = Some(Arc::clone(&invoker) as Arc<dyn AgentInvoker>);
                invoker
            }
            None => Arc::new(RecordingChildInvoker),
        };
        let coordinator = ChildRunCoordinator::new(invoker);
        coordinator
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
        let prepared = commit
            .session()
            .child_mapping(parent.run_id, parent_effect_id)
            .cloned()
            .ok_or_else(|| {
                AgentRunError::runtime_message("child preparation did not become durable")
            })?;
        debug_assert_eq!(parent_accepted.run_id(), parent.run_id);
        let _ = session;
        Ok(prepared)
    }

    /// Accept a prepared child by starting `child` on the frozen locator.
    ///
    /// # Arguments
    ///
    /// * `prepared` - Durable `ChildRunPrepared` mapping from
    ///   [`Self::prepare_child`].
    /// * `child` - Child agent started on the frozen locator.
    /// * `request` - Same bounded request used for prepare.
    ///
    /// # Errors
    ///
    /// Returns a configuration or runtime failure when the mapping is missing,
    /// the child relation is invalid, or the child agent cannot start.
    #[cfg(feature = "native-tokio")]
    pub async fn accept_child(
        &self,
        prepared: &ChildRunPrepared,
        child: &Agent,
        request: AgentRunRequest,
    ) -> Result<Self, AgentRunError> {
        if matches!(prepared.placement, ChildPlacement::RemoteChildSession) {
            return accept_remote_child(self, prepared);
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

    /// Prepare then accept one child on the selected placement.
    ///
    /// Prefer isolated placement: the child journal is a new session on the
    /// parent store. Compatible-lane placement is rejected while the parent
    /// turn is open so this path never opens a second coordinator on the
    /// parent session.
    ///
    /// # Arguments
    ///
    /// * `child` - Child agent composition.
    /// * `request` - Bounded child run input.
    /// * `placement` - Isolated session (preferred) or compatible parent lane.
    ///   Remote placement fails closed without `remote`.
    /// * `remote` - Optional remote route for [`ChildPlacement::RemoteChildSession`].
    ///
    /// # Errors
    ///
    /// Returns the prepare or accept failure.
    #[cfg(feature = "native-tokio")]
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

    /// Route one authenticated external completion through ingress.
    ///
    /// The command locator must match this run. Submission time is the
    /// current native clock.
    ///
    /// # Arguments
    ///
    /// * `command` - Authenticated completion for a deferred effect on this
    ///   run.
    ///
    /// # Errors
    ///
    /// Returns a runtime failure when the locator does not match or ingress
    /// rejects the command.
    #[cfg(feature = "native-tokio")]
    pub async fn complete_external(
        &self,
        command: ExternalEffectCompletionCommand,
    ) -> Result<ExternalRouteOutcome, AgentRunError> {
        Box::pin(self.complete_external_at(command, NativeIds::now()?)).await
    }

    #[cfg(feature = "native-tokio")]
    pub(crate) async fn complete_external_at(
        &self,
        command: ExternalEffectCompletionCommand,
        submitted_at: Timestamp,
    ) -> Result<ExternalRouteOutcome, AgentRunError> {
        if command.locator != *self.locator() {
            return Err(AgentRunError::runtime_message(
                "external completion locator does not match this run",
            ));
        }
        let router = ExternalCompletionRouter::trusted(Arc::clone(self.journal_store()))
            .await
            .map_err(|error| AgentRunError::runtime_message(error.to_string()))?;
        Box::pin(router.route(command, submitted_at))
            .await
            .map_err(|error| AgentRunError::runtime_message(error.to_string()))
    }

    #[cfg(feature = "native-tokio")]
    pub(super) async fn fan_out_cancellation(&self) -> Result<(), AgentRunError> {
        let children = self
            .inner
            .children
            .lock()
            .map_err(|_| AgentRunError::runtime_message("run child lock is poisoned"))?
            .clone();
        let mut live = BTreeSet::new();
        for child in children {
            live.insert(child.inner.locator.run_id);
            let should_start = {
                let mut cancellation = child.inner.cancellation.lock().map_err(|_| {
                    AgentRunError::runtime_message("run cancellation lock is poisoned")
                })?;
                if cancellation.result.is_some() || cancellation.started {
                    false
                } else {
                    cancellation.started = true;
                    true
                }
            };
            if !should_start {
                continue;
            }
            let result = child.submit_cancellation().await;
            if let Ok(mut cancellation) = child.inner.cancellation.lock() {
                cancellation.result = Some(result.clone());
            }
            child.inner.cancellation_ready.notify_waiters();
            result?;
        }
        self.fan_out_journaled_children(&live).await
    }

    /// Rebuild isolated children from journaled mappings when live handles
    /// are gone. Compatible mappings stay on the recovered parent journal
    /// and are not accepted again. Remote stays a no-op.
    #[cfg(feature = "native-tokio")]
    pub(crate) async fn recover_children(&self) -> Result<(), AgentRunError> {
        let live = self.live_child_run_ids()?;
        for prepared in self.journaled_child_mappings().await? {
            if live.contains(&prepared.child.operation.run_id) {
                continue;
            }
            match prepared.placement {
                ChildPlacement::IsolatedChildSession => {
                    let _ = finstack_ai_runtime::session::SessionRuntime::open(
                        Arc::clone(&self.inner.store),
                        prepared.child.operation.session_id,
                        Arc::clone(&prepared.child.operation.tenant_scope),
                    )
                    .await
                    .map_err(|error| AgentRunError::runtime_message(error.to_string()))?;
                }
                ChildPlacement::CompatibleLaneInParentSession
                | ChildPlacement::RemoteChildSession => {}
            }
        }
        Ok(())
    }

    fn parent_turn_is_open(&self) -> Result<bool, AgentRunError> {
        let result = self
            .inner
            .result
            .lock()
            .map_err(|_| AgentRunError::runtime_message("run result lock is poisoned"))?;
        Ok(result.is_none())
    }

    fn live_child_run_ids(&self) -> Result<BTreeSet<finstack_ai_kernel::RunId>, AgentRunError> {
        let children = self
            .inner
            .children
            .lock()
            .map_err(|_| AgentRunError::runtime_message("run child lock is poisoned"))?;
        Ok(children
            .iter()
            .map(|child| child.inner.locator.run_id)
            .collect())
    }

    #[cfg(feature = "native-tokio")]
    async fn journaled_child_mappings(&self) -> Result<Vec<ChildRunPrepared>, AgentRunError> {
        let parent_run = self.inner.locator.run_id;
        let Ok(commit) = CommitCoordinator::recover_run(
            Arc::clone(&self.inner.store),
            self.inner.locator.session_id,
            Some(parent_run),
        )
        .await
        else {
            return Ok(Vec::new());
        };
        Ok(commit
            .session()
            .child_mappings()
            .iter()
            .filter(|((run_id, _), _)| *run_id == parent_run)
            .map(|(_, prepared)| prepared.clone())
            .collect())
    }

    #[cfg(feature = "native-tokio")]
    async fn fan_out_journaled_children(
        &self,
        live: &BTreeSet<finstack_ai_kernel::RunId>,
    ) -> Result<(), AgentRunError> {
        for prepared in self.journaled_child_mappings().await? {
            if live.contains(&prepared.child.operation.run_id) {
                continue;
            }
            match prepared.placement {
                ChildPlacement::RemoteChildSession => {
                    self.cancel_remote_child(&prepared.child).await?;
                }
                ChildPlacement::CompatibleLaneInParentSession => {
                    if self.parent_turn_is_open()? {
                        continue;
                    }
                    self.cancel_journaled_run(
                        prepared.child.operation.session_id,
                        prepared.child.operation.run_id,
                    )
                    .await?;
                }
                ChildPlacement::IsolatedChildSession => {
                    self.cancel_journaled_run(
                        prepared.child.operation.session_id,
                        prepared.child.operation.run_id,
                    )
                    .await?;
                }
            }
        }
        Ok(())
    }

    /// Cancel one previously prepared child by its frozen locator.
    ///
    /// Live child handles are the fast path. Isolated children without a
    /// live handle are cancelled through the child session journal.
    /// Compatible children are not cancelled through a second parent
    /// coordinator while the parent turn is open.
    ///
    /// # Errors
    ///
    /// Returns a configuration or runtime failure when cancellation cannot
    /// be submitted safely.
    #[cfg(feature = "native-tokio")]
    pub async fn cancel_child_locator(
        &self,
        locator: &ChildRunLocator,
    ) -> Result<(), AgentRunError> {
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
        if locator.remote.is_some() {
            return self.cancel_remote_child(locator).await;
        }
        if locator.operation.session_id == self.inner.locator.session_id
            && self.parent_turn_is_open()?
        {
            return Err(AgentRunError::configuration(
                AGENT_RUN_INVALID_CONFIGURATION,
                "compatible child cancel cannot open a second parent coordinator",
            ));
        }
        self.cancel_journaled_run(locator.operation.session_id, locator.operation.run_id)
            .await
    }

    async fn cancel_remote_child(&self, locator: &ChildRunLocator) -> Result<(), AgentRunError> {
        let invoker = self
            .inner
            .remote_invoker
            .lock()
            .map_err(|_| AgentRunError::runtime_message("run remote invoker lock is poisoned"))?
            .clone();
        let Some(invoker) = invoker else {
            return Err(AgentRunError::configuration(
                AGENT_RUN_INVALID_CONFIGURATION,
                "remote child cancel has no installed invoker",
            ));
        };
        invoker
            .cancel(locator)
            .await
            .map_err(|error| AgentRunError::runtime_message(error.to_string()))
    }

    #[cfg(feature = "native-tokio")]
    async fn cancel_journaled_run(
        &self,
        session_id: finstack_ai_kernel::SessionId,
        run_id: finstack_ai_kernel::RunId,
    ) -> Result<(), AgentRunError> {
        let session = finstack_ai_runtime::session::SessionRuntime::open(
            Arc::clone(&self.inner.store),
            session_id,
            Arc::clone(&self.inner.locator.tenant_scope),
        )
        .await
        .map_err(|error| AgentRunError::session(&error))?;
        session
            .cancel_run(
                run_id,
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

    async fn wait_accepted(&self) -> Result<RunAccepted, AgentRunError> {
        self.runtime_handle().await?;
        loop {
            if let Ok(commit) = CommitCoordinator::recover_run(
                Arc::clone(&self.inner.store),
                self.inner.locator.session_id,
                Some(self.inner.locator.run_id),
            )
            .await
                && let Some(accepted) = commit.state().accepted().cloned()
                && accepted.run_id() == self.inner.locator.run_id
            {
                return Ok(accepted);
            }
            driver::yield_now().await;
        }
    }

    async fn allocate_child_locator(
        &self,
        placement: ChildPlacement,
        remote: Option<&finstack_ai_remote_child::RemoteChildInvoker>,
    ) -> Result<(ChildRunLocator, crate::Session), AgentRunError> {
        let run_id = NativeIds::generate::<RunTag>()?;
        match placement {
            ChildPlacement::CompatibleLaneInParentSession => {
                let session = self.session();
                let lane = session
                    .create_lane(format!("child-{run_id}"), None)
                    .await
                    .map_err(|error| AgentRunError::runtime_message(error.to_string()))?;
                let locator = ChildRunLocator {
                    operation: OperationLocator::try_new(
                        self.inner.locator.tenant_scope.as_ref(),
                        self.inner.locator.session_id,
                        lane.lane_id(),
                        run_id,
                    )
                    .map_err(|error| {
                        AgentRunError::configuration(
                            AGENT_RUN_INVALID_CONFIGURATION,
                            error.to_string(),
                        )
                    })?,
                    remote: None,
                };
                Ok((locator, session))
            }
            ChildPlacement::IsolatedChildSession => {
                let session = crate::Session::create(
                    Arc::clone(&self.inner.store),
                    Arc::clone(&self.inner.locator.tenant_scope),
                )
                .await
                .map_err(|error| AgentRunError::runtime_message(error.to_string()))?;
                let lane = session
                    .lane("main")
                    .await
                    .map_err(|error| AgentRunError::runtime_message(error.to_string()))?;
                let locator = ChildRunLocator {
                    operation: OperationLocator::try_new(
                        self.inner.locator.tenant_scope.as_ref(),
                        session.session_id(),
                        lane.lane_id(),
                        run_id,
                    )
                    .map_err(|error| {
                        AgentRunError::configuration(
                            AGENT_RUN_INVALID_CONFIGURATION,
                            error.to_string(),
                        )
                    })?,
                    remote: None,
                };
                Ok((locator, session))
            }
            ChildPlacement::RemoteChildSession => {
                let invoker = remote.ok_or_else(|| {
                    AgentRunError::configuration(
                        AGENT_RUN_INVALID_CONFIGURATION,
                        "remote child placement requires an explicit route",
                    )
                })?;
                let session = crate::Session::create(
                    Arc::clone(&self.inner.store),
                    Arc::clone(&self.inner.locator.tenant_scope),
                )
                .await
                .map_err(|error| AgentRunError::runtime_message(error.to_string()))?;
                let lane = session
                    .lane("main")
                    .await
                    .map_err(|error| AgentRunError::runtime_message(error.to_string()))?;
                let locator = ChildRunLocator {
                    operation: OperationLocator::try_new(
                        self.inner.locator.tenant_scope.as_ref(),
                        session.session_id(),
                        lane.lane_id(),
                        run_id,
                    )
                    .map_err(|error| {
                        AgentRunError::configuration(
                            AGENT_RUN_INVALID_CONFIGURATION,
                            error.to_string(),
                        )
                    })?,
                    remote: Some(invoker.route_ref().map_err(|error| {
                        AgentRunError::configuration(
                            AGENT_RUN_INVALID_CONFIGURATION,
                            error.to_string(),
                        )
                    })?),
                };
                Ok((locator, session))
            }
        }
    }
}

fn build_remote_invoker(
    spec: RemoteChildRouteSpec,
) -> Result<finstack_ai_remote_child::RemoteChildInvoker, AgentRunError> {
    finstack_ai_remote_child::RemoteChildInvoker::try_new(
        finstack_ai_remote_child::RemoteChildRoute {
            endpoint: spec.endpoint,
            service: spec.service,
            route: spec.route,
            token: spec.token,
        },
    )
    .map_err(|error| {
        AgentRunError::configuration(AGENT_RUN_INVALID_CONFIGURATION, error.to_string())
    })
}

fn accept_remote_child(
    parent: &AgentRun,
    prepared: &ChildRunPrepared,
) -> Result<AgentRun, AgentRunError> {
    let invoker = parent
        .inner
        .remote_invoker
        .lock()
        .map_err(|_| AgentRunError::runtime_message("run remote invoker lock is poisoned"))?
        .clone()
        .ok_or_else(|| {
            AgentRunError::configuration(
                AGENT_RUN_INVALID_CONFIGURATION,
                "remote child accept has no installed invoker",
            )
        })?;
    Ok(AgentRun {
        inner: Arc::new(AgentRunInner {
            locator: prepared.child.operation.clone(),
            store: Arc::clone(&parent.inner.store),
            child_runs: parent.inner.child_runs,
            cancellation_initiator: parent.inner.cancellation_initiator.clone(),
            handle: Mutex::new(None),
            handle_ready: driver::Signal::new(),
            result: Mutex::new(None),
            result_ready: driver::Signal::new(),
            events: Mutex::new(EventStreamState::Waiting),
            events_fault: OnceLock::new(),
            cancellation: Mutex::new(CancellationState::default()),
            cancellation_ready: driver::Signal::new(),
            children: Mutex::new(Vec::new()),
            remote_invoker: Mutex::new(Some(invoker)),
            remote_child: Some(prepared.child.clone()),
        }),
    })
}

fn child_depth(parent_depth: u16) -> Result<u16, AgentRunError> {
    parent_depth
        .checked_add(1)
        .ok_or_else(|| AgentRunError::runtime_message("child relation depth overflow"))
}

fn enforce_child_run_policy(policy: ChildRunPolicy, child_depth: u16) -> Result<(), AgentRunError> {
    match policy {
        ChildRunPolicy::Deny => Err(AgentRunError::configuration(
            AGENT_INVOKE_INVALID_ACCEPTANCE,
            "child run policy denies child invocation",
        )),
        ChildRunPolicy::Allow { max_depth } if child_depth > max_depth => {
            Err(AgentRunError::configuration(
                AGENT_INVOKE_INVALID_ACCEPTANCE,
                "child run depth exceeds policy max_depth",
            ))
        }
        ChildRunPolicy::Allow { .. } => Ok(()),
    }
}

fn child_run_request(
    child: &Agent,
    request: &AgentRunRequest,
    placement: ChildPlacement,
    locator: ChildRunLocator,
) -> Result<ChildRunRequest, AgentRunError> {
    let spec = child.resolved.spec().ok_or_else(|| {
        AgentRunError::configuration(
            AGENT_RUN_INVALID_CONFIGURATION,
            "child Agent requires a bundle-resolved specification",
        )
    })?;
    let lock = child.resolved.lock().ok_or_else(|| {
        AgentRunError::configuration(
            AGENT_RUN_INVALID_CONFIGURATION,
            "child Agent requires a bundle-resolved lock",
        )
    })?;
    let agent = AgentRef {
        id: spec.id.clone(),
        bundle: lock.bundle.as_ref().map(|bundle| bundle.id.clone()),
        spec_digest: spec
            .fingerprint()
            .map_err(|error| AgentRunError::runtime_message(error.to_string()))?,
    };
    let input = Arc::from([ContentBlock::Text(
        TextBlock::try_new(request.input.as_ref()).map_err(|error| {
            AgentRunError::configuration(AGENT_RUN_INVALID_CONFIGURATION, error.to_string())
        })?,
    )]);
    let mut child_request = ChildRunRequest {
        agent,
        input,
        placement,
        locator,
        requested_deadline: None,
        requested_budget: BudgetRequest::default(),
        delegation_id: None,
        metadata: Metadata::empty(),
        request_digest: Digest::raw_json(b"null"),
    };
    child_request.request_digest = child_request
        .canonical_digest()
        .map_err(|error| AgentRunError::runtime_message(error.to_string()))?;
    child_request
        .validate()
        .map_err(|error| AgentRunError::runtime_message(error.to_string()))?;
    Ok(child_request)
}

fn child_run_context(
    parent: &OperationLocator,
    parent_effect_id: EffectId,
    request: &AgentRunRequest,
) -> ChildRunContext {
    let security = &request.security;
    ChildRunContext {
        parent: parent.clone(),
        parent_effect_id,
        authorization: AuthorizationContext {
            principal: security.principal().clone(),
            authentication_method: Arc::from(security.authentication_method()),
            assurance_level: Arc::from(security.assurance_level()),
            roles: Arc::from([]),
            permitted_scopes: Arc::from([]),
            safe_claims: Metadata::empty(),
            policy_version: Arc::from(security.authorization_policy_version()),
            decision_id: Arc::from(security.authorization_decision_id()),
        },
    }
}

fn child_acceptance(
    child: &Agent,
    request: &AgentRunRequest,
    prepared: &ChildRunPrepared,
    parent: &RunAccepted,
) -> Result<RunAccepted, AgentRunError> {
    let spec = child.resolved.spec().ok_or_else(|| {
        AgentRunError::configuration(
            AGENT_RUN_INVALID_CONFIGURATION,
            "child Agent requires a bundle-resolved specification",
        )
    })?;
    let lock = child.resolved.lock().ok_or_else(|| {
        AgentRunError::configuration(
            AGENT_RUN_INVALID_CONFIGURATION,
            "child Agent requires a bundle-resolved lock",
        )
    })?;
    let depth = child_depth(parent.relation().depth())?;
    let relation = RunRelation::try_new(
        parent.relation().root_run_id(),
        Some(parent.run_id()),
        Some(prepared.parent_effect_id),
        RunRelationKind::ChildAgent,
        depth,
        None,
        None::<&str>,
    )
    .map_err(|error| {
        AgentRunError::configuration(AGENT_RUN_INVALID_CONFIGURATION, error.to_string())
    })?;
    let limits = super::prepare::attenuated_run_limits(&spec.limits, request)?;
    let effective_deadline = super::prepare::request_deadline(
        super::prepare::NativeIds::now()?,
        request.timeout,
        parent.effective_deadline(),
    )?;
    RunAccepted::try_new(
        prepared.child.operation.run_id,
        relation,
        request.security.clone(),
        effective_deadline,
        limits,
        RunPropagationPolicy {
            cancellation: CancellationPropagation::Cascade,
            deadline: DeadlinePropagation::MinimumOfParentAndChild,
            budget: BudgetPropagation::SharedScope,
            principal: PrincipalPropagation::Inherit,
        },
        lock.fingerprint()
            .map_err(|error| AgentRunError::runtime_message(error.to_string()))?,
        Some(parent),
    )
    .map_err(|error| {
        AgentRunError::configuration(AGENT_RUN_INVALID_CONFIGURATION, error.to_string())
    })
}

async fn child_session(
    parent: &AgentRun,
    prepared: &ChildRunPrepared,
) -> Result<crate::Session, AgentRunError> {
    match prepared.placement {
        ChildPlacement::CompatibleLaneInParentSession => Ok(parent.session()),
        ChildPlacement::IsolatedChildSession => crate::Session::open(
            Arc::clone(&parent.inner.store),
            prepared.child.operation.session_id,
            Arc::clone(&prepared.child.operation.tenant_scope),
        )
        .await
        .map_err(|error| AgentRunError::runtime_message(error.to_string())),
        ChildPlacement::RemoteChildSession => Err(AgentRunError::configuration(
            AGENT_RUN_INVALID_CONFIGURATION,
            "remote child placement is not routed by AgentRun",
        )),
    }
}
