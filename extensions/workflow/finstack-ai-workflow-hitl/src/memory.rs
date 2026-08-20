//! In-process HITL inbox store for tests and non-durable deployments.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use finstack_ai_kernel::Timestamp;

use crate::error::HitlError;
use crate::row::{InteractionRow, InteractionStatus};
use crate::store::HitlInboxStore;

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
        rows.insert(
            (
                Arc::clone(&row.tenant_scope),
                Arc::clone(&row.interaction_id),
            ),
            row.clone(),
        );
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

    fn load_open(&self, tenant_scope: &str) -> Result<Vec<InteractionRow>, HitlError> {
        let rows = self.rows.lock().map_err(|_| HitlError::StoreUnavailable {
            code: "memory_hitl_lock_poisoned",
        })?;
        let mut open: Vec<InteractionRow> = rows
            .values()
            .filter(|row| {
                row.tenant_scope.as_ref() == tenant_scope && row.status == InteractionStatus::Open
            })
            .cloned()
            .collect();
        sort_by_requested_then_id(&mut open);
        Ok(open)
    }

    fn load_active(&self) -> Result<Vec<InteractionRow>, HitlError> {
        let rows = self.rows.lock().map_err(|_| HitlError::StoreUnavailable {
            code: "memory_hitl_lock_poisoned",
        })?;
        let mut active: Vec<InteractionRow> = rows
            .values()
            .filter(|row| {
                matches!(
                    row.status,
                    InteractionStatus::Open | InteractionStatus::Delivered
                )
            })
            .cloned()
            .collect();
        sort_by_requested_then_id(&mut active);
        Ok(active)
    }

    fn set_status(
        &self,
        tenant_scope: &str,
        interaction_id: &str,
        status: InteractionStatus,
        resolved_by: Option<&str>,
        updated_at: Timestamp,
    ) -> Result<(), HitlError> {
        let mut rows = self.rows.lock().map_err(|_| HitlError::StoreUnavailable {
            code: "memory_hitl_lock_poisoned",
        })?;
        let Some(row) = rows.get_mut(&(Arc::from(tenant_scope), Arc::from(interaction_id))) else {
            return Err(HitlError::UnknownInteraction);
        };
        row.status = status;
        row.resolved_by = resolved_by.map(Arc::from);
        row.updated_at = updated_at;
        Ok(())
    }
}
