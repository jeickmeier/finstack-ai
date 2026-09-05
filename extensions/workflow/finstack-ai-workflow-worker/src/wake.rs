//! Wake index — the adapter table that tells the tick loop which parked
//! sessions are ready to be resumed.

use std::sync::Arc;

use finstack_ai_kernel::{LaneId, RunId, SessionId, Timestamp};

use crate::error::WorkerError;

/// Why a parked session is expected to wake.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeReason {
    /// Woken by a scheduled timer (`wake_at`).
    Timer,
    /// Woken by an inbound interaction (message, tool result, etc.).
    Interaction,
    /// Woken by a deferred effect completing out of band.
    Deferred,
}

impl WakeReason {
    /// Stable lowercase wire representation.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Timer => "timer",
            Self::Interaction => "interaction",
            Self::Deferred => "deferred",
        }
    }

    /// Parse the wire representation produced by [`WakeReason::as_str`].
    ///
    /// # Errors
    ///
    /// Returns [`WorkerError::StoreIntegrity`] for any unknown value.
    pub fn parse(value: &str) -> Result<Self, WorkerError> {
        match value {
            "timer" => Ok(Self::Timer),
            "interaction" => Ok(Self::Interaction),
            "deferred" => Ok(Self::Deferred),
            _ => Err(WorkerError::StoreIntegrity {
                code: "wake_reason",
            }),
        }
    }
}

/// One wake-index row: a parked session and the condition under which it
/// should be resumed, plus the lease state used to coordinate workers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeRow {
    /// Tenant scope the session belongs to.
    pub tenant_scope: Arc<str>,
    /// Parked session.
    pub session_id: SessionId,
    /// Lane the session runs in.
    pub lane_id: LaneId,
    /// Run the session belongs to.
    pub run_id: RunId,
    /// Workflow kind recorded at park time, used to look up the ports
    /// factory on resume.
    pub workflow_kind: Arc<str>,
    /// Condition under which this session is due.
    pub reason: WakeReason,
    /// Earliest instant at which this row is due. Set at park time for
    /// [`WakeReason::Timer`] rows; for the inbox-driven reasons it is `None`
    /// until `record_failure` pushes it forward as a retry backoff. Either
    /// way, a set value gates dueness — see [`wake_due`].
    pub wake_at: Option<Timestamp>,
    /// Semantic deadline of the parked wait, when it has one. Written at park
    /// time for [`WakeReason::Interaction`] rows from the committed
    /// `InteractionRequest`'s `expires_at`; `None` means the wait carries no
    /// deadline and can only ever be woken by the inbox.
    ///
    /// This is deliberately *not* `wake_at`: `wake_at` is the retry gate that
    /// `record_failure` owns, and overloading it would make a backed-off row
    /// forget its deadline (and a deadline'd row invisible to inbox delivery
    /// until the deadline). The deadline never gates
    /// [`WakeIndexStore::load_due`]; the tick reads it after the fact to
    /// decide whether a parked interaction with no buffered response is due
    /// for the kernel's own expiry.
    pub expires_at: Option<Timestamp>,
    /// Identifier of the pending effect this row resumes.
    pub pending_id: Arc<str>,
    /// Worker currently holding the claim lease, if any.
    pub leased_by: Option<Arc<str>>,
    /// When the current lease expires, if any.
    pub lease_expires_at: Option<Timestamp>,
    /// Number of failed claim/resume attempts recorded so far.
    pub attempts: u32,
}

/// Adapter-owned wake index. Not part of the kernel journal schema.
pub trait WakeIndexStore: Send + Sync {
    /// Insert or replace one row, keyed by `(tenant_scope, session_id)`.
    ///
    /// # Errors
    ///
    /// Returns store-unavailable or integrity failures.
    fn upsert(&self, row: &WakeRow) -> Result<(), WorkerError>;

    /// Remove the row for one session, if present.
    ///
    /// # Errors
    ///
    /// Returns store-unavailable or integrity failures.
    fn delete(&self, tenant_scope: &str, session_id: SessionId) -> Result<(), WorkerError>;

    /// Every row (any tenant) that is due at `now` and not currently
    /// under an open lease, up to `limit`.
    ///
    /// Repeated bounded scans must make progress past previously returned
    /// rows even when callers leave them unchanged (for example, unanswered
    /// interactions). Built-in stores rotate in key order per store handle;
    /// a newly opened handle starts at the beginning. Ordering is not stable
    /// across calls. A zero limit does not advance the scan.
    ///
    /// # Errors
    ///
    /// Returns store-unavailable or integrity failures.
    fn load_due(&self, now: Timestamp, limit: usize) -> Result<Vec<WakeRow>, WorkerError>;

    /// Every row for one tenant, regardless of lease or due state.
    ///
    /// # Errors
    ///
    /// Returns store-unavailable or integrity failures.
    fn load_tenant(&self, tenant_scope: &str) -> Result<Vec<WakeRow>, WorkerError>;

    /// Whether one tenant currently has an interaction wake for `pending_id`.
    ///
    /// # Errors
    ///
    /// Returns store-unavailable or integrity failures.
    fn contains_interaction(
        &self,
        tenant_scope: &str,
        pending_id: &str,
    ) -> Result<bool, WorkerError>;

    /// Attempt to claim one session for `worker_id`, winning only when the
    /// existing lease is absent or expired. Third-party stores fail closed
    /// unless they override this method.
    ///
    /// # Errors
    ///
    /// Returns [`WorkerError::StoreUnavailable`] by default.
    fn try_claim(
        &self,
        tenant_scope: &str,
        session_id: SessionId,
        worker_id: &str,
        now: Timestamp,
        lease_ttl_ms: u64,
    ) -> Result<bool, WorkerError> {
        let _ = (tenant_scope, session_id, worker_id, now, lease_ttl_ms);
        Err(WorkerError::StoreUnavailable {
            code: "wake_claim_unsupported",
        })
    }

    /// Extend the current holder's lease. Succeeds only when `worker_id`
    /// already holds the lease. Third-party stores fail closed unless they
    /// override this method.
    ///
    /// # Errors
    ///
    /// Returns [`WorkerError::StoreUnavailable`] by default.
    fn renew(
        &self,
        tenant_scope: &str,
        session_id: SessionId,
        worker_id: &str,
        now: Timestamp,
        lease_ttl_ms: u64,
    ) -> Result<bool, WorkerError> {
        let _ = (tenant_scope, session_id, worker_id, now, lease_ttl_ms);
        Err(WorkerError::StoreUnavailable {
            code: "wake_renew_unsupported",
        })
    }

    /// Release a lease held by `worker_id` without modifying retry state.
    ///
    /// # Errors
    ///
    /// Returns store-unavailable or integrity failures.
    fn release(
        &self,
        tenant_scope: &str,
        session_id: SessionId,
        worker_id: &str,
    ) -> Result<bool, WorkerError>;

    /// Record a failed resume attempt: clears the lease, increments
    /// `attempts`, and schedules the next attempt at `retry_at`.
    ///
    /// # Errors
    ///
    /// Returns store-unavailable or integrity failures.
    fn record_failure(
        &self,
        tenant_scope: &str,
        session_id: SessionId,
        retry_at: Timestamp,
    ) -> Result<(), WorkerError>;
}

/// Whether the row's lease is absent or expired at `now`.
#[must_use]
pub fn lease_open(row: &WakeRow, now: Timestamp) -> bool {
    row.lease_expires_at.is_none_or(|expires| expires <= now)
}

/// Whether the row is due at `now` (lease aside).
///
/// Timer rows require a scheduled `wake_at` that has arrived. Non-timer rows
/// are driven by the inbox rather than the clock, so they are due
/// immediately — unless `record_failure` pushed `wake_at` forward as a
/// retry backoff, in which case they wait for it like a timer does. Freshly
/// parked non-timer rows carry `wake_at: None` and stay immediately due.
#[must_use]
pub fn wake_due(row: &WakeRow, now: Timestamp) -> bool {
    match row.reason {
        WakeReason::Timer => row.wake_at.is_some_and(|due| due <= now),
        WakeReason::Interaction | WakeReason::Deferred => row.wake_at.is_none_or(|due| due <= now),
    }
}

/// `now + ttl` as a timestamp, failing closed on overflow.
///
/// # Errors
///
/// Returns [`WorkerError::TimeOverflow`] if the arithmetic or the result
/// falls outside the representable range.
pub fn lease_deadline(now: Timestamp, lease_ttl_ms: u64) -> Result<Timestamp, WorkerError> {
    let ttl = i64::try_from(lease_ttl_ms).map_err(|_| WorkerError::TimeOverflow)?;
    let ms = now
        .as_unix_ms()
        .checked_add(ttl)
        .ok_or(WorkerError::TimeOverflow)?;
    Timestamp::from_unix_ms(ms).map_err(|_| WorkerError::TimeOverflow)
}
