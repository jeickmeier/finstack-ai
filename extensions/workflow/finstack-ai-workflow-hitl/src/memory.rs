//! In-process HITL inbox store for tests and non-durable deployments.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use crate::error::HitlError;
use crate::row::{InteractionRow, InteractionStatus};
use crate::store::{HitlInboxStore, InteractionTransition};

type Rows = BTreeMap<(Arc<str>, Arc<str>), InteractionRow>;

/// In-memory HITL inbox store, keyed by `(tenant_scope, interaction_id)`.
#[derive(Debug, Default)]
pub struct MemoryHitlStore {
    rows: Mutex<Rows>,
}

impl MemoryHitlStore {
    /// Empty store with no rows.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

/// Sort rows by `requested_at` then `interaction_id`, matching the ordering
/// contract shared by `load_open` and `load_active`.
fn sort_by_requested_then_id(rows: &mut [InteractionRow]) {
    rows.sort_by(|a, b| {
        a.requested_at
            .cmp(&b.requested_at)
            .then_with(|| a.interaction_id.cmp(&b.interaction_id))
    });
}

impl HitlInboxStore for MemoryHitlStore {
    fn upsert(&self, row: &InteractionRow) -> Result<(), HitlError> {
        let mut rows = self.rows.lock().map_err(|_| HitlError::StoreUnavailable {
            code: "memory_hitl_lock_poisoned",
        })?;
        let key = (
            Arc::clone(&row.tenant_scope),
            Arc::clone(&row.interaction_id),
        );
        let mut captured = row.clone();
        if let Some(existing) = rows.get(&key)
            && existing.status != InteractionStatus::Closed
        {
            captured.status = existing.status;
            captured.resolved_by.clone_from(&existing.resolved_by);
            captured.outcome_code.clone_from(&existing.outcome_code);
            captured.updated_at = existing.updated_at;
        }
        rows.insert(key, captured);
        Ok(())
    }

    fn load(
        &self,
        tenant_scope: &str,
        interaction_id: &str,
    ) -> Result<Option<InteractionRow>, HitlError> {
        let rows = self.rows.lock().map_err(|_| HitlError::StoreUnavailable {
            code: "memory_hitl_lock_poisoned",
        })?;
        Ok(rows
            .get(&(Arc::from(tenant_scope), Arc::from(interaction_id)))
            .cloned())
    }

    fn load_open(
        &self,
        tenant_scope: &str,
        limit: usize,
    ) -> Result<Vec<InteractionRow>, HitlError> {
        let rows = self.rows.lock().map_err(|_| HitlError::StoreUnavailable {
            code: "memory_hitl_lock_poisoned",
        })?;
        let mut open: Vec<InteractionRow> = rows
            .values()
            .filter(|row| {
                row.tenant_scope.as_ref() == tenant_scope
                    && matches!(
                        row.status,
                        InteractionStatus::Open | InteractionStatus::Rejected
                    )
            })
            .cloned()
            .collect();
        sort_by_requested_then_id(&mut open);
        open.truncate(limit);
        Ok(open)
    }

    fn load_active(&self, limit: usize) -> Result<Vec<InteractionRow>, HitlError> {
        let rows = self.rows.lock().map_err(|_| HitlError::StoreUnavailable {
            code: "memory_hitl_lock_poisoned",
        })?;
        let mut active: Vec<InteractionRow> = rows
            .values()
            .filter(|row| {
                matches!(
                    row.status,
                    InteractionStatus::Open | InteractionStatus::Buffered
                )
            })
            .cloned()
            .collect();
        sort_by_requested_then_id(&mut active);
        active.truncate(limit);
        Ok(active)
    }

    fn transition(
        &self,
        tenant_scope: &str,
        interaction_id: &str,
        transition: InteractionTransition<'_>,
    ) -> Result<bool, HitlError> {
        let mut rows = self.rows.lock().map_err(|_| HitlError::StoreUnavailable {
            code: "memory_hitl_lock_poisoned",
        })?;
        let Some(row) = rows.get_mut(&(Arc::from(tenant_scope), Arc::from(interaction_id))) else {
            return Err(HitlError::UnknownInteraction);
        };
        if row.status != transition.expected {
            return Ok(false);
        }
        row.status = transition.next;
        row.resolved_by = transition.resolved_by.map(Arc::from);
        row.outcome_code = transition.outcome_code.map(Arc::from);
        row.updated_at = transition.updated_at;
        Ok(true)
    }
}
