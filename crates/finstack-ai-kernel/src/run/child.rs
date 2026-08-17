//! Child-run placement, locators, and prepared mappings.

use serde::{Deserialize, Serialize};

use crate::digest::Digest;
use crate::external::OperationLocator;
use crate::ids::{EffectId, RunId};
use crate::refs::ComponentRef;

use super::error::RunError;
use crate::ids::BudgetReservationId;
use crate::refs::ExternalHandleRef;

/// Placement selected before a child mapping is committed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChildPlacement {
    /// Exact new lane in the parent session.
    CompatibleLaneInParentSession,
    /// Exact main lane in an isolated child session.
    IsolatedChildSession,
    /// Exact remote route plus remote operation locator.
    RemoteChildSession,
}

/// Non-secret remote route frozen by parent preparation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteRouteRef {
    /// Remote service component.
    pub service: ComponentRef,
    /// Opaque non-secret route handle.
    pub route: ExternalHandleRef,
}

/// Complete immutable child locator committed before invocation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChildRunLocator {
    /// Exact child session/lane/run operation identity.
    pub operation: OperationLocator,
    /// Remote route for remote placement only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<RemoteRouteRef>,
}

impl ChildRunLocator {
    /// Validate that remote-route presence matches the selected placement.
    ///
    /// # Errors
    ///
    /// Returns [`RunError::ChildLocatorPlacementMismatch`] for incomplete or
    /// over-specified locators.
    pub fn validate_for(&self, placement: ChildPlacement) -> Result<(), RunError> {
        if matches!(placement, ChildPlacement::RemoteChildSession) == self.remote.is_some() {
            Ok(())
        } else {
            Err(RunError::ChildLocatorPlacementMismatch)
        }
    }
}

/// Parent-owned durable mapping from one child-agent effect to one exact child.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChildRunPrepared {
    /// Parent run owning the child-agent effect.
    pub parent_run_id: RunId,
    /// Parent effect identity; one mapping is allowed for this pair.
    pub parent_effect_id: EffectId,
    /// Complete preallocated child locator.
    pub child: ChildRunLocator,
    /// Canonical digest of the normalized child request.
    pub request_digest: Digest,
    /// Frozen placement policy result.
    pub placement: ChildPlacement,
    /// Shared-budget reservation when required.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_reservation_id: Option<BudgetReservationId>,
}

impl ChildRunPrepared {
    /// Validate intrinsic locator and lineage shape.
    ///
    /// # Errors
    ///
    /// Returns a run error if placement is incomplete, tenant scope differs,
    /// or the child reuses the parent run identity.
    pub fn validate(&self, parent_tenant_scope: &str) -> Result<(), RunError> {
        self.child.validate_for(self.placement)?;
        if self.child.operation.tenant_scope.as_ref() != parent_tenant_scope {
            return Err(RunError::ChildTenantScopeMismatch);
        }
        if self.child.operation.run_id == self.parent_run_id {
            return Err(RunError::ChildRunIdentityReuse);
        }
        Ok(())
    }
}
