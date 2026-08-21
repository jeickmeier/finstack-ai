//! Adapter-owned HITL inbox contract.

use finstack_ai_kernel::Timestamp;

use crate::error::HitlError;
use crate::row::{InteractionRow, InteractionStatus, InteractionSummary};

/// Atomic status update applied by [`HitlInboxStore::transition`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InteractionTransition<'a> {
    /// Status required for the compare-and-set to succeed.
    pub expected: InteractionStatus,
    /// Status written when the expected status matches.
    pub next: InteractionStatus,
    /// Resolving principal subject to record, if any.
    pub resolved_by: Option<&'a str>,
    /// Stable ingress or reconciliation outcome code, if any.
    pub outcome_code: Option<&'a str>,
    /// Timestamp written with the transition.
    pub updated_at: Timestamp,
}

/// Adapter-owned HITL inbox. A hint; the journal is authority.
pub trait HitlInboxStore: Send + Sync {
    /// Insert or recapture one row, keyed by `(tenant_scope, interaction_id)`.
    /// Recapture atomically preserves every non-Closed disposition; a stale
    /// Closed row is reopened from the newly committed wait.
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

    /// Actionable Open or Rejected rows for one tenant, ordered by
    /// `requested_at` then `interaction_id`.
    ///
    /// # Errors
    ///
    /// Returns store-unavailable or integrity failures.
    fn load_open(&self, tenant_scope: &str, limit: usize)
    -> Result<Vec<InteractionRow>, HitlError>;

    /// Every Open or Buffered row across tenants (sweep input), same
    /// ordering as [`HitlInboxStore::load_open`].
    ///
    /// # Errors
    ///
    /// Returns store-unavailable or integrity failures.
    fn load_active(&self, limit: usize) -> Result<Vec<InteractionRow>, HitlError>;

    /// Every Open or Buffered row across tenants as a summary, same
    /// ordering as [`HitlInboxStore::load_active`] — the sweep's reconcile
    /// input, without the serialized request payload. Backends with a query
    /// engine should override this so a sweep does not fetch request blobs
    /// it never reads.
    ///
    /// # Errors
    ///
    /// Returns store-unavailable or integrity failures.
    fn load_active_summaries(&self, limit: usize) -> Result<Vec<InteractionSummary>, HitlError> {
        Ok(self
            .load_active(limit)?
            .into_iter()
            .map(|row| InteractionSummary {
                tenant_scope: row.tenant_scope,
                interaction_id: row.interaction_id,
                status: row.status,
                resolved_by: row.resolved_by,
                outcome_code: row.outcome_code,
            })
            .collect())
    }

    /// Compare-and-set a row's status. Returns `false` on an expected-state
    /// mismatch and [`HitlError::UnknownInteraction`] when the row is absent.
    ///
    /// # Errors
    ///
    /// Returns store-unavailable or integrity failures, or
    /// [`HitlError::UnknownInteraction`] when the row does not exist.
    fn transition(
        &self,
        tenant_scope: &str,
        interaction_id: &str,
        transition: InteractionTransition<'_>,
    ) -> Result<bool, HitlError>;
}
