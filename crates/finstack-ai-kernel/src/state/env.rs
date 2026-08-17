//! Explicit transition time and preallocated identifiers.

use serde::{Deserialize, Serialize};

use crate::primitives::AllocatedIds;
use crate::primitives::Timestamp;

/// Runtime-supplied nondeterministic values for one pure decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransitionEnv {
    /// Semantic transition timestamp.
    pub now: Timestamp,
    /// Ordered bags of preallocated identifiers.
    pub ids: AllocatedIds,
}
