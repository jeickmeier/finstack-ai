//! Host-side complete-set queue for mid-run capability activation.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use finstack_ai_kernel::{ActiveCapability, CapabilityId, Digest, RunId};
use thiserror::Error;

/// Stable error when the per-run activation bound is reached.
pub const CAPABILITY_ACTIVATION_BOUND: &str = "capability_activation_bound";
/// Stable error when the host cannot read or submit an activation.
pub const CAPABILITY_ACTIVATION_FAILED: &str = "capability_activation_failed";

/// Default concurrent mid-run activations allowed on one run.
pub const MAX_CONCURRENT_CAPABILITY_ACTIVATIONS: u16 = 8;

/// Shared queue the skills toolset fills and the native driver drains.
#[derive(Debug)]
pub struct NativeCapabilityHost {
    catalog: String,
    lock_digest: Mutex<Option<Digest>>,
    active: Mutex<BTreeMap<RunId, Arc<[ActiveCapability]>>>,
    pending: Mutex<BTreeMap<RunId, Vec<Arc<[ActiveCapability]>>>>,
    in_flight: Mutex<BTreeMap<RunId, u16>>,
}

impl NativeCapabilityHost {
    /// Construct a host with a frozen compact catalog; the per-run bound is
    /// [`MAX_CONCURRENT_CAPABILITY_ACTIVATIONS`].
    #[must_use]
    pub fn new(catalog: impl Into<String>) -> Self {
        Self {
            catalog: catalog.into(),
            lock_digest: Mutex::new(None),
            active: Mutex::new(BTreeMap::new()),
            pending: Mutex::new(BTreeMap::new()),
            in_flight: Mutex::new(BTreeMap::new()),
        }
    }

    /// Frozen compact catalog rendered for `capability_list`.
    #[must_use]
    pub fn compact_catalog(&self) -> &str {
        &self.catalog
    }

    /// Record the lock fingerprint used as `resolved_plan_digest`.
    ///
    /// Later `capability_activate` submissions reuse this digest so
    /// `prior_plan_digest` can chain without re-resolving the agent.
    pub fn set_lock_digest(&self, digest: Digest) {
        if let Ok(mut guard) = self.lock_digest.lock() {
            *guard = Some(digest);
        }
    }

    /// Seed the current committed set after initial or mid-run activation.
    ///
    /// A poisoned lock is ignored; the next read fails closed.
    pub fn seed_active(&self, run_id: RunId, active: Arc<[ActiveCapability]>) {
        if let Ok(mut guard) = self.active.lock() {
            guard.insert(run_id, active);
        }
    }

    /// Borrow the last seeded active set for one run.
    ///
    /// # Errors
    ///
    /// Returns [`ActivationHostError::Failed`] when the host lock is poisoned.
    pub fn active(&self, run_id: RunId) -> Result<Vec<ActiveCapability>, ActivationHostError> {
        let guard = self
            .active
            .lock()
            .map_err(|_| ActivationHostError::Failed {
                reason: Arc::from("activation host lock poisoned"),
            })?;
        Ok(guard
            .get(&run_id)
            .map(|set| set.to_vec())
            .unwrap_or_default())
    }

    /// Queue one complete intended set. Fails when the concurrent bound is hit.
    ///
    /// Overflow fails this call and does not evict an earlier activation.
    ///
    /// # Errors
    ///
    /// Returns [`ActivationHostError::Bound`] when
    /// [`MAX_CONCURRENT_CAPABILITY_ACTIVATIONS`] in-flight activations already
    /// exist for `run_id`, or
    /// [`ActivationHostError::Failed`] when the host lock is poisoned.
    pub fn queue_activation(
        &self,
        run_id: RunId,
        complete: Vec<ActiveCapability>,
    ) -> Result<(), ActivationHostError> {
        {
            let mut counts = self
                .in_flight
                .lock()
                .map_err(|_| ActivationHostError::Failed {
                    reason: Arc::from("activation host lock poisoned"),
                })?;
            let count = counts.entry(run_id).or_insert(0);
            if *count >= MAX_CONCURRENT_CAPABILITY_ACTIVATIONS {
                return Err(ActivationHostError::Bound);
            }
            *count = count.saturating_add(1);
        }
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| ActivationHostError::Failed {
                reason: Arc::from("activation host lock poisoned"),
            })?;
        pending.entry(run_id).or_default().push(complete.into());
        Ok(())
    }

    /// Drain queued complete sets for one run and union them with the seeded set.
    ///
    /// Returns `None` when nothing is queued.
    pub fn take_pending(&self, run_id: RunId) -> Option<Vec<ActiveCapability>> {
        let queued = self
            .pending
            .lock()
            .ok()
            .and_then(|mut pending| pending.remove(&run_id))
            .unwrap_or_default();
        if queued.is_empty() {
            return None;
        }
        if let Ok(mut counts) = self.in_flight.lock() {
            counts.remove(&run_id);
        }
        let mut merged: BTreeMap<CapabilityId, ActiveCapability> = BTreeMap::new();
        if let Ok(active) = self.active.lock()
            && let Some(current) = active.get(&run_id)
        {
            for item in current.iter() {
                merged.insert(item.capability_id.clone(), item.clone());
            }
        }
        for set in queued {
            for item in set.iter() {
                merged.insert(item.capability_id.clone(), item.clone());
            }
        }
        Some(merged.into_values().collect())
    }

    /// Whether this process still holds an uncommitted activation proposal.
    #[cfg(feature = "durable-host")]
    pub(super) fn has_pending(&self, run_id: RunId) -> bool {
        self.pending
            .lock()
            .is_ok_and(|pending| pending.get(&run_id).is_some_and(|sets| !sets.is_empty()))
    }

    /// Lock fingerprint used as the submitted `resolved_plan_digest`.
    #[must_use]
    pub fn lock_digest(&self) -> Option<Digest> {
        self.lock_digest.lock().ok().and_then(|guard| *guard)
    }
}

/// Host failure surfaced as a tool result.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ActivationHostError {
    /// Concurrent activation bound reached; no eviction.
    #[error("capability_activation_bound")]
    Bound,
    /// Host could not read or queue the activation.
    #[error("capability_activation_failed")]
    Failed {
        /// Stable non-secret reason.
        reason: Arc<str>,
    },
}
