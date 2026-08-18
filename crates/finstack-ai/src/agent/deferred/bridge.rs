//! Child-run bridge types. Construction and settle land in a later task.

use std::sync::Arc;

use thiserror::Error;

use super::planner::{ChildRunResolver, DeferredChildPlanner};
use super::sink::ChildEventSink;
use finstack_ai_runtime::AgentInvoker;

/// Stable code when a planner rejects a deferred handle.
pub const CHILD_RUN_BRIDGE_PLANNER_REJECTED: &str = "child_run_bridge_planner_rejected";
/// Stable code when a planner cannot decide safely.
pub const CHILD_RUN_BRIDGE_PLANNER_UNAVAILABLE: &str = "child_run_bridge_planner_unavailable";
/// Stable code when child start, resolve, or completion fails.
pub const CHILD_RUN_BRIDGE_FAILED: &str = "child_run_bridge_failed";

/// Binds deferred parent tool effects to child runs.
#[allow(dead_code, reason = "constructors and settle land in the next task")]
pub struct ChildRunBridge {
    pub(crate) planners: Vec<Arc<dyn DeferredChildPlanner>>,
    pub(crate) invoker: Arc<dyn AgentInvoker>,
    pub(crate) resolver: Arc<dyn ChildRunResolver>,
    pub(crate) sink: Option<Arc<dyn ChildEventSink>>,
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
}
