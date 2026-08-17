//! Run lineage and security errors.

use thiserror::Error;

use crate::limits::LimitsError;
use crate::refs::RefsError;

/// Run lineage / acceptance errors.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RunError {
    /// Child placement and remote-route fields disagree.
    #[error("child locator does not match placement")]
    ChildLocatorPlacementMismatch,
    /// Child locator crosses the authenticated tenant scope.
    #[error("child locator tenant scope differs from parent")]
    ChildTenantScopeMismatch,
    /// Child reuses its parent run identity.
    #[error("child locator reuses parent run identity")]
    ChildRunIdentityReuse,
    /// Invalid relation shape or depth.
    #[error("invalid run relation: {reason}")]
    InvalidRelation {
        /// Reason.
        reason: &'static str,
    },
    /// Child failed attenuation against parent.
    #[error("child run is not an attenuation of parent: {reason}")]
    NotAttenuated {
        /// Reason.
        reason: &'static str,
    },
    /// Security context fields are internally inconsistent.
    #[error("invalid run security context: {reason}")]
    InvalidSecurityContext {
        /// Reason.
        reason: &'static str,
    },
    /// Shared label/ref error.
    #[error(transparent)]
    Refs(#[from] RefsError),
    /// Run limits failed semantic validation.
    #[error(transparent)]
    Limits(#[from] LimitsError),
}

impl RunError {
    /// Stable error code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::ChildLocatorPlacementMismatch => "child_locator_placement_mismatch",
            Self::ChildTenantScopeMismatch => "child_tenant_scope_mismatch",
            Self::ChildRunIdentityReuse => "child_run_identity_reuse",
            Self::InvalidRelation { .. } => "invalid_run_relation",
            Self::NotAttenuated { .. } => "run_not_attenuated",
            Self::InvalidSecurityContext { .. } => "invalid_run_security_context",
            Self::Refs(inner) => inner.code(),
            Self::Limits(inner) => inner.code(),
        }
    }
}
