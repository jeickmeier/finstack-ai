//! Wake index — the adapter table that tells the tick loop which parked
//! sessions are ready to be resumed.

use std::sync::Arc;

use finstack_ai_kernel::{Id, IdTag, LaneId, RunId, SessionId, Timestamp};
use finstack_ai_runtime::ids::{ExternalClock, OsRandomSource, UuidV7Generator};

use crate::error::WorkerError;

/// UUID family for one acquisition of a workflow wake lease.
pub enum WakeLeaseTag {}
impl IdTag for WakeLeaseTag {
    const NAME: &'static str = "wake-lease";
}
/// Unique identity of one claim, including reacquisition by the same worker.
pub type WakeLeaseId = Id<WakeLeaseTag>;

/// Ownership fence returned by a successful claim. Stores compare its exact
/// identity on every owner mutation; worker labels are never ownership tokens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeLease {
    /// Tenant of the claimed row.
    pub tenant_scope: Arc<str>,
    /// Session of the claimed row.
    pub session_id: SessionId,
    /// Unique acquisition identity.
    pub id: WakeLeaseId,
}
impl WakeLease {
    /// Allocate a fresh claim identity using operating-system entropy.
    /// # Errors
    /// Returns store-unavailable when an identity cannot be generated.
    pub fn try_new(
        tenant_scope: &str,
        session_id: SessionId,
        now: Timestamp,
    ) -> Result<Self, WorkerError> {
        let id = UuidV7Generator::new(ExternalClock::new(now), OsRandomSource)
            .generate()
            .map_err(|_| WorkerError::StoreUnavailable {
                code: "wake_lease_entropy",
            })?;
        Ok(Self {
            tenant_scope: Arc::from(tenant_scope),
            session_id,
            id,
        })
    }
}

/// Validate an atomic wake-row mutation against its current ownership.
pub(crate) fn check_lease(
    tenant: &str,
    session: SessionId,
    current: Option<&WakeRow>,
    lease: Option<&WakeLease>,
) -> Result<(), WorkerError> {
    let valid = match lease {
        Some(lease) => {
            lease.tenant_scope.as_ref() == tenant
                && lease.session_id == session
                && current.is_some_and(|row| row.lease_id == Some(lease.id))
        }
        None => current.is_none_or(|row| row.leased_by.is_none() && row.lease_id.is_none()),
    };
    if valid {
        Ok(())
    } else {
        Err(WorkerError::LeaseLost)
    }
}

/// Why a parked session is expected to wake.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeReason {
    /// An accepted run awaiting application stage advancement.
    Runnable,
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
            Self::Runnable => "runnable",
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
            "runnable" => Ok(Self::Runnable),
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
    /// Unique acquisition identity, absent when unleased.
    pub lease_id: Option<WakeLeaseId>,
    /// When the current lease expires, if any.
    pub lease_expires_at: Option<Timestamp>,
    /// Number of failed claim/resume attempts recorded so far.
    pub attempts: u32,
}

/// Adapter-owned wake index. Not part of the kernel journal schema.
/// All mutations atomically compare the acquisition fence. Initial publication
/// (`lease = None`) may modify only unleased rows. Successful publication clears
/// the lease; callers must never manufacture lease state in the replacement row.
pub trait WakeIndexStore: Send + Sync {
    /// Insert or replace a hint, atomically checking ownership.
    /// # Errors
    /// Returns `LeaseLost` for a stale fence or unfenced write to a leased row.
    fn upsert(&self, row: &WakeRow, lease: Option<&WakeLease>) -> Result<(), WorkerError>;
    /// Remove a hint, atomically checking ownership.
    /// # Errors
    /// Returns `LeaseLost` for a stale fence or unfenced delete of a leased row.
    fn delete(
        &self,
        tenant_scope: &str,
        session_id: SessionId,
        lease: Option<&WakeLease>,
    ) -> Result<(), WorkerError>;
    /// Read a bounded, rotating page of due rows with absent or expired leases.
    /// A zero limit does not advance the cursor. Ordering across calls is not stable.
    /// # Errors
    /// Returns store-unavailable or integrity failures.
    fn load_due(&self, now: Timestamp, limit: usize) -> Result<Vec<WakeRow>, WorkerError>;
    /// Read every row for one tenant, including leased rows.
    /// # Errors
    /// Returns store-unavailable or integrity failures.
    fn load_tenant(&self, tenant_scope: &str) -> Result<Vec<WakeRow>, WorkerError>;
    /// Whether the tenant has an interaction hint for the pending identity.
    /// # Errors
    /// Returns store-unavailable or integrity failures.
    fn contains_interaction(
        &self,
        tenant_scope: &str,
        pending_id: &str,
    ) -> Result<bool, WorkerError>;
    /// Claim an absent or expired lease and return a fresh acquisition fence.
    /// # Errors
    /// Third-party stores fail closed unless they implement fenced claims.
    fn try_claim(
        &self,
        tenant_scope: &str,
        session_id: SessionId,
        worker_id: &str,
        now: Timestamp,
        lease_ttl_ms: u64,
    ) -> Result<Option<WakeLease>, WorkerError> {
        let _ = (tenant_scope, session_id, worker_id, now, lease_ttl_ms);
        Err(WorkerError::StoreUnavailable {
            code: "wake_claim_unsupported",
        })
    }
    /// Renew a matching, unexpired acquisition. Expired fences cannot resurrect leases.
    /// # Errors
    /// Returns store-unavailable or integrity failures.
    fn renew(
        &self,
        lease: &WakeLease,
        now: Timestamp,
        lease_ttl_ms: u64,
    ) -> Result<bool, WorkerError>;
    /// Release only the matching acquisition without changing retry state.
    /// # Errors
    /// Returns store-unavailable or integrity failures.
    fn release(&self, lease: &WakeLease) -> Result<bool, WorkerError>;
    /// Record failure and backoff only for the matching acquisition.
    /// # Errors
    /// Returns `LeaseLost` for a stale fence, or storage/integrity failures.
    fn record_failure(&self, lease: &WakeLease, retry_at: Timestamp) -> Result<(), WorkerError>;
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
        WakeReason::Runnable | WakeReason::Timer => row.wake_at.is_some_and(|due| due <= now),
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
