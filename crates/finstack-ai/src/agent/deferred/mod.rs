//! Deferred tool-effect child-run bridge contracts.

mod bridge;
mod planner;
mod scan;
mod sink;

#[cfg(test)]
mod tests;

pub use bridge::{
    CHILD_RUN_BRIDGE_FAILED, CHILD_RUN_BRIDGE_PLANNER_REJECTED,
    CHILD_RUN_BRIDGE_PLANNER_UNAVAILABLE, ChildRunBridge, ChildRunBridgeError, ChildSettleOutcome,
};
pub use planner::{ChildPlanContext, ChildRunResolver, DeferredChildPlanner, DeferredPlanError};
pub use scan::OutstandingDeferral;
pub use sink::{ChildEventContext, ChildEventSink};
