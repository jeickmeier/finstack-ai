//! The leased worker: one tick claims due cron fires, bridges claimed fires
//! into started runs, and resumes due parked sessions.
//!
//! Every phase isolates per-item failures into [`TickReport::failures`] so a
//! single poisoned row can never stall the loop. The adapter tables are
//! hints; the kernel journal stays authoritative.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use finstack_ai_kernel::{
    ExternalEffectCompletionCommand, InteractionRequest, InteractionResolutionCommand,
    OperationLocator, RunSecurityContext, SessionId, Timestamp, UNIX_EPOCH,
};
use finstack_ai_runtime::ids::{Clock, ExternalClock, SystemClock};
use finstack_ai_runtime::ingress::ExternalRouteOutcome;
use finstack_ai_runtime::ports::journal::JournalStore;
use finstack_ai_runtime::workflow::{
    WorkflowCheckpoint, WorkflowDriverError, WorkflowSession, WorkflowWait, classify_wait,
};
use finstack_ai_workflow_local::{CronFire, CronSchedule, CronScheduleStore};
use serde::Serialize;

use crate::error::WorkerError;
use crate::fires::{FireRow, FireStatus, FireStore, idempotency_key};
use crate::inbox::{InboxInsertOutcome, InboxKind, InboxRow, InboxStore};
use crate::park::park_for_wake;
use crate::wake::{WakeIndexStore, WakeReason, WakeRow, lease_deadline};

/// Binds host-owned ports onto a bare attached session.
pub trait PortsFactory: Send + Sync {
    /// Rebind model/tool/middleware ports for one workflow kind.
    ///
    /// # Errors
    ///
    /// Returns host-defined failures as [`WorkerError`].
    fn bind(&self, session: WorkflowSession) -> Result<WorkflowSession, WorkerError>;
}

/// Runtime-authoritative disposition of one buffered interaction command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InteractionDeliveryOutcome {
    /// The command was committed or replayed idempotently.
    Accepted,
    /// The interaction ingress durably rejected the command.
    Rejected {
        /// Stable non-secret rejection reason.
        reason_code: &'static str,
    },
}

/// Optional lifecycle bridge implemented by HITL adapters.
///
/// The worker owns journal execution but does not depend on the HITL crate.
/// This callback lets an adapter capture interactions created during worker
/// re-park and record the ingress's authoritative delivery outcome.
pub trait InteractionLifecycle: Send + Sync {
    /// Capture a newly parked interaction.
    ///
    /// # Errors
    ///
    /// Returns a stable worker error when the adapter cannot persist it.
    fn capture(
        &self,
        checkpoint: &WorkflowCheckpoint,
        request: &InteractionRequest,
        security: &RunSecurityContext,
        captured_at: Timestamp,
    ) -> Result<(), WorkerError>;

    /// Record the runtime ingress outcome for a buffered resolution.
    ///
    /// # Errors
    ///
    /// Returns a stable worker error when the adapter cannot persist it.
    fn settled(
        &self,
        tenant_scope: &str,
        interaction_id: &str,
        outcome: InteractionDeliveryOutcome,
        settled_at: Timestamp,
    ) -> Result<(), WorkerError>;
}

/// Identity of a run started from a cron fire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartedRun {
    /// Session id of the accepted run.
    pub session_id: SessionId,
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
    /// Parked interactions whose committed deadline had passed and which this
    /// tick drove the kernel into expiring.
    ///
    /// Counted only when the expiring resume completes. The runtime commits
    /// the expiry while the session attaches, so a resume that fails *after*
    /// that commit (a drive timeout, a store error while re-parking) reports
    /// only [`TickReport::failures`] for the tick — never an expiry and a
    /// failure for the same row at once. The retry re-parks the row without
    /// re-expiring, so such an expiry stays uncounted rather than
    /// double-signaled; the journal, not this counter, is the authority.
    ///
    /// A buffered response that arrives for an interaction whose deadline has
    /// already passed does *not* avoid this counter. The interaction ingress is
    /// fail-closed on a late answer and settles it `Expired` rather than
    /// `Granted`, so the expiry is real and is counted here as well as in
    /// [`TickReport::sessions_resumed`]. Only a response the ingress actually
    /// accepts — one submitted before the deadline — leaves this counter alone.
    pub sessions_expired: usize,
    /// Buffered responses durably rejected and moved to dead letters.
    pub responses_rejected: usize,
    /// Per-item failures isolated during the tick.
    pub failures: usize,
}

/// Default worker identity used for leases.
const DEFAULT_WORKER_ID: &str = "worker-1";
/// Default lease time-to-live, in milliseconds.
const DEFAULT_LEASE_TTL_MS: u64 = 30_000;
/// Default per-session drive budget.
const DEFAULT_DRIVE_TIMEOUT: Duration = Duration::from_secs(2);
/// Default upper bound for each adapter scan in one tick phase.
const DEFAULT_BATCH_LIMIT: usize = 64;
/// Base backoff applied after a failed resume, in milliseconds.
const BACKOFF_BASE_MS: u64 = 1_000;
/// Cap on the backoff exponent, bounding the retry delay.
const BACKOFF_MAX_SHIFT: u32 = 6;
/// Delay between state polls while a claimed row is being resumed.
const POLL_INTERVAL: Duration = Duration::from_millis(1);

#[derive(Clone, Copy)]
enum ResponseSubmission {
    Applied,
    Rejected { reason_code: &'static str },
}

enum ResumeOutcome {
    Terminal,
    Reparked,
    Rejected,
}

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

/// Whether `row` is a parked interaction whose committed deadline has passed.
///
/// The deadline is a *semantic* event, not an authorization: nobody has to
/// answer, so no principal and no evidence are involved. The runtime settles
/// it credential-free through
/// `InteractionResumeAction::ExpireIfDue`
/// (`crates/finstack-ai-runtime/src/services/interaction.rs`), applied by
/// `apply_interaction_resume`
/// (`crates/finstack-ai-runtime/src/exec/settlement/interaction.rs`) inside
/// every `RunTaskOwner` constructor. The worker therefore submits no input of
/// its own for expiry: attaching the session with a clock past the deadline
/// is the whole mechanism.
fn expiry_due(row: &WakeRow, now: Timestamp) -> bool {
    row.reason == WakeReason::Interaction && row.expires_at.is_some_and(|deadline| deadline <= now)
}

/// Whether the session's pending interaction is the one `row` was parked on.
fn pending_matches_row(session: &WorkflowSession, row: &WakeRow) -> bool {
    session
        .last_state()
        .pending_interaction()
        .is_some_and(|pending| {
            pending.request.interaction_id().to_canonical_string() == row.pending_id.as_ref()
        })
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
                interaction_lifecycle: None,
                clock: ExternalClock::new(UNIX_EPOCH),
                worker_id: Arc::from(DEFAULT_WORKER_ID),
                lease_ttl_ms: DEFAULT_LEASE_TTL_MS,
                drive_timeout: DEFAULT_DRIVE_TIMEOUT,
                batch_limit: DEFAULT_BATCH_LIMIT,
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

    /// Maximum rows loaded by each scheduler phase. Defaults to `64`.
    #[must_use]
    pub const fn batch_limit(mut self, limit: usize) -> Self {
        self.worker.batch_limit = limit;
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

    /// Register the optional HITL lifecycle bridge.
    #[must_use]
    pub fn interaction_lifecycle(mut self, lifecycle: Arc<dyn InteractionLifecycle>) -> Self {
        self.worker.interaction_lifecycle = Some(lifecycle);
        self
    }

    /// Finish the worker.
    ///
    /// # Errors
    ///
    /// Returns [`WorkerError::InvalidConfiguration`] when the worker id is
    /// empty, a duration is zero, the drive budget can outlive its lease, or
    /// the batch limit is zero.
    pub fn build(self) -> Result<WorkflowWorker, WorkerError> {
        if self.worker.worker_id.is_empty() {
            return Err(WorkerError::InvalidConfiguration {
                code: "worker_id_empty",
            });
        }
        if self.worker.lease_ttl_ms == 0 {
            return Err(WorkerError::InvalidConfiguration {
                code: "lease_ttl_zero",
            });
        }
        if self.worker.drive_timeout.is_zero() {
            return Err(WorkerError::InvalidConfiguration {
                code: "drive_timeout_zero",
            });
        }
        if self.worker.drive_timeout.as_millis() >= u128::from(self.worker.lease_ttl_ms) {
            return Err(WorkerError::InvalidConfiguration {
                code: "drive_timeout_exceeds_lease",
            });
        }
        if self.worker.batch_limit == 0 {
            return Err(WorkerError::InvalidConfiguration {
                code: "batch_limit_zero",
            });
        }
        Ok(self.worker)
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
    /// Optional adapter bridge for HITL capture and delivery outcomes.
    interaction_lifecycle: Option<Arc<dyn InteractionLifecycle>>,
    /// Injected clock; the single source of tick time.
    clock: ExternalClock,
    /// Lease holder identity.
    worker_id: Arc<str>,
    /// Lease time-to-live in milliseconds.
    lease_ttl_ms: u64,
    /// Per-session drive budget.
    drive_timeout: Duration,
    /// Maximum rows loaded by each adapter scan.
    batch_limit: usize,
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
    ) -> Result<InboxInsertOutcome, WorkerError> {
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
    ) -> Result<InboxInsertOutcome, WorkerError> {
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
    ) -> Result<InboxInsertOutcome, WorkerError> {
        let payload = serde_json::to_vec(command).map_err(|_| WorkerError::StoreIntegrity {
            code: "inbox_encode",
        })?;
        let row = InboxRow::try_new(
            Arc::clone(&locator.tenant_scope),
            locator.session_id,
            Arc::from(pending_id),
            kind,
            Arc::from(payload.into_boxed_slice()),
            received_at,
        )?;
        self.inbox.insert(&row)
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
        for schedule in self.cron.load_due(now, self.batch_limit)? {
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
        for row in self.fires.load_unstarted(self.batch_limit)? {
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
                self.defer_start(key.as_str().to_owned(), now);
                continue;
            };
            self.clear_start_backoff(key.as_str());
            match self.fires.mark_started(
                row.tenant_scope.as_ref(),
                row.schedule_id.as_ref(),
                row.fire_count,
                run.session_id,
            ) {
                Ok(_) => report.runs_started += 1,
                Err(_) => report.failures += 1,
            }
        }
        Ok(())
    }

    /// Phase 4: claim and resume every due parked session.
    async fn tick_wake(&self, now: Timestamp, report: &mut TickReport) -> Result<(), WorkerError> {
        let due = self.wake.load_due(now, self.batch_limit)?;
        for row in due {
            let Ok(entry) = self.inbox.load(
                row.tenant_scope.as_ref(),
                row.session_id,
                row.pending_id.as_ref(),
            ) else {
                report.failures += 1;
                continue;
            };
            // A past-deadline interaction is due on the clock alone, exactly
            // like a timer: nobody is going to answer it, and the kernel's own
            // expiry can only fire once the session is attached. Interaction
            // rows without a deadline, and every other inbox-driven reason,
            // still wait for a buffered response.
            let expiring = expiry_due(&row, now);
            if row.reason != WakeReason::Timer && entry.is_none() && !expiring {
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
            let mut expired = false;
            let outcome =
                Box::pin(self.resume_row(&row, entry.as_ref(), claim_now, &mut expired)).await;
            match outcome {
                Ok(ResumeOutcome::Terminal) => {
                    if expired {
                        report.sessions_expired += 1;
                    }
                    report.sessions_resumed += 1;
                }
                Ok(ResumeOutcome::Reparked) => {
                    if expired {
                        report.sessions_expired += 1;
                    }
                    report.sessions_resumed += 1;
                    report.sessions_reparked += 1;
                }
                Ok(ResumeOutcome::Rejected) => {
                    report.responses_rejected += 1;
                    drop(self.wake.release(
                        row.tenant_scope.as_ref(),
                        row.session_id,
                        self.worker_id.as_ref(),
                    ));
                }
                Err(WorkerError::LeaseLost) => report.failures += 1,
                Err(_) => {
                    report.failures += 1;
                    // A store that cannot record the backoff keeps the stale
                    // lease unless the holder can explicitly release it.
                    if self.back_off(&row, self.row_now(now)).is_err() {
                        drop(self.wake.release(
                            row.tenant_scope.as_ref(),
                            row.session_id,
                            self.worker_id.as_ref(),
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    async fn apply_inbox_entry(
        &self,
        session: &WorkflowSession,
        entry: &InboxRow,
        now: Timestamp,
    ) -> Result<Option<ResumeOutcome>, WorkerError> {
        match Box::pin(self.submit_response(session, entry, now)).await {
            Ok(ResponseSubmission::Applied) => Ok(None),
            Ok(ResponseSubmission::Rejected { reason_code }) => {
                self.inbox.dead_letter(
                    entry.tenant_scope.as_ref(),
                    entry.session_id,
                    entry.pending_id.as_ref(),
                    entry.payload_digest,
                    reason_code,
                    now,
                )?;
                Ok(Some(ResumeOutcome::Rejected))
            }
            Err(error) => {
                let Some(reason_code) = permanent_response_error(&error) else {
                    return Err(error);
                };
                if entry.kind == InboxKind::Interaction
                    && let Some(lifecycle) = &self.interaction_lifecycle
                {
                    lifecycle.settled(
                        entry.tenant_scope.as_ref(),
                        entry.pending_id.as_ref(),
                        InteractionDeliveryOutcome::Rejected { reason_code },
                        now,
                    )?;
                }
                self.inbox.dead_letter(
                    entry.tenant_scope.as_ref(),
                    entry.session_id,
                    entry.pending_id.as_ref(),
                    entry.payload_digest,
                    reason_code,
                    now,
                )?;
                Ok(Some(ResumeOutcome::Rejected))
            }
        }
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
        expired: &mut bool,
    ) -> Result<ResumeOutcome, WorkerError> {
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
        let session =
            WorkflowSession::trusted(Arc::clone(&self.journal), locator, self.clock.clone())
                .await?;
        let mut session = factory
            .bind(session)?
            .with_drive_timeout(self.drive_timeout);
        self.ensure_lease(row, now)?;
        if let Some(entry) = inbox_entry
            && let Some(outcome) = self.apply_inbox_entry(&session, entry, now).await?
        {
            return Ok(outcome);
        }
        // Only meaningful on the expiry path: whether the interaction this row
        // was parked on is *still* pending as the owner respawns. The respawn
        // is what applies `ExpireIfDue`, so comparing across it is what tells
        // an expiry this tick performed apart from one an earlier tick already
        // did — a re-tick over a row whose resume failed after the expiry must
        // not count it a second time.
        //
        // A row can be due on both counts at once: a resolution buffered before
        // the deadline, ticked after it. That still counts as an expiry, and
        // deliberately so — the resolution does not win. The interaction
        // ingress is fail-closed on a late answer: `interaction_settled_input`
        // (`crates/finstack-ai-runtime/src/driver/ingress/shared.rs`) rewrites
        // a resolution submitted at or after `expires_at` into
        // `InteractionSettled::Expired` before it ever reaches the reducer, so
        // the `submit_response` above commits the expiry itself and the
        // interaction is settled `Expired`, not `Granted`. Counting it is
        // therefore truthful: an expiry really was committed, by this tick, for
        // this row. Gating on `inbox_entry.is_none()` here would *under*-report
        // exactly that case.
        let was_pending = expiry_due(row, now) && pending_matches_row(&session, row);
        self.ensure_lease(row, self.row_now(now))?;
        session.respawn_owner().await?;
        if was_pending {
            // `respawn_owner` refreshes *before* it spawns, so the state it
            // leaves behind predates the expiry commit the spawn just made.
            session.ensure_owner().await?;
            *expired = !pending_matches_row(&session, row);
        }
        let wait = self.drive_past_wait(&mut session, row, now).await?;
        let terminal = matches!(wait, WorkflowWait::Terminal { .. });
        let security = session
            .last_state()
            .accepted()
            .map(|accepted| accepted.security().clone());
        let checkpoint =
            park_for_wake(&mut session, self.wake.as_ref(), row.workflow_kind.as_ref())?;
        if let (Some(lifecycle), Some(security), WorkflowWait::Interaction { request, .. }) =
            (&self.interaction_lifecycle, security, &wait)
        {
            lifecycle.capture(&checkpoint, request, &security, now)?;
        }
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
            let deleted = self.inbox.delete_if_digest(
                entry.tenant_scope.as_ref(),
                entry.session_id,
                entry.pending_id.as_ref(),
                entry.payload_digest,
            )?;
            if !deleted {
                return Err(WorkerError::Conflict {
                    code: "inbox_consumed_conflict",
                });
            }
        }
        Ok(if terminal {
            ResumeOutcome::Terminal
        } else {
            ResumeOutcome::Reparked
        })
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
    ) -> Result<ResponseSubmission, WorkerError> {
        match entry.kind {
            InboxKind::Interaction => {
                let command: InteractionResolutionCommand =
                    serde_json::from_slice(entry.payload.as_ref()).map_err(|_| {
                        WorkerError::StoreIntegrity {
                            code: "inbox_payload",
                        }
                    })?;
                let outcome = session.resolve_interaction(command, now).await?;
                let submission = match outcome {
                    ExternalRouteOutcome::Committed(_)
                    | ExternalRouteOutcome::Idempotent { .. } => ResponseSubmission::Applied,
                    ExternalRouteOutcome::Rejected { reason_code, .. } => {
                        ResponseSubmission::Rejected { reason_code }
                    }
                };
                if let Some(lifecycle) = &self.interaction_lifecycle {
                    let outcome = match submission {
                        ResponseSubmission::Applied => InteractionDeliveryOutcome::Accepted,
                        ResponseSubmission::Rejected { reason_code } => {
                            InteractionDeliveryOutcome::Rejected { reason_code }
                        }
                    };
                    lifecycle.settled(
                        entry.tenant_scope.as_ref(),
                        entry.pending_id.as_ref(),
                        outcome,
                        now,
                    )?;
                }
                return Ok(submission);
            }
            InboxKind::External => {
                let command: ExternalEffectCompletionCommand =
                    serde_json::from_slice(entry.payload.as_ref()).map_err(|_| {
                        WorkerError::StoreIntegrity {
                            code: "inbox_payload",
                        }
                    })?;
                let outcome = Box::pin(session.complete_external(command, now)).await?;
                if let ExternalRouteOutcome::Rejected { reason_code, .. } = outcome {
                    return Ok(ResponseSubmission::Rejected { reason_code });
                }
            }
        }
        Ok(ResponseSubmission::Applied)
    }

    /// Record a failed resume with exponential backoff.
    fn back_off(&self, row: &WakeRow, now: Timestamp) -> Result<(), WorkerError> {
        let shift = row.attempts.min(BACKOFF_MAX_SHIFT);
        let backoff_ms = BACKOFF_BASE_MS.saturating_mul(1_u64 << shift);
        let retry_at = lease_deadline(now, backoff_ms)?;
        self.wake
            .record_failure(row.tenant_scope.as_ref(), row.session_id, retry_at)
    }

    /// Renew and verify ownership before journal-affecting resume work.
    fn ensure_lease(&self, row: &WakeRow, now: Timestamp) -> Result<(), WorkerError> {
        if self.wake.renew(
            row.tenant_scope.as_ref(),
            row.session_id,
            self.worker_id.as_ref(),
            now,
            self.lease_ttl_ms,
        )? {
            Ok(())
        } else {
            Err(WorkerError::LeaseLost)
        }
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
            join: Some(join),
        }
    }
}

fn permanent_response_error(error: &WorkerError) -> Option<&'static str> {
    match error {
        WorkerError::Driver(
            WorkflowDriverError::UnknownLocator | WorkflowDriverError::Ingress(_),
        )
        | WorkerError::StoreIntegrity {
            code: "inbox_payload",
        } => Some(error.code()),
        _ => None,
    }
}

/// Handle to a spawned worker loop.
pub struct WorkerHandle {
    /// Signals the loop to stop after its current tick.
    shutdown: tokio::sync::watch::Sender<bool>,
    /// Join handle for the spawned loop task.
    join: Option<tokio::task::JoinHandle<()>>,
}

impl WorkerHandle {
    /// Signal shutdown and wait for the loop to finish the current tick.
    pub async fn shutdown(mut self) {
        let _ = self.shutdown.send(true);
        if let Some(join) = self.join.take() {
            let _ = join.await;
        }
    }
}

impl Drop for WorkerHandle {
    fn drop(&mut self) {
        let _ = self.shutdown.send(true);
        if let Some(join) = self.join.take() {
            join.abort();
        }
    }
}
