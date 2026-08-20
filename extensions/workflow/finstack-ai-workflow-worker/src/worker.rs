//! The leased worker: one tick claims due cron fires, bridges claimed fires
//! into started runs, and resumes due parked sessions.
//!
//! Every phase isolates per-item failures into [`TickReport::failures`] so a
//! single poisoned row can never stall the loop. The adapter tables are
//! hints; the kernel journal stays authoritative.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use finstack_ai_kernel::{
    ExternalEffectCompletionCommand, InteractionResolutionCommand, OperationLocator, SessionId,
    Timestamp, UNIX_EPOCH,
};
use finstack_ai_runtime::{
    Clock, ExternalClock, JournalStore, WorkflowDriverError, WorkflowSession, WorkflowWait,
    classify_wait,
};
use finstack_ai_workflow_local::{CronFire, CronSchedule, CronScheduleStore};

use crate::error::WorkerError;
use crate::fires::{FireRow, FireStatus, FireStore, idempotency_key};
use crate::inbox::{InboxKind, InboxRow, InboxStore};
use crate::park::park;
use crate::wake::{WakeIndexStore, WakeReason, WakeRow, lease_deadline};

/// Key identifying one buffered response: tenant, session, pending effect.
type InboxKey = (Arc<str>, SessionId, Arc<str>);

/// Binds host-owned ports onto a bare attached session.
pub trait PortsFactory: Send + Sync {
    /// Rebind model/tool/middleware ports for one workflow kind.
    ///
    /// # Errors
    ///
    /// Returns host-defined failures as [`WorkerError`].
    fn bind(&self, session: WorkflowSession) -> Result<WorkflowSession, WorkerError>;
}

/// Identity of a run started from a cron fire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartedRun {
    /// Canonical session id string of the accepted run.
    pub session_id: Arc<str>,
}

/// Starts one run for one claimed cron fire, idempotently.
pub trait RunStarter: Send + Sync {
    /// Start (or find, when the key was already used) the run for `fire`.
    fn start<'a>(
        &'a self,
        fire: &'a CronFire,
        idempotency_key: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<StartedRun, WorkerError>> + Send + 'a>>;
}

/// Per-tick outcome counters.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TickReport {
    /// Cron schedules claimed and recorded as fires this tick.
    pub cron_fires: usize,
    /// Claimed fires bridged into started runs this tick.
    pub runs_started: usize,
    /// Parked sessions successfully driven to their next wait.
    pub sessions_resumed: usize,
    /// Resumed sessions that parked again instead of terminating.
    pub sessions_reparked: usize,
    /// Per-item failures isolated during the tick.
    pub failures: usize,
}

/// Default worker identity used for leases.
const DEFAULT_WORKER_ID: &str = "worker-1";
/// Default lease time-to-live, in milliseconds.
const DEFAULT_LEASE_TTL_MS: u64 = 30_000;
/// Default per-session drive budget.
const DEFAULT_DRIVE_TIMEOUT: Duration = Duration::from_secs(2);
/// Base backoff applied after a failed resume, in milliseconds.
const BACKOFF_BASE_MS: u64 = 1_000;
/// Cap on the backoff exponent, bounding the retry delay.
const BACKOFF_MAX_SHIFT: u32 = 6;
/// Delay between state polls while a claimed row is being resumed.
const POLL_INTERVAL: Duration = Duration::from_millis(1);

/// Whether `wait` is still the wait recorded on `row`.
fn is_recorded_wait(wait: Option<&WorkflowWait>, row: &WakeRow) -> bool {
    let pending = match wait {
        Some(WorkflowWait::Interaction { interaction_id, .. }) => {
            interaction_id.to_canonical_string()
        }
        Some(
            WorkflowWait::Timer { effect_id, .. } | WorkflowWait::DeferredEffect { effect_id, .. },
        ) => effect_id.to_canonical_string(),
        Some(WorkflowWait::Terminal { .. }) | None => return false,
    };
    pending == row.pending_id.as_ref()
}

/// Builder for [`WorkflowWorker`].
pub struct WorkerBuilder {
    /// Partially configured worker, finished by [`WorkerBuilder::build`].
    worker: WorkflowWorker,
}

impl WorkerBuilder {
    /// Builder over the journal plus the four adapter tables.
    #[must_use]
    pub fn new(
        journal: Arc<dyn JournalStore>,
        cron: Arc<dyn CronScheduleStore>,
        wake: Arc<dyn WakeIndexStore>,
        fires: Arc<dyn FireStore>,
        inbox: Arc<dyn InboxStore>,
    ) -> Self {
        Self {
            worker: WorkflowWorker {
                journal,
                cron,
                wake,
                fires,
                inbox,
                ports: BTreeMap::new(),
                starters: BTreeMap::new(),
                clock: ExternalClock::new(UNIX_EPOCH),
                worker_id: Arc::from(DEFAULT_WORKER_ID),
                lease_ttl_ms: DEFAULT_LEASE_TTL_MS,
                drive_timeout: DEFAULT_DRIVE_TIMEOUT,
                seed_counter: AtomicU64::new(0),
            },
        }
    }

    /// Lease holder identity. Defaults to `"worker-1"`.
    #[must_use]
    pub fn worker_id(mut self, worker_id: &str) -> Self {
        self.worker.worker_id = Arc::from(worker_id);
        self
    }

    /// Lease time-to-live in milliseconds. Defaults to `30_000`.
    #[must_use]
    pub const fn lease_ttl_ms(mut self, ttl: u64) -> Self {
        self.worker.lease_ttl_ms = ttl;
        self
    }

    /// Per-session drive budget. Defaults to two seconds.
    #[must_use]
    pub const fn drive_timeout(mut self, timeout: Duration) -> Self {
        self.worker.drive_timeout = timeout;
        self
    }

    /// Injected clock. Defaults to a clock fixed at the unix epoch.
    #[must_use]
    pub fn clock(mut self, clock: ExternalClock) -> Self {
        self.worker.clock = clock;
        self
    }

    /// Register the ports factory used to resume one workflow kind.
    #[must_use]
    pub fn register_ports(mut self, kind: &str, factory: Arc<dyn PortsFactory>) -> Self {
        self.worker.ports.insert(Arc::from(kind), factory);
        self
    }

    /// Register the run starter used to bridge one schedule's fires.
    #[must_use]
    pub fn register_starter(mut self, schedule_id: &str, starter: Arc<dyn RunStarter>) -> Self {
        self.worker.starters.insert(Arc::from(schedule_id), starter);
        self
    }

    /// Finish the worker.
    #[must_use]
    pub fn build(self) -> WorkflowWorker {
        self.worker
    }
}

/// Leased worker over the local workflow driver.
pub struct WorkflowWorker {
    /// Authoritative kernel journal.
    journal: Arc<dyn JournalStore>,
    /// Adapter-owned cron schedule table.
    cron: Arc<dyn CronScheduleStore>,
    /// Adapter-owned wake index.
    wake: Arc<dyn WakeIndexStore>,
    /// Adapter-owned cron-fire records.
    fires: Arc<dyn FireStore>,
    /// Adapter-owned response inbox.
    inbox: Arc<dyn InboxStore>,
    /// Ports factories keyed by workflow kind.
    ports: BTreeMap<Arc<str>, Arc<dyn PortsFactory>>,
    /// Run starters keyed by schedule id.
    starters: BTreeMap<Arc<str>, Arc<dyn RunStarter>>,
    /// Injected clock; the single source of tick time.
    clock: ExternalClock,
    /// Lease holder identity.
    worker_id: Arc<str>,
    /// Lease time-to-live in milliseconds.
    lease_ttl_ms: u64,
    /// Per-session drive budget.
    drive_timeout: Duration,
    /// Monotonic random seed source for attached sessions.
    seed_counter: AtomicU64,
}

impl WorkflowWorker {
    /// Injected clock. Durable sleep advances only through this clock.
    #[must_use]
    pub const fn clock(&self) -> &ExternalClock {
        &self.clock
    }

    /// Run one tick: claim due cron fires, bridge them into runs, then
    /// resume every due parked session.
    ///
    /// # Errors
    ///
    /// Returns [`WorkerError::TimeOverflow`] when the clock leaves the
    /// representable range, and adapter-table load failures. Per-item
    /// failures are counted in [`TickReport::failures`] instead.
    pub async fn tick(&self) -> Result<TickReport, WorkerError> {
        let now = self.clock.now().map_err(|_| WorkerError::TimeOverflow)?;
        let mut report = TickReport::default();
        self.tick_cron(now, &mut report)?;
        self.tick_bridge(&mut report).await?;
        Box::pin(self.tick_wake(now, &mut report)).await?;
        Ok(report)
    }

    /// Phase 2: claim every due schedule and record its fire.
    fn tick_cron(&self, now: Timestamp, report: &mut TickReport) -> Result<(), WorkerError> {
        for schedule in self.cron.load_due(now)? {
            match self.claim_schedule(&schedule, now) {
                Ok(true) => report.cron_fires += 1,
                Ok(false) => {}
                Err(_) => report.failures += 1,
            }
        }
        Ok(())
    }

    /// Claim one due schedule, mirroring `LocalWorkflowDriver::fire_due`.
    fn claim_schedule(&self, schedule: &CronSchedule, now: Timestamp) -> Result<bool, WorkerError> {
        let expected_next = schedule.next_fire_at.as_unix_ms();
        let mut claimed = schedule.clone();
        claimed.last_fired_at = Some(now);
        claimed.fire_count = claimed.fire_count.saturating_add(1);
        claimed.next_fire_at = claimed.expression.next_after(claimed.origin, now)?;
        if !self.cron.try_claim(
            claimed.tenant_scope.as_ref(),
            claimed.schedule_id.as_ref(),
            expected_next,
            now,
            &claimed,
        )? {
            return Ok(false);
        }
        self.fires.record_claimed(&FireRow {
            tenant_scope: Arc::clone(&claimed.tenant_scope),
            schedule_id: Arc::clone(&claimed.schedule_id),
            fire_count: claimed.fire_count,
            fired_at: now,
            status: FireStatus::Claimed,
            started_session: None,
        })?;
        Ok(true)
    }

    /// Phase 3: bridge claimed-but-unstarted fires into started runs.
    ///
    /// A fire whose schedule has no registered starter is counted as a
    /// failure and left claimed — fail closed, visible, never dropped.
    async fn tick_bridge(&self, report: &mut TickReport) -> Result<(), WorkerError> {
        for row in self.fires.load_unstarted()? {
            let Some(starter) = self.starters.get(row.schedule_id.as_ref()) else {
                report.failures += 1;
                continue;
            };
            let fire = CronFire {
                tenant_scope: Arc::clone(&row.tenant_scope),
                schedule_id: Arc::clone(&row.schedule_id),
                fired_at: row.fired_at,
            };
            let key = idempotency_key(&row);
            let Ok(run) = starter.start(&fire, key.as_str()).await else {
                report.failures += 1;
                continue;
            };
            match self.fires.mark_started(
                row.tenant_scope.as_ref(),
                row.schedule_id.as_ref(),
                row.fire_count,
                run.session_id.as_ref(),
            ) {
                Ok(()) => report.runs_started += 1,
                Err(_) => report.failures += 1,
            }
        }
        Ok(())
    }

    /// Phase 4: claim and resume every due parked session.
    async fn tick_wake(&self, now: Timestamp, report: &mut TickReport) -> Result<(), WorkerError> {
        let due = self.wake.load_due(now)?;
        let inbox: BTreeMap<InboxKey, InboxRow> = self
            .inbox
            .load_all()?
            .into_iter()
            .map(|row| {
                (
                    (
                        Arc::clone(&row.tenant_scope),
                        row.session_id,
                        Arc::clone(&row.pending_id),
                    ),
                    row,
                )
            })
            .collect();
        for row in due {
            let key = (
                Arc::clone(&row.tenant_scope),
                row.session_id,
                Arc::clone(&row.pending_id),
            );
            let entry = inbox.get(&key);
            if row.reason != WakeReason::Timer && entry.is_none() {
                continue;
            }
            let claim = self.wake.try_claim(
                row.tenant_scope.as_ref(),
                row.session_id,
                self.worker_id.as_ref(),
                now,
                self.lease_ttl_ms,
            );
            let Ok(won) = claim else {
                report.failures += 1;
                continue;
            };
            if !won {
                continue;
            }
            if let Ok(cleared) = Box::pin(self.resume_row(&row, entry, now)).await {
                report.sessions_resumed += 1;
                if !cleared {
                    report.sessions_reparked += 1;
                }
            } else {
                report.failures += 1;
                // A store that cannot record the backoff keeps the stale
                // lease until it expires; the failure is already counted.
                drop(self.back_off(&row, now));
            }
        }
        Ok(())
    }

    /// Resume one claimed row. Returns `true` when the row was cleared —
    /// either the run reached a terminal state or it left the recorded wait
    /// without parking on a new one, which hands it back to the run loop.
    ///
    /// The recorded wait is journal-authoritative:
    /// [`WorkflowSession::drive_until_wait`] would classify it and return
    /// immediately, so the row must be driven *past* its wait instead. The
    /// owner respawn arms the pending timer; the poll below then waits for
    /// the state to leave that wait, bounded by the drive timeout.
    async fn resume_row(
        &self,
        row: &WakeRow,
        inbox_entry: Option<&InboxRow>,
        now: Timestamp,
    ) -> Result<bool, WorkerError> {
        let factory =
            self.ports
                .get(row.workflow_kind.as_ref())
                .ok_or_else(|| WorkerError::UnknownKind {
                    kind: Arc::clone(&row.workflow_kind),
                })?;
        let locator = OperationLocator::try_new(
            row.tenant_scope.as_ref(),
            row.session_id,
            row.lane_id,
            row.run_id,
        )
        .map_err(|_| WorkerError::StoreIntegrity {
            code: "wake_locator",
        })?;
        let seed = self.seed_counter.fetch_add(1, Ordering::AcqRel);
        let session =
            WorkflowSession::trusted(Arc::clone(&self.journal), locator, self.clock.clone(), seed)
                .await?;
        let mut session = factory
            .bind(session)?
            .with_drive_timeout(self.drive_timeout);
        if let Some(entry) = inbox_entry {
            Box::pin(self.submit_response(&session, entry, now)).await?;
        }
        session.respawn_owner().await?;
        if let Some(wait) = self.drive_past_wait(&mut session, row).await? {
            let terminal = matches!(wait, WorkflowWait::Terminal { .. });
            park(&mut session, self.wake.as_ref(), row.workflow_kind.as_ref())?;
            return Ok(terminal);
        }
        // The recorded wait resolved and the run is back inside the
        // model/tool loop, where its owner — not this worker — drives it.
        // The row is stale; drop it rather than re-firing it.
        self.wake
            .delete(row.tenant_scope.as_ref(), row.session_id)?;
        session.abort_owner();
        Ok(true)
    }

    /// Poll until the state leaves the wait recorded on `row`.
    ///
    /// Returns the next classified wait, or `None` when the run left its
    /// wait and is mid-flight in the model/tool loop.
    async fn drive_past_wait(
        &self,
        session: &mut WorkflowSession,
        row: &WakeRow,
    ) -> Result<Option<WorkflowWait>, WorkerError> {
        tokio::time::timeout(self.drive_timeout, async {
            loop {
                session.ensure_owner().await?;
                let wait = classify_wait(session.last_state());
                if !is_recorded_wait(wait.as_ref(), row) {
                    return Ok(wait);
                }
                tokio::time::sleep(POLL_INTERVAL).await;
            }
        })
        .await
        .map_err(|_| WorkerError::Driver(WorkflowDriverError::DriveTimeout))?
    }

    /// Submit one buffered response through the session ingress and drop the
    /// inbox row once it is accepted.
    async fn submit_response(
        &self,
        session: &WorkflowSession,
        entry: &InboxRow,
        now: Timestamp,
    ) -> Result<(), WorkerError> {
        match entry.kind {
            InboxKind::Interaction => {
                let command: InteractionResolutionCommand =
                    serde_json::from_slice(entry.payload.as_ref()).map_err(|_| {
                        WorkerError::StoreIntegrity {
                            code: "inbox_payload",
                        }
                    })?;
                session.resolve_interaction(command, now).await?;
            }
            InboxKind::External => {
                let command: ExternalEffectCompletionCommand =
                    serde_json::from_slice(entry.payload.as_ref()).map_err(|_| {
                        WorkerError::StoreIntegrity {
                            code: "inbox_payload",
                        }
                    })?;
                Box::pin(session.complete_external(command, now)).await?;
            }
        }
        self.inbox.delete(
            entry.tenant_scope.as_ref(),
            entry.session_id,
            entry.pending_id.as_ref(),
        )
    }

    /// Record a failed resume with exponential backoff.
    fn back_off(&self, row: &WakeRow, now: Timestamp) -> Result<(), WorkerError> {
        let shift = row.attempts.min(BACKOFF_MAX_SHIFT);
        let backoff_ms = BACKOFF_BASE_MS.saturating_mul(1_u64 << shift);
        let retry_at = lease_deadline(now, backoff_ms)?;
        self.wake
            .record_failure(row.tenant_scope.as_ref(), row.session_id, retry_at)
    }
}
