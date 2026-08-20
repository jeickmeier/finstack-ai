//! Adapter-owned HITL inbox contract.

use finstack_ai_kernel::Timestamp;

use crate::error::HitlError;
use crate::row::{InteractionRow, InteractionStatus, InteractionSummary};

/// Adapter-owned HITL inbox. A hint; the journal is authority.
pub trait HitlInboxStore: Send + Sync {
    /// Insert or replace one row, keyed by `(tenant_scope, interaction_id)`.
    /// Replaces an existing row wholesale (redelivery-safe).
    ///
    /// # Errors
    ///
    /// Returns store-unavailable or integrity failures.
    fn upsert(&self, row: &InteractionRow) -> Result<(), HitlError>;

    /// Load one row by tenant and interaction id, if present.
    ///
    /// # Errors
    ///
    /// Returns store-unavailable or integrity failures.
    fn load(
        &self,
        tenant_scope: &str,
        interaction_id: &str,
    ) -> Result<Option<InteractionRow>, HitlError>;

    /// Open rows for one tenant, ordered by `requested_at` then
    /// `interaction_id`.
    ///
    /// # Errors
    ///
    /// Returns store-unavailable or integrity failures.
    fn load_open(&self, tenant_scope: &str) -> Result<Vec<InteractionRow>, HitlError>;

    /// Every Open or Delivered row across tenants (sweep input), same
    /// ordering as [`HitlInboxStore::load_open`].
    ///
    /// # Errors
    ///
    /// Returns store-unavailable or integrity failures.
    fn load_active(&self) -> Result<Vec<InteractionRow>, HitlError>;

    /// Every Open or Delivered row across tenants as a summary, same
    /// ordering as [`HitlInboxStore::load_active`] — the sweep's reconcile
    /// input, without the serialized request payload. Backends with a query
    /// engine should override this so a sweep does not fetch request blobs
    /// it never reads.
    ///
    /// # Errors
    ///
    /// Returns store-unavailable or integrity failures.
    fn load_active_summaries(&self) -> Result<Vec<InteractionSummary>, HitlError> {
        Ok(self
            .load_active()?
            .into_iter()
            .map(|row| InteractionSummary {
                tenant_scope: row.tenant_scope,
                interaction_id: row.interaction_id,
                status: row.status,
                resolved_by: row.resolved_by,
                expires_at: row.expires_at,
            })
            .collect())
    }

    /// Transition a row's status. Returns [`HitlError::UnknownInteraction`]
    /// when no row exists for the given key.
    ///
    /// # Errors
    ///
    /// Returns store-unavailable or integrity failures, or
    /// [`HitlError::UnknownInteraction`] when the row does not exist.
    fn set_status(
        &self,
        tenant_scope: &str,
        interaction_id: &str,
        status: InteractionStatus,
        resolved_by: Option<&str>,
        updated_at: Timestamp,
    ) -> Result<(), HitlError>;
}
