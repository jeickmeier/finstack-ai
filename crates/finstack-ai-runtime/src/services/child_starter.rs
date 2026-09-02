//! Host-owned child-run admission and commit-before-invoke service.

use std::sync::Arc;

use finstack_ai_kernel::{
    AppendBatchTag, BudgetRequest, ChildPlacement, ChildRunLocator, ContentBlock, Id, IdTag,
    LaneTag, Metadata, OperationLocator, RecordTag, RemoteRouteRef, RunTag, SessionTag, Timestamp,
};

use crate::child::{
    AgentInvokeError, AgentInvoker, AgentRef, ChildCoordinationIds, ChildRunContext,
    ChildRunCoordinator, ChildRunHandle, ChildRunPolicy, ChildRunRequest,
};
use crate::commit::CommitCoordinator;
use crate::ids::{Clock, OsRandomSource, SystemClock, UuidV7Generator};
use crate::ports::journal::JournalStore;
use crate::ports::tool::ToolCallContext;
use crate::session::{LaneCreateIds, SessionCreateIds, SessionRuntime};

/// Data-only request submitted to the host child-run admission boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildRunStartRequest {
    /// Exact locked child agent identity.
    pub agent: AgentRef,
    /// Child input blocks.
    pub input: Arc<[ContentBlock]>,
    /// Requested child placement.
    pub placement: ChildPlacement,
    /// Exact remote route for remote placement.
    pub remote: Option<RemoteRouteRef>,
    /// Optional deadline no later than the parent deadline.
    pub requested_deadline: Option<Timestamp>,
    /// Optional shared-budget request.
    pub requested_budget: BudgetRequest,
    /// Optional non-secret delegation reference.
    pub delegation_id: Option<Arc<str>>,
    /// Bounded non-authoritative metadata.
    pub metadata: Metadata,
}

/// Host-bound child-run starter over one journal, policy, and leaf invoker.
pub struct ChildRunStarter {
    store: Arc<dyn JournalStore>,
    policy: ChildRunPolicy,
    invoker: Arc<dyn AgentInvoker>,
}

impl ChildRunStarter {
    /// Bind the exact journal, frozen admission policy, and leaf invoker.
    #[must_use]
    pub fn new(
        store: Arc<dyn JournalStore>,
        policy: ChildRunPolicy,
        invoker: Arc<dyn AgentInvoker>,
    ) -> Self {
        Self {
            store,
            policy,
            invoker,
        }
    }

    /// Start or attach a child after policy enforcement and durable preparation.
    ///
    /// Equal retries recover and reuse the committed locator. No child identity
    /// is allocated and no journal or leaf-invoker call is made when policy
    /// denies the requested depth.
    ///
    /// # Errors
    ///
    /// Fails closed on policy, placement, identity, journal, or invoker errors.
    pub async fn start_or_attach(
        &self,
        ctx: &ToolCallContext,
        request: ChildRunStartRequest,
    ) -> Result<ChildRunHandle, AgentInvokeError> {
        validate_start_request(self.policy, ctx, &request)?;

        let mut commit = self.recover_parent(ctx).await?;
        let locator = if let Some(existing) = commit
            .session()
            .child_mapping(ctx.run.locator.run_id, ctx.run.effect_id)
        {
            existing.child.clone()
        } else {
            let allocated = Box::pin(self.allocate_locator(
                &ctx.run.locator,
                request.placement,
                request.remote.clone(),
            ))
            .await?;
            // Compatible-lane allocation advances the parent journal's
            // structural head, so refresh before preparing the child.
            commit = self.recover_parent(ctx).await?;
            commit
                .session()
                .child_mapping(ctx.run.locator.run_id, ctx.run.effect_id)
                .map_or(allocated, |existing| existing.child.clone())
        };

        let prepared = ChildRunRequest::try_new(
            request.agent,
            request.input,
            request.placement,
            locator,
            request.requested_deadline,
            request.requested_budget,
            request.delegation_id,
            request.metadata,
        )?;
        let context = ChildRunContext {
            parent: ctx.run.locator.clone(),
            parent_effect_id: ctx.run.effect_id,
            authorization: ctx.run.authorization.clone(),
        };
        let ids = ChildCoordinationIds {
            preparation_batch_id: generate::<AppendBatchTag>()?,
            preparation_record_id: generate::<RecordTag>()?,
            reservation_request_record_id: None,
            reservation_settlement: None,
        };
        ChildRunCoordinator::new(Arc::clone(&self.invoker))
            .start_or_attach(&mut commit, context, prepared, None, ids, now()?)
            .await
            .map_err(|error| match error {
                crate::child::CompositionError::Agent(error) => error,
                other => unavailable(other.to_string()),
            })
    }

    /// Query the exact accepted child through the bound leaf invoker.
    ///
    /// # Errors
    ///
    /// Returns the bound invoker's stable status error.
    pub async fn status(
        &self,
        locator: &ChildRunLocator,
    ) -> Result<crate::child::ChildRunStatus, AgentInvokeError> {
        self.invoker.status(locator).await
    }

    /// Cancel the exact accepted child through the bound leaf invoker.
    ///
    /// # Errors
    ///
    /// Returns the bound invoker's stable cancellation error.
    pub async fn cancel(&self, locator: &ChildRunLocator) -> Result<(), AgentInvokeError> {
        self.invoker.cancel(locator).await
    }

    async fn recover_parent(
        &self,
        ctx: &ToolCallContext,
    ) -> Result<CommitCoordinator, AgentInvokeError> {
        CommitCoordinator::recover_run(
            Arc::clone(&self.store),
            ctx.run.locator.session_id,
            Some(ctx.run.locator.run_id),
        )
        .await
        .map_err(|error| unavailable(error.to_string()))
    }

    async fn allocate_locator(
        &self,
        parent: &OperationLocator,
        placement: ChildPlacement,
        remote: Option<RemoteRouteRef>,
    ) -> Result<ChildRunLocator, AgentInvokeError> {
        let run_id = generate::<RunTag>()?;
        let lane_id = generate::<LaneTag>()?;
        let session_id = match placement {
            ChildPlacement::CompatibleLaneInParentSession => {
                let session = SessionRuntime::open(
                    Arc::clone(&self.store),
                    parent.session_id,
                    Arc::clone(&parent.tenant_scope),
                )
                .await
                .map_err(|error| unavailable(error.to_string()))?;
                session
                    .create_lane(
                        format!("child-{lane_id}"),
                        None,
                        LaneCreateIds {
                            lane_id,
                            lane_created_record_id: generate::<RecordTag>()?,
                            lane_moved_record_id: None,
                            batch_id: generate::<AppendBatchTag>()?,
                            now: now()?,
                        },
                    )
                    .await
                    .map_err(|error| unavailable(error.to_string()))?;
                parent.session_id
            }
            ChildPlacement::IsolatedChildSession => {
                let session_id = generate::<SessionTag>()?;
                SessionRuntime::create(
                    Arc::clone(&self.store),
                    Arc::clone(&parent.tenant_scope),
                    SessionCreateIds {
                        session_id,
                        main_lane_id: lane_id,
                        session_created_record_id: generate::<RecordTag>()?,
                        lane_created_record_id: generate::<RecordTag>()?,
                        batch_id: generate::<AppendBatchTag>()?,
                        now: now()?,
                    },
                )
                .await
                .map_err(|error| unavailable(error.to_string()))?;
                session_id
            }
            ChildPlacement::RemoteChildSession => generate::<SessionTag>()?,
        };
        let operation =
            OperationLocator::try_new(parent.tenant_scope.as_ref(), session_id, lane_id, run_id)
                .map_err(|error| invalid_request(error.to_string()))?;
        Ok(ChildRunLocator { operation, remote })
    }
}

fn generate<T: IdTag>() -> Result<Id<T>, AgentInvokeError> {
    UuidV7Generator::new(SystemClock, OsRandomSource)
        .generate()
        .map_err(|error| unavailable(error.to_string()))
}

fn now() -> Result<Timestamp, AgentInvokeError> {
    SystemClock
        .now()
        .map_err(|error| unavailable(error.to_string()))
}

fn validate_start_request(
    policy: ChildRunPolicy,
    ctx: &ToolCallContext,
    request: &ChildRunStartRequest,
) -> Result<(), AgentInvokeError> {
    let child_depth = ctx
        .run
        .relation_depth
        .checked_add(1)
        .ok_or_else(|| invalid_request("child run relation depth overflow"))?;
    match policy {
        ChildRunPolicy::Deny => {
            return Err(invalid_request("child run policy denies invocation"));
        }
        ChildRunPolicy::Allow { max_depth } if child_depth > max_depth => {
            return Err(invalid_request("child run depth exceeds policy max_depth"));
        }
        ChildRunPolicy::Allow { .. } => {}
    }
    if request.requested_budget != BudgetRequest::default() {
        return Err(invalid_request(
            "child run starter requires a configured shared-budget ledger",
        ));
    }
    if matches!(request.placement, ChildPlacement::RemoteChildSession) != request.remote.is_some() {
        return Err(invalid_request(
            "remote route must be present exactly for remote child placement",
        ));
    }
    if ctx.run.deadline.is_some_and(|parent| {
        request
            .requested_deadline
            .is_none_or(|child| child > parent)
    }) {
        return Err(invalid_request(
            "child deadline must not outlive the parent deadline",
        ));
    }
    Ok(())
}

fn invalid_request(message: impl Into<Arc<str>>) -> AgentInvokeError {
    AgentInvokeError::InvalidRequest {
        message: message.into(),
    }
}

fn unavailable(message: impl Into<Arc<str>>) -> AgentInvokeError {
    AgentInvokeError::Unavailable {
        message: message.into(),
    }
}
