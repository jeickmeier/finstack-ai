//! Expiry policy for interactions that cross their deadline.
//!
//! The sweep never invents a decision on its own: it asks an
//! [`ExpiryPolicy`], and a policy that declines leaves the row `Open` for the
//! next sweep. [`ApprovalExpiry`] is the fail-closed default — an approval
//! nobody answered in time is refused, and every other kind is left to the
//! host.

use finstack_ai_kernel::{
    AuthorizationEvidence, InteractionKind, InteractionRequest, PrincipalRef, RawJson,
};

use crate::error::HitlError;
use crate::row::InteractionRow;

/// What an expired interaction resolves to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpiryResolution {
    /// Principal recorded as the resolver of the expiry.
    pub principal: PrincipalRef,
    /// Authorization evidence for the expiry decision.
    pub evidence: AuthorizationEvidence,
    /// Response payload delivered to the waiting run.
    pub payload: RawJson,
}

/// Host policy for interactions that cross `expires_at`.
pub trait ExpiryPolicy: Send + Sync {
    /// Decide what `row` (whose decoded envelope is `request`) resolves to
    /// now that its deadline has passed. `Ok(None)` takes no action and
    /// leaves the row `Open`.
    ///
    /// # Errors
    ///
    /// Returns a [`HitlError`] when the policy cannot decide; the sweep
    /// aborts on the first such error.
    fn expire(
        &self,
        row: &InteractionRow,
        request: &InteractionRequest,
    ) -> Result<Option<ExpiryResolution>, HitlError>;
}

/// Default policy: refuse expired approvals, leave every other kind
/// untouched.
///
/// Refusals are attributed to the principal
/// `("finstack.workflow.hitl", "expiry", Some(tenant))` with evidence
/// `("hitl-expiry-v1", "refuse-on-expiry")` and the payload
/// `{"approved": false}`, which is what the approval response schema expects
/// for a denial.
#[derive(Debug, Clone, Copy, Default)]
pub struct ApprovalExpiry;

impl ExpiryPolicy for ApprovalExpiry {
    fn expire(
        &self,
        row: &InteractionRow,
        request: &InteractionRequest,
    ) -> Result<Option<ExpiryResolution>, HitlError> {
        if !matches!(request.kind(), InteractionKind::Approval) {
            return Ok(None);
        }
        let principal = PrincipalRef::try_new(
            "finstack.workflow.hitl",
            "expiry",
            Some(row.tenant_scope.as_ref()),
        )
        .map_err(|_| HitlError::InvalidResolution {
            code: "hitl_expiry_principal",
        })?;
        let evidence = AuthorizationEvidence::try_new("hitl-expiry-v1", "refuse-on-expiry")
            .map_err(|_| HitlError::InvalidResolution {
                code: "hitl_expiry_evidence",
            })?;
        let payload =
            RawJson::parse(r#"{"approved":false}"#).map_err(|_| HitlError::InvalidResolution {
                code: "hitl_expiry_payload",
            })?;
        Ok(Some(ExpiryResolution {
            principal,
            evidence,
            payload,
        }))
    }
}

/// What one [`crate::HitlRouter::sweep`] pass changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SweepReport {
    /// Rows whose deadline passed and whose policy resolution was delivered.
    pub expired: usize,
    /// Active rows closed because their wake row was already consumed.
    pub reconciled: usize,
}
