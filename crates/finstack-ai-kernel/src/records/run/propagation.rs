//! Child-run propagation policy.

use serde::{Deserialize, Serialize};

/// Cancellation propagation policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CancellationPropagation {
    /// Cascade cancellation to children.
    Cascade,
    /// Detach only when preauthorized.
    DetachOnlyIfPreauthorized,
}

/// Deadline propagation policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeadlinePropagation {
    /// Child deadline is min(parent, child).
    MinimumOfParentAndChild,
}

/// Budget propagation policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetPropagation {
    /// Shared budget scope.
    SharedScope,
    /// Reserved child allocation.
    ReservedChildAllocation,
}

/// Principal propagation policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrincipalPropagation {
    /// Inherit parent principal.
    Inherit,
    /// Attenuated delegation.
    AttenuatedDelegation,
}

/// Propagation policy bundle on `RunAccepted`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunPropagationPolicy {
    /// Cancellation policy.
    pub cancellation: CancellationPropagation,
    /// Deadline policy.
    pub deadline: DeadlinePropagation,
    /// Budget policy.
    pub budget: BudgetPropagation,
    /// Principal policy.
    pub principal: PrincipalPropagation,
}
