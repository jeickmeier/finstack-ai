//! `AgentRun` child-run prepare/accept and external-completion routing.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use crate::ChildRunPolicy;
use finstack_ai_kernel::{
    AppendBatchTag, BudgetPropagation, BudgetRequest, CancellationPropagation, ChildPlacement,
    ChildRunLocator, ChildRunPrepared, ContentBlock, DeadlinePropagation, Digest, EffectId,
    EffectTag, Metadata, OperationLocator, PrincipalPropagation, RecordTag, RunAccepted,
    RunPropagationPolicy, RunRelation, RunRelationKind, RunTag, TextBlock,
};
use finstack_ai_runtime::{
    AGENT_INVOKE_INVALID_ACCEPTANCE, AgentInvokeError, AgentInvoker, AgentRef,
    AuthorizationContext, ChildCoordinationIds, ChildRunContext, ChildRunCoordinator,
    ChildRunHandle, ChildRunRequest, CommitCoordinator, ExternalClock,
    ExternalEffectCompletionCommand, PortFuture, child_relation_digest,
};
#[cfg(feature = "native-tokio")]
use finstack_ai_runtime::{ExternalRouteOutcome, WorkflowSession};

#[cfg(all(feature = "wasm-host", not(feature = "native-tokio")))]
use finstack_ai_runtime::host_driver as driver;
#[cfg(feature = "native-tokio")]
use finstack_ai_runtime::native_driver as driver;

use super::handle::Agent;
use super::prepare::NativeIds;
use super::run::AgentRun;
use super::types::{AGENT_RUN_INVALID_CONFIGURATION, AgentRunError, AgentRunRequest};

struct RecordingChildInvoker {
    starts: Arc<std::sync::atomic::AtomicUsize>,
}

impl AgentInvoker for RecordingChildInvoker {
    fn start_or_attach(
        &self,
        context: ChildRunContext,
        request: ChildRunRequest,
    ) -> PortFuture<Result<ChildRunHandle, AgentInvokeError>> {
        self.starts.fetch_add(1, Ordering::SeqCst);
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
    /// Commit one [`ChildRunPrepared`] mapping through [`ChildRunCoordinator`].
    ///
    /// Enforces the parent [`crate::ChildRunPolicy`] before allocating a
    /// locator or invoking [`AgentInvoker::start_or_attach`]. Deny and
    /// over-depth Allow reject with [`AGENT_INVOKE_INVALID_ACCEPTANCE`] and
    /// write no child journal records. The invoker used here only returns the
    /// acceptance handle; [`Self::accept_child`] starts the child agent on the
    /// frozen locator.
    ///
    /// Remote placement is out of scope for this surface.
    ///
    /// # Arguments
    ///
    /// * `child` - Child agent composition. The parent journal store is
    ///   authoritative.
    /// * `request` - Bounded child run input.
    /// * `placement` - Isolated session (preferred) or compatible parent lane.
    ///   Remote placement fails closed.
    ///
    /// # Errors
    ///
    /// Returns a configuration or runtime failure when the parent is not yet
    /// accepted, placement is remote, or the durable mapping conflicts.
    #[cfg(feature = "native-tokio")]
    pub async fn prepare_child(
        &self,
        child: &Agent,
        request: AgentRunRequest,
        placement: ChildPlacement,
    ) -> Result<ChildRunPrepared, AgentRunError> {
        if matches!(placement, ChildPlacement::RemoteChildSession) {
            return Err(AgentRunError::configuration(
                AGENT_RUN_INVALID_CONFIGURATION,
                "remote child placement is not routed by AgentRun",
            ));
        }
        request.validate()?;
        let parent_accepted = self.wait_accepted().await?;
        enforce_child_run_policy(
            self.inner.child_runs,
            child_depth(parent_accepted.relation().depth())?,
        )?;
        let parent = self.inner.locator.clone();
        let (locator, session) = self.allocate_child_locator(placement).await?;
        let parent_effect_id = NativeIds::generate::<EffectTag>()?;
        let child_request = child_run_request(child, &request, placement, locator.clone())?;
        let context = child_run_context(&parent, parent_effect_id, &request);
        let ids = ChildCoordinationIds {
            preparation_batch_id: NativeIds::generate::<AppendBatchTag>()?,
            preparation_record_id: NativeIds::generate::<RecordTag>()?,
            reservation_request_record_id: None,
            reservation_settlement: None,
        };
        let mut commit =
            CommitCoordinator::recover(Arc::clone(&self.inner.store), parent.session_id)
                .await
                .map_err(|error| AgentRunError::runtime_message(error.to_string()))?;
        let coordinator = ChildRunCoordinator::new(Arc::new(RecordingChildInvoker {
            starts: Arc::clone(&self.inner.child_invoker_starts),
        }));
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
            return Err(AgentRunError::configuration(
                AGENT_RUN_INVALID_CONFIGURATION,
                "remote child placement is not routed by AgentRun",
            ));
        }
        request.validate()?;
        let parent_accepted = self.wait_accepted().await?;
        let commit = CommitCoordinator::recover(
            Arc::clone(&self.inner.store),
            self.inner.locator.session_id,
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
    /// parent store. Compatible-lane placement shares the parent session and
    /// can block later parent commits.
    ///
    /// # Arguments
    ///
    /// * `child` - Child agent composition.
    /// * `request` - Bounded child run input.
    /// * `placement` - Isolated session (preferred) or compatible parent lane.
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
    ) -> Result<Self, AgentRunError> {
        let prepared = Box::pin(self.prepare_child(child, request.clone(), placement)).await?;
        let accepted = self.accept_child(&prepared, child, request).await?;
        if let Ok(mut children) = self.inner.children.lock() {
            children.push(accepted.clone());
        }
        Ok(accepted)
    }

    /// Route one authenticated external completion through the workflow ingress.
    ///
    /// Patterned on [`WorkflowSession::complete_external`]. The command locator
    /// must match this run.
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
        if command.locator != self.inner.locator {
            return Err(AgentRunError::runtime_message(
                "external completion locator does not match this run",
            ));
        }
        let now = NativeIds::now()?;
        let session = WorkflowSession::trusted(
            Arc::clone(&self.inner.store),
            self.inner.locator.clone(),
            ExternalClock::new(now),
            u64::try_from(now.as_unix_ms()).unwrap_or(1),
        )
        .await
        .map_err(|error| AgentRunError::runtime_message(error.to_string()))?;
        Box::pin(session.complete_external(command, now))
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
        for child in children {
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
        Ok(())
    }

    async fn wait_accepted(&self) -> Result<RunAccepted, AgentRunError> {
        self.runtime_handle().await?;
        loop {
            if let Ok(commit) = CommitCoordinator::recover(
                Arc::clone(&self.inner.store),
                self.inner.locator.session_id,
            )
            .await
                && let Some(accepted) = commit.state().accepted.clone()
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
            ChildPlacement::RemoteChildSession => Err(AgentRunError::configuration(
                AGENT_RUN_INVALID_CONFIGURATION,
                "remote child placement is not routed by AgentRun",
            )),
        }
    }
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
    let request_digest = child_request_digest(
        &agent,
        &input,
        placement,
        &locator,
        request.security.tenant_scope(),
    )?;
    let child_request = ChildRunRequest {
        agent,
        input,
        placement,
        locator,
        requested_deadline: None,
        requested_budget: BudgetRequest::default(),
        delegation_id: None,
        metadata: Metadata::empty(),
        request_digest,
    };
    child_request
        .validate()
        .map_err(|error| AgentRunError::runtime_message(error.to_string()))?;
    Ok(child_request)
}

fn child_request_digest(
    agent: &AgentRef,
    input: &[ContentBlock],
    placement: ChildPlacement,
    locator: &ChildRunLocator,
    tenant_scope: &str,
) -> Result<Digest, AgentRunError> {
    let canonical = serde_json_canonicalizer::to_vec(&(
        &agent.id,
        &agent.bundle,
        &agent.spec_digest,
        input,
        placement,
        locator,
        tenant_scope,
    ))
    .map_err(|error| AgentRunError::runtime_message(error.to_string()))?;
    Digest::domain_separated("child-run-request", 1, &canonical)
        .map_err(|error| AgentRunError::runtime_message(error.to_string()))
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
    RunAccepted::try_new(
        prepared.child.operation.run_id,
        relation,
        request.security.clone(),
        None,
        spec.limits.clone(),
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
