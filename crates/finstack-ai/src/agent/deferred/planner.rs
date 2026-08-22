//! Planner and child-run resolver contracts for deferred tool effects.

use finstack_ai_kernel::{ChildRunLocator, EffectDeferred, OperationLocator};
use finstack_ai_runtime::child::ChildRunRequest;
use finstack_ai_runtime::ports::PortFuture;
use thiserror::Error;

use super::bridge::ChildRunBridgeError;
use crate::AgentRun;

/// Context offered to one [`DeferredChildPlanner`] for a committed deferral.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildPlanContext {
    /// Parent run that owns the deferred effect.
    pub parent: OperationLocator,
    /// Committed first-pass deferral.
    pub deferred: EffectDeferred,
}

/// Why a planner declined to claim a deferred handle.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DeferredPlanError {
    /// The planner recognized the handle and refused it.
    #[error("deferred child planner rejected the handle: {message}")]
    Rejected {
        /// Non-secret explanation.
        message: String,
    },
    /// The planner cannot decide safely right now.
    #[error("deferred child planner is unavailable: {message}")]
    Unavailable {
        /// Non-secret explanation.
        message: String,
    },
}

/// Claims or declines a deferred parent effect for a child run.
///
/// Return [`Ok`]`(None)` when this planner does not own the handle. The
/// in-process invoker usually implements both [`finstack_ai_runtime::child::AgentInvoker`]
/// and [`ChildRunResolver`].
pub trait DeferredChildPlanner: Send + Sync {
    /// Plan a child request for `context`, or return `Ok(None)` if unowned.
    fn plan(
        &self,
        context: &ChildPlanContext,
    ) -> PortFuture<Result<Option<ChildRunRequest>, DeferredPlanError>>;
}

/// Resolves a prepared child locator to a live [`AgentRun`].
///
/// The in-process invoker usually implements both
/// [`finstack_ai_runtime::child::AgentInvoker`] and this trait.
pub trait ChildRunResolver: Send + Sync {
    /// Return the live child handle for a previously prepared locator.
    fn resolve(&self, child: &ChildRunLocator)
    -> PortFuture<Result<AgentRun, ChildRunBridgeError>>;
}
