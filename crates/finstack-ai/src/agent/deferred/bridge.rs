//! Child-run bridge that settles deferred parent tool effects.

use std::future::{Future, poll_fn};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;

use finstack_ai_runtime::ports::PortFuture;

use finstack_ai_kernel::{
    AuthorizationEvidence, ContentBlock, EffectDeferred, ErrorCategory, ErrorDescriptor,
    ExternalEffectCompletion, ExternalEffectCompletionCommand, ExternalEffectOutcome, RawJson,
    TextBlock, ToolResultBlock,
};
use finstack_ai_runtime::child::AgentInvoker;
use finstack_ai_runtime::ingress::ExternalRouteOutcome;
use finstack_ai_runtime::native_driver as driver;
use thiserror::Error;

use super::planner::{ChildPlanContext, ChildRunResolver, DeferredChildPlanner, DeferredPlanError};
use super::sink::{ChildEventContext, ChildEventSink};
use crate::AgentRun;

/// Stable code when a planner rejects a deferred handle.
pub const CHILD_RUN_BRIDGE_PLANNER_REJECTED: &str = "child_run_bridge_planner_rejected";
/// Stable code when a planner cannot decide safely.
pub const CHILD_RUN_BRIDGE_PLANNER_UNAVAILABLE: &str = "child_run_bridge_planner_unavailable";
/// Stable code when child start, resolve, or completion fails.
pub const CHILD_RUN_BRIDGE_FAILED: &str = "child_run_bridge_failed";

/// Binds deferred parent tool effects to child runs.
pub struct ChildRunBridge {
    planners: Vec<Arc<dyn DeferredChildPlanner>>,
    invoker: Arc<dyn AgentInvoker>,
    resolver: Arc<dyn ChildRunResolver>,
    sink: Option<Arc<dyn ChildEventSink>>,
}

/// Result of attempting to settle one deferred parent effect through a child.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildSettleOutcome {
    /// No planner claimed the deferred handle.
    Unclaimed,
    /// The child completed and the parent effect was completed.
    Completed,
    /// The child failed and the parent effect was failed.
    Failed,
}

/// Failure from deferred child-run settlement.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ChildRunBridgeError {
    /// A planner recognized the handle and refused it.
    #[error("deferred child planner rejected the handle: {message}")]
    PlannerRejected {
        /// Non-secret explanation.
        message: String,
    },
    /// A planner cannot decide safely.
    #[error("deferred child planner is unavailable: {message}")]
    PlannerUnavailable {
        /// Non-secret explanation.
        message: String,
    },
    /// Child start, resolve, pump, or completion failed.
    #[error("deferred child run failed: {message}")]
    Failed {
        /// Non-secret explanation.
        message: String,
    },
}

impl ChildRunBridgeError {
    /// Stable machine-readable error code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::PlannerRejected { .. } => CHILD_RUN_BRIDGE_PLANNER_REJECTED,
            Self::PlannerUnavailable { .. } => CHILD_RUN_BRIDGE_PLANNER_UNAVAILABLE,
            Self::Failed { .. } => CHILD_RUN_BRIDGE_FAILED,
        }
    }

    fn failed(message: impl Into<String>) -> Self {
        Self::Failed {
            message: message.into(),
        }
    }
}

impl From<DeferredPlanError> for ChildRunBridgeError {
    fn from(error: DeferredPlanError) -> Self {
        match error {
            DeferredPlanError::Rejected { message } => Self::PlannerRejected { message },
            DeferredPlanError::Unavailable { message } => Self::PlannerUnavailable { message },
        }
    }
}

impl ChildRunBridge {
    /// Construct a bridge over planners, a child invoker, and a child resolver.
    #[must_use]
    pub fn new(
        planners: Vec<Arc<dyn DeferredChildPlanner>>,
        invoker: Arc<dyn AgentInvoker>,
        resolver: Arc<dyn ChildRunResolver>,
    ) -> Self {
        Self {
            planners,
            invoker,
            resolver,
            sink: None,
        }
    }

    /// Install a notification-only child event sink.
    #[must_use]
    pub fn with_event_sink(mut self, sink: Arc<dyn ChildEventSink>) -> Self {
        self.sink = Some(sink);
        self
    }

    /// Claim, start or attach, pump, and complete one deferred parent effect.
    ///
    /// The first planner that returns `Ok(Some(request))` wins. `Ok(None)` is
    /// unowned. Planner errors fail closed.
    ///
    /// # Errors
    ///
    /// Returns a planner, child-start, resolve, or completion failure.
    pub async fn settle(
        &self,
        parent: &AgentRun,
        deferred: &EffectDeferred,
    ) -> Result<ChildSettleOutcome, ChildRunBridgeError> {
        Box::pin(self.settle_inner(parent, deferred)).await
    }

    async fn settle_inner(
        &self,
        parent: &AgentRun,
        deferred: &EffectDeferred,
    ) -> Result<ChildSettleOutcome, ChildRunBridgeError> {
        let context = ChildPlanContext {
            parent: parent.locator().clone(),
            deferred: deferred.clone(),
        };
        let mut claimed = None;
        for planner in &self.planners {
            if let Some(request) = planner.plan(&context).await? {
                claimed = Some(request);
                break;
            }
        }
        let Some(request) = claimed else {
            return Ok(ChildSettleOutcome::Unclaimed);
        };
        let handle = parent
            .start_or_attach_child(Arc::clone(&self.invoker), deferred.effect_id, request)
            .await
            .map_err(|error| ChildRunBridgeError::failed(error.to_string()))?;
        let child = self.resolver.resolve(&handle.locator).await?;
        let events = ChildEventContext {
            parent: parent.locator().clone(),
            child: handle.locator.clone(),
            effect_id: deferred.effect_id,
        };
        self.pump_child(&child, &events).await;
        let (outcome, settle) = match child.result().await {
            Ok(output) => {
                let output = tool_result_output(parent, deferred, &output.text(), false).await?;
                (
                    ExternalEffectOutcome::Completed {
                        output,
                        usage: None,
                        artifacts: Arc::from([]),
                    },
                    ChildSettleOutcome::Completed,
                )
            }
            Err(error) => (
                ExternalEffectOutcome::Failed {
                    error: ErrorDescriptor::new(
                        "child_run_failed",
                        error.to_string(),
                        ErrorCategory::Cancellation,
                        false,
                    )
                    .map_err(|error| ChildRunBridgeError::failed(error.to_string()))?,
                },
                ChildSettleOutcome::Failed,
            ),
        };
        let command = completion_command(parent, deferred, outcome).await?;
        match parent
            .complete_external(command)
            .await
            .map_err(|error| ChildRunBridgeError::failed(error.to_string()))?
        {
            ExternalRouteOutcome::Committed(_) | ExternalRouteOutcome::Idempotent { .. } => {
                Ok(settle)
            }
            ExternalRouteOutcome::Rejected { reason_code, .. } => {
                Err(ChildRunBridgeError::failed(reason_code))
            }
        }
    }

    /// Recover and settle every outstanding deferred tool call on `parent`.
    ///
    /// Re-entry attaches through [`AgentRun::start_or_attach_child`]. A second
    /// recover after successful completion sees no outstanding deferrals.
    ///
    /// # Errors
    ///
    /// Returns a recover, planner, child-start, resolve, or completion failure.
    pub async fn recover(
        &self,
        parent: &AgentRun,
    ) -> Result<Vec<ChildSettleOutcome>, ChildRunBridgeError> {
        Box::pin(self.recover_inner(parent)).await
    }

    async fn recover_inner(
        &self,
        parent: &AgentRun,
    ) -> Result<Vec<ChildSettleOutcome>, ChildRunBridgeError> {
        let commit = parent
            .recover_commit()
            .await
            .map_err(|error| ChildRunBridgeError::failed(error.to_string()))?;
        let mut outcomes = Vec::new();
        for pending in super::scan::outstanding_deferrals(commit.state()) {
            outcomes.push(self.settle(parent, &pending.deferred).await?);
        }
        Ok(outcomes)
    }

    async fn pump_child(&self, child: &AgentRun, context: &ChildEventContext) {
        // One in-flight callback and no queued batches. It is polled alongside
        // event consumption, so a pending sink never gates child settlement.
        // Completion or cancellation of this pump drops the callback directly.
        let mut delivery: Option<PortFuture<()>> = None;
        loop {
            let mut next = Box::pin(child.next_event_batch());
            let batch = poll_fn(|cx| {
                if delivery
                    .as_mut()
                    .is_some_and(|future| future.as_mut().poll(cx).is_ready())
                {
                    delivery = None;
                }
                next.as_mut().poll(cx)
            })
            .await;
            match batch {
                Ok(Some(batch)) if delivery.is_none() => {
                    if let Some(sink) = &self.sink {
                        let sink = Arc::clone(sink);
                        let context = context.clone();
                        delivery = Some(Box::pin(async move {
                            let Ok(mut future) =
                                catch_unwind(AssertUnwindSafe(|| sink.on_batch(&context, &batch)))
                            else {
                                return;
                            };
                            let callback = poll_fn(move |cx| {
                                match catch_unwind(AssertUnwindSafe(|| future.as_mut().poll(cx))) {
                                    Ok(poll) => poll.map(|_| ()),
                                    Err(_) => Poll::Ready(()),
                                }
                            });
                            let _ = driver::timeout(Duration::from_secs(1), callback).await;
                        }));
                    }
                }
                Ok(Some(_)) => {} // Best-effort notifications are dropped while busy.
                Ok(None) | Err(_) => return,
            }
        }
    }
}

async fn completion_command(
    parent: &AgentRun,
    deferred: &EffectDeferred,
    outcome: ExternalEffectOutcome,
) -> Result<ExternalEffectCompletionCommand, ChildRunBridgeError> {
    let commit = parent
        .recover_commit()
        .await
        .map_err(|error| ChildRunBridgeError::failed(error.to_string()))?;
    let accepted = commit
        .state()
        .accepted()
        .ok_or_else(|| ChildRunBridgeError::failed("parent run is not accepted"))?;
    let security = accepted.security();
    let authorization = AuthorizationEvidence::try_new(
        security.authorization_policy_version(),
        security.authorization_decision_id(),
    )
    .map_err(|error| ChildRunBridgeError::failed(error.to_string()))?;
    let completion = ExternalEffectCompletion::try_new(
        deferred.effect_id,
        deferred.effect_id.to_canonical_string(),
        outcome,
    )
    .map_err(|error| ChildRunBridgeError::failed(error.to_string()))?;
    ExternalEffectCompletionCommand::try_new(
        parent.locator().clone(),
        security.principal().clone(),
        authorization,
        completion,
    )
    .map_err(|error| ChildRunBridgeError::failed(error.to_string()))
}

async fn tool_result_output(
    parent: &AgentRun,
    deferred: &EffectDeferred,
    text: &str,
    is_error: bool,
) -> Result<RawJson, ChildRunBridgeError> {
    let commit = parent
        .recover_commit()
        .await
        .map_err(|error| ChildRunBridgeError::failed(error.to_string()))?;
    let call = commit
        .state()
        .active_tool_batch()
        .and_then(|batch| {
            batch
                .calls
                .iter()
                .find(|call| call.assigned.effect_id == deferred.effect_id)
        })
        .ok_or_else(|| ChildRunBridgeError::failed("deferred tool call is not active"))?;
    let result = ToolResultBlock::try_new(
        *call.assigned.plan.call().tool_call_id(),
        vec![ContentBlock::Text(TextBlock::try_new(text).map_err(
            |error| ChildRunBridgeError::failed(error.to_string()),
        )?)],
        is_error,
    )
    .map_err(|error| ChildRunBridgeError::failed(error.to_string()))?;
    let encoded = serde_json::to_string(&result)
        .map_err(|error| ChildRunBridgeError::failed(error.to_string()))?;
    RawJson::parse(encoded).map_err(|error| ChildRunBridgeError::failed(error.to_string()))
}
