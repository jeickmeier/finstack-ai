//! In-process HITL inbox store for tests and non-durable deployments.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

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

    fn rows(&self) -> Result<MutexGuard<'_, Rows>, HitlError> {
        self.rows.lock().map_err(|_| HitlError::StoreUnavailable {
            code: "memory_hitl_lock_poisoned",
        })
    }

    /// Rows matching `keep`, in the ordering contract shared by `load_open`
    /// and `load_active`, cut to `limit`.
    fn select(
        &self,
        limit: usize,
        keep: impl Fn(&InteractionRow) -> bool,
    ) -> Result<Vec<InteractionRow>, HitlError> {
        let mut selected: Vec<InteractionRow> = self
            .rows()?
            .values()
            .filter(|row| keep(row))
            .cloned()
            .collect();
        selected.sort_by(|a, b| {
            a.requested_at
                .cmp(&b.requested_at)
                .then_with(|| a.interaction_id.cmp(&b.interaction_id))
        });
        selected.truncate(limit);
        Ok(selected)
    }
}

impl HitlInboxStore for MemoryHitlStore {
    fn upsert(&self, row: &InteractionRow) -> Result<(), HitlError> {
        let mut rows = self.rows()?;
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
        Ok(self
            .rows()?
            .get(&(Arc::from(tenant_scope), Arc::from(interaction_id)))
            .cloned())
    }

    fn load_open(
        &self,
        tenant_scope: &str,
        limit: usize,
    ) -> Result<Vec<InteractionRow>, HitlError> {
        self.select(limit, |row| {
            row.tenant_scope.as_ref() == tenant_scope
                && matches!(
                    row.status,
                    InteractionStatus::Open | InteractionStatus::Rejected
                )
        })
    }

    fn load_active(&self, limit: usize) -> Result<Vec<InteractionRow>, HitlError> {
        self.select(limit, |row| {
            matches!(
                row.status,
                InteractionStatus::Open | InteractionStatus::Buffered
            )
        })
    }

    fn transition(
        &self,
        tenant_scope: &str,
        interaction_id: &str,
        transition: InteractionTransition<'_>,
    ) -> Result<bool, HitlError> {
        let mut rows = self.rows()?;
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
