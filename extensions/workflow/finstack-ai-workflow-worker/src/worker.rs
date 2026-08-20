//! The leased worker: one tick claims due cron fires, bridges claimed fires
//! into started runs, and resumes due parked sessions.
//!
//! Every phase isolates per-item failures into [`TickReport::failures`] so a
//! single poisoned row can never stall the loop. The adapter tables are
//! hints; the kernel journal stays authoritative.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use finstack_ai_kernel::{
    ExternalEffectCompletionCommand, InteractionResolutionCommand, OperationLocator, SessionId,
    Timestamp, UNIX_EPOCH,
};
use finstack_ai_runtime::{
    Clock, ExternalClock, JournalStore, SystemClock, WorkflowDriverError, WorkflowSession,
    WorkflowWait, classify_wait,
};
use finstack_ai_workflow_local::{CronFire, CronSchedule, CronScheduleStore};
use serde::Serialize;

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
fn is_recorded_wait(wait: &WorkflowWait, row: &WakeRow) -> bool {
    let pending = match wait {
        WorkflowWait::Interaction { interaction_id, .. } => interaction_id.to_canonical_string(),
        WorkflowWait::Timer { effect_id, .. } | WorkflowWait::DeferredEffect { effect_id, .. } => {
            effect_id.to_canonical_string()
        }
        WorkflowWait::Terminal { .. } => return false,
    };
    pending == row.pending_id.as_ref()
}

/// Whether the journal's own timer for `wait` is still in the future.
///
/// The wake index is a hint: a row can be due before the committed timer is.
/// Re-parking on the authoritative wait is then both correct and cheap,
/// instead of polling out the whole drive budget for a timer that cannot
/// fire yet.
fn timer_is_early(wait: &WorkflowWait, now: Timestamp) -> bool {
    matches!(wait, WorkflowWait::Timer { due_at, .. } if *due_at > now)
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
                pump_clock: AtomicBool::new(false),
                start_backoff: Mutex::new(BTreeMap::new()),
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
    ///
    /// # Panics
    ///
    /// Debug builds assert the per-session drive budget is shorter than the
    /// lease TTL. A drive that can outlive its own lease lets a second worker
    /// claim the same session while the first is still driving it, which the
    /// journal's append CAS turns into wasted work and counted failures. The
    /// defaults satisfy this; overriding either knob must preserve it.
    #[must_use]
    pub fn build(self) -> WorkflowWorker {
        debug_assert!(
            self.worker.drive_timeout.as_millis() < u128::from(self.worker.lease_ttl_ms),
            "drive timeout must be shorter than the lease TTL",
        );
        self.worker
    }
}

/// Leased worker over the local workflow driver.
///
/// # Scope of a resume
///
/// Registering a [`PortsFactory`] is necessary to resume a run, but it is not
/// sufficient to carry every run to its next wait. This worker fires timers,
/// applies inbox responses to the journal, and re-parks (or completes) runs
/// whose next wait is reachable without a facade decision. A run whose next
/// step needs an externally submitted
/// `KernelInput::StageSettled { .. AfterModel | AfterToolBatch .., Continue }`
/// — the decision made by the application-level facade in the `finstack-ai`
/// agent layer, on which this crate does not depend — cannot be advanced
/// here. Such a run is left mid-flight in the stage loop: its response stays
/// in the inbox, its wake row survives, and the attempt is recorded as one
/// counted failure with backoff, preserved for a host that can drive it.
/// Hosts embedding a facade see those runs complete; hosts that do not see
/// them held safely rather than resumed.
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
    /// Whether the tick loop drives this worker from wall time.
    pump_clock: AtomicBool,
    /// Process-local start backoff per fire: attempts and next eligible time.
    start_backoff: Mutex<BTreeMap<String, (u32, Timestamp)>>,
}

impl WorkflowWorker {
    /// Injected clock. Durable sleep advances only through this clock.
    #[must_use]
    pub const fn clock(&self) -> &ExternalClock {
        &self.clock
    }

    /// Durably record one interaction response for a future tick.
    ///
    /// No live session is required: the response is buffered in the inbox
    /// and consumed the next time [`Self::tick`] resumes the parked session.
    ///
    /// # Errors
    ///
    /// Returns [`WorkerError::StoreIntegrity`] with code `"inbox_encode"`
    /// when the command cannot be serialized, and adapter-table failures
    /// from [`InboxStore::insert`].
    pub fn deliver_interaction(
        &self,
        command: &InteractionResolutionCommand,
        received_at: Timestamp,
    ) -> Result<(), WorkerError> {
        self.deliver(
            &command.locator,
            command.resolution.interaction_id().to_canonical_string(),
            InboxKind::Interaction,
            command,
            received_at,
        )
    }

    /// Durably record one external completion for a future tick.
    ///
    /// No live session is required: the response is buffered in the inbox
    /// and consumed the next time [`Self::tick`] resumes the parked session.
    ///
    /// # Errors
    ///
    /// Returns [`WorkerError::StoreIntegrity`] with code `"inbox_encode"`
    /// when the command cannot be serialized, and adapter-table failures
    /// from [`InboxStore::insert`].
    pub fn deliver_external(
        &self,
        command: &ExternalEffectCompletionCommand,
        received_at: Timestamp,
    ) -> Result<(), WorkerError> {
        self.deliver(
            &command.locator,
            command.completion.effect_id.to_canonical_string(),
            InboxKind::External,
            command,
            received_at,
        )
    }

    /// Shared encode-and-insert path for both `deliver_*` methods.
    fn deliver<T: Serialize>(
        &self,
        locator: &OperationLocator,
        pending_id: String,
        kind: InboxKind,
        command: &T,
        received_at: Timestamp,
    ) -> Result<(), WorkerError> {
        let payload = serde_json::to_vec(command).map_err(|_| WorkerError::StoreIntegrity {
            code: "inbox_encode",
        })?;
        self.inbox.insert(&InboxRow {
            tenant_scope: Arc::clone(&locator.tenant_scope),
            session_id: locator.session_id,
            pending_id: Arc::from(pending_id),
            kind,
            payload: Arc::from(payload.as_slice()),
            received_at,
        })
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
        Box::pin(self.tick_bridge(now, &mut report)).await?;
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
    ///
    /// The fire is recorded *before* the schedule CAS. The CAS is the durable,
    /// irreversible step: once it lands, `next_fire_at` has moved past this
    /// tick and nothing will ever offer the schedule again, so a fire recorded
    /// after it and lost to a store failure is lost for good. Recording first
    /// cannot double-run instead, because the record is keyed by
    /// `(tenant, schedule_id, fire_count)` and every worker racing for this
    /// tick derives the same `fire_count` from the same observed row: the
    /// key collides, `record_claimed` ignores the duplicate, and the bridge
    /// still starts exactly one run. Losing the CAS therefore leaves the
    /// winner's identical record in place, and crashing between the two
    /// leaves a fire that the next successful CAS reconciles to the same key.
    fn claim_schedule(&self, schedule: &CronSchedule, now: Timestamp) -> Result<bool, WorkerError> {
        let expected_next = schedule.next_fire_at.as_unix_ms();
        let mut claimed = schedule.clone();
        claimed.last_fired_at = Some(now);
        claimed.fire_count = claimed.fire_count.saturating_add(1);
        claimed.next_fire_at = claimed.expression.next_after(claimed.origin, now)?;
        self.fires.record_claimed(&FireRow {
            tenant_scope: Arc::clone(&claimed.tenant_scope),
            schedule_id: Arc::clone(&claimed.schedule_id),
            fire_count: claimed.fire_count,
            fired_at: now,
            status: FireStatus::Claimed,
            started_session: None,
        })?;
        if !self.cron.try_claim(
            claimed.tenant_scope.as_ref(),
            claimed.schedule_id.as_ref(),
            expected_next,
            now,
            &claimed,
        )? {
            return Ok(false);
        }
        Ok(true)
    }

    /// Phase 3: bridge claimed-but-unstarted fires into started runs.
    ///
    /// A fire whose schedule has no registered starter is counted as a
    /// failure and left claimed — fail closed, visible, never dropped.
    ///
    /// Two bounds protect the tick from the host code behind [`RunStarter`].
    /// Each call is capped by the same per-item budget a resume gets, so a
    /// starter that never returns cannot wedge the loop (and with it the
    /// shutdown signal, which is only observed between ticks). A call that
    /// fails or times out also backs the fire off, so a starter whose
    /// dependency is down is retried on a widening delay instead of once per
    /// poll interval. That backoff is process-local: the fires table records
    /// no attempt state, so a restart deliberately retries immediately.
    async fn tick_bridge(
        &self,
        now: Timestamp,
        report: &mut TickReport,
    ) -> Result<(), WorkerError> {
        for row in self.fires.load_unstarted()? {
            let Some(starter) = self.starters.get(row.schedule_id.as_ref()) else {
                report.failures += 1;
                continue;
            };
            let key = idempotency_key(&row);
            if self.start_deferred(key.as_str(), now) {
                continue;
            }
            let fire = CronFire {
                tenant_scope: Arc::clone(&row.tenant_scope),
                schedule_id: Arc::clone(&row.schedule_id),
                fired_at: row.fired_at,
            };
            let outcome =
                tokio::time::timeout(self.drive_timeout, starter.start(&fire, key.as_str())).await;
            let Ok(Ok(run)) = outcome else {
                report.failures += 1;
                self.defer_start(key, now);
                continue;
            };
            self.clear_start_backoff(key.as_str());
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
            // Both the lease this takes out and the backoff written below are
            // deadlines measured from the moment they are written, so each
            // row reads the clock afresh rather than reusing the tick's.
            let claim_now = self.row_now(now);
            let claim = self.wake.try_claim(
                row.tenant_scope.as_ref(),
                row.session_id,
                self.worker_id.as_ref(),
                claim_now,
                self.lease_ttl_ms,
            );
            let Ok(won) = claim else {
                report.failures += 1;
                continue;
            };
            if !won {
                continue;
            }
            if let Ok(terminal) = Box::pin(self.resume_row(&row, entry, claim_now)).await {
                report.sessions_resumed += 1;
                if !terminal {
                    report.sessions_reparked += 1;
                }
            } else {
                report.failures += 1;
                // A store that cannot record the backoff keeps the stale
                // lease until it expires; the failure is already counted.
                drop(self.back_off(&row, self.row_now(now)));
            }
        }
        Ok(())
    }

    /// Resume one claimed row. Returns `true` when the run reached a terminal
    /// state, which deletes its wake row.
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
        if let Some(entry) = inbox_entry
            && let Err(error) = Box::pin(self.submit_response(&session, entry, now)).await
        {
            // A payload whose command locator does not match this session
            // (TM-19, `require_locator`) can never resolve on any future
            // attempt either: it is permanently poisoned, not merely
            // transient. Unlike the drive-timeout case below — where the
            // entry survives for redelivery once the run can actually be
            // parked — this entry is deleted so it cannot wedge every
            // future tick claiming the same row. The wake row itself is
            // untouched: the caller's `back_off` still records the failure
            // and the run stays parked on its original wait.
            if matches!(
                error,
                WorkerError::Driver(WorkflowDriverError::UnknownLocator)
            ) {
                self.inbox.delete(
                    entry.tenant_scope.as_ref(),
                    entry.session_id,
                    entry.pending_id.as_ref(),
                )?;
            }
            return Err(error);
        }
        session.respawn_owner().await?;
        let wait = self.drive_past_wait(&mut session, row, now).await?;
        let terminal = matches!(wait, WorkflowWait::Terminal { .. });
        park(&mut session, self.wake.as_ref(), row.workflow_kind.as_ref())?;
        // Only now is the response fully consumed. Dropping it earlier would
        // strand the row: a non-timer row is claimed only while its inbox
        // entry exists, so a resume that failed after the delete could never
        // be retried.
        //
        // The spec words the park and this delete as "the same transaction".
        // They are two calls against two stores, so what we actually provide
        // is at-least-once: a crash between them replays the entry on the
        // next tick. That is safe because settling the journal side is
        // idempotent — the pending effect is already settled, so the replayed
        // resume finds nothing to apply and simply re-parks on the same wait.
        // Ordering matters more than atomicity here, and the order above is
        // the one that cannot lose work.
        if let Some(entry) = inbox_entry {
            self.inbox.delete(
                entry.tenant_scope.as_ref(),
                entry.session_id,
                entry.pending_id.as_ref(),
            )?;
        }
        Ok(terminal)
    }

    /// Poll until the state parks on a wait other than the one recorded on
    /// `row`, and return it.
    ///
    /// A run that has left its recorded wait but classifies no new one is
    /// mid-flight in the model/tool loop, where only its owner can drive it.
    /// The worker cannot park that, and dropping the row would orphan the
    /// run, so the budget simply runs out: the resulting
    /// [`WorkflowDriverError::DriveTimeout`] routes through the caller's
    /// backoff, which keeps the row for a later tick.
    ///
    /// # Errors
    ///
    /// Returns [`WorkflowDriverError::DriveTimeout`] when no new wait is
    /// classified within the drive budget, plus recover/spawn failures.
    async fn drive_past_wait(
        &self,
        session: &mut WorkflowSession,
        row: &WakeRow,
        now: Timestamp,
    ) -> Result<WorkflowWait, WorkerError> {
        tokio::time::timeout(self.drive_timeout, async {
            loop {
                session.ensure_owner().await?;
                if let Some(wait) = classify_wait(session.last_state())
                    && (!is_recorded_wait(&wait, row) || timer_is_early(&wait, now))
                {
                    return Ok(wait);
                }
                tokio::time::sleep(POLL_INTERVAL).await;
            }
        })
        .await
        .map_err(|_| WorkerError::Driver(WorkflowDriverError::DriveTimeout))?
    }

    /// Submit one buffered response through the session ingress.
    ///
    /// The inbox row survives this call; [`Self::resume_row`] deletes it only
    /// once the resume it unblocked has parked. Re-submitting an already
    /// settled response is safe: the ingress answers a duplicate with
    /// `ExternalRouteOutcome::Idempotent`, and a conflicting one with a
    /// durable `Rejected` — neither is an error, so redelivery cannot push
    /// the row into permanent backoff.
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
        Ok(())
    }

    /// Record a failed resume with exponential backoff.
    fn back_off(&self, row: &WakeRow, now: Timestamp) -> Result<(), WorkerError> {
        let shift = row.attempts.min(BACKOFF_MAX_SHIFT);
        let backoff_ms = BACKOFF_BASE_MS.saturating_mul(1_u64 << shift);
        let retry_at = lease_deadline(now, backoff_ms)?;
        self.wake
            .record_failure(row.tenant_scope.as_ref(), row.session_id, retry_at)
    }

    /// Whether this fire's starter is still inside its backoff window.
    fn start_deferred(&self, key: &str, now: Timestamp) -> bool {
        let Ok(backoff) = self.start_backoff.lock() else {
            return false;
        };
        backoff
            .get(key)
            .is_some_and(|(_, retry_at)| *retry_at > now)
    }

    /// Widen this fire's starter backoff after a failed or timed-out start.
    fn defer_start(&self, key: String, now: Timestamp) {
        let Ok(mut backoff) = self.start_backoff.lock() else {
            return;
        };
        let attempts = backoff.get(&key).map_or(0, |(attempts, _)| *attempts);
        let shift = attempts.min(BACKOFF_MAX_SHIFT);
        let backoff_ms = BACKOFF_BASE_MS.saturating_mul(1_u64 << shift);
        let Ok(retry_at) = lease_deadline(now, backoff_ms) else {
            return;
        };
        backoff.insert(key, (attempts.saturating_add(1), retry_at));
    }

    /// Drop this fire's starter backoff after a successful start.
    fn clear_start_backoff(&self, key: &str) {
        if let Ok(mut backoff) = self.start_backoff.lock() {
            backoff.remove(key);
        }
    }

    /// Current instant for one row's work.
    ///
    /// A tick that drives many rows can span far more wall time than its own
    /// budget, and every lease deadline and backoff it writes must be
    /// measured from when it is written, not from when the tick began. When
    /// [`Self::spawn`] drives the loop the wall clock is the source of truth,
    /// so re-read it (and pump the injected clock with it, so sessions
    /// attached later in the tick see the same instant). A worker whose
    /// `tick` is called directly keeps its injected clock authoritative and
    /// deterministic: the tick-start value is returned unchanged.
    fn row_now(&self, tick_now: Timestamp) -> Timestamp {
        if !self.pump_clock.load(Ordering::Acquire) {
            return tick_now;
        }
        let Ok(now) = SystemClock.now() else {
            return tick_now;
        };
        self.clock.set(now);
        now
    }

    /// Run the tick loop every `poll_interval`, pumping the clock from
    /// [`SystemClock`] before each tick.
    ///
    /// Each iteration reads the wall clock and feeds it to [`Self::clock`];
    /// a clock read failure skips that tick rather than aborting the loop.
    /// [`Self::tick`] already isolates per-item failures, so a tick error is
    /// likewise swallowed: the loop itself never panics or exits early.
    #[must_use]
    pub fn spawn(self: Arc<Self>, poll_interval: Duration) -> WorkerHandle {
        let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);
        self.pump_clock.store(true, Ordering::Release);
        let join = tokio::spawn(async move {
            let mut interval = tokio::time::interval(poll_interval);
            // A tick that overruns the interval must not be chased by a burst
            // of immediate catch-up ticks: the point of the interval is to
            // pace queries against a database the journal is also writing,
            // and the overrun is exactly when that pacing matters most.
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        if let Ok(now) = SystemClock.now() {
                            self.clock.set(now);
                            let _ = self.tick().await;
                        }
                    }
                    _ = shutdown_rx.changed() => break,
                }
            }
        });
        WorkerHandle {
            shutdown: shutdown_tx,
            join,
        }
    }
}

/// Handle to a spawned worker loop.
pub struct WorkerHandle {
    /// Signals the loop to stop after its current tick.
    shutdown: tokio::sync::watch::Sender<bool>,
    /// Join handle for the spawned loop task.
    join: tokio::task::JoinHandle<()>,
}

impl WorkerHandle {
    /// Signal shutdown and wait for the loop to finish the current tick.
    pub async fn shutdown(self) {
        let _ = self.shutdown.send(true);
        let _ = self.join.await;
    }
}
