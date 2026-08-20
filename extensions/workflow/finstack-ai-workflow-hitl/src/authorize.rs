//! Authorization hook for interaction resolution.

use finstack_ai_kernel::{InteractionRequest, PrincipalRef};

use crate::error::HitlError;
use crate::row::InteractionRow;

/// Decides whether a principal may resolve a pending interaction.
///
/// Implementations fail closed: any doubt is a denial, expressed as
/// [`HitlError::Unauthorized`] with a stable reason code.
pub trait ResolveAuthorizer: Send + Sync {
    /// Authorize `principal` to resolve `row` (whose decoded envelope is
    /// `request`).
    ///
    /// # Errors
    ///
    /// Returns [`HitlError::Unauthorized`] with a stable reason code when the
    /// principal may not resolve this interaction.
    fn authorize(
        &self,
        row: &InteractionRow,
        request: &InteractionRequest,
        principal: &PrincipalRef,
    ) -> Result<(), HitlError>;
}

/// Default authorizer: the principal's tenant must equal the interaction's
/// `tenant_scope`.
///
/// An unscoped principal is denied — the kernel treats `None` as
/// "inherit", which is not a claim this battery can verify.
#[derive(Debug, Clone, Copy, Default)]
pub struct TenantAuthorizer;

impl ResolveAuthorizer for TenantAuthorizer {
    fn authorize(
        &self,
        row: &InteractionRow,
        _request: &InteractionRequest,
        principal: &PrincipalRef,
    ) -> Result<(), HitlError> {
        if principal.tenant_scope() == Some(row.tenant_scope.as_ref()) {
            return Ok(());
        }
        Err(HitlError::Unauthorized {
            code: "tenant_mismatch",
        })
    }
}
