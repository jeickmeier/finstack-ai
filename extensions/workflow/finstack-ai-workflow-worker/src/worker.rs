//! The leased worker: one tick optionally claims due cron fires and bridges
//! them into started runs, then resumes due parked sessions.
//!
//! Every phase isolates per-item failures into [`TickReport::failures`] so a
//! single poisoned row can never stall the loop. The adapter tables are
//! hints; the kernel journal stays authoritative.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use finstack_ai_kernel::{
    ExternalEffectCompletionCommand, InteractionRequest, InteractionResolutionCommand,
    OperationLocator, RunSecurityContext, SessionId, Timestamp, UNIX_EPOCH,
};
use finstack_ai_runtime::commit::CommitCoordinatorError;
use finstack_ai_runtime::ids::{Clock, ExternalClock, SystemClock};
use finstack_ai_runtime::ingress::{ExternalRouteError, ExternalRouteOutcome};
use finstack_ai_runtime::ports::journal::JournalStore;
use finstack_ai_runtime::workflow::{
    WorkflowCheckpoint, WorkflowDriverError, WorkflowSession, WorkflowWait, classify_wait,
};
use finstack_ai_workflow_local::{CronFire, CronScheduleStore};
use serde::Serialize;

use crate::cron_bridge::CronBridge;
use crate::error::WorkerError;
use crate::fires::FireStore;
use crate::inbox::{InboxInsertOutcome, InboxKind, InboxRow, InboxStore};
use crate::park::index_checkpoint;
use crate::wake::{WakeIndexStore, WakeLease, WakeReason, WakeRow, lease_deadline};

/// Binds host-owned ports onto a bare attached session.
pub trait PortsFactory: Send + Sync {
    /// Rebind model/tool/middleware ports for one workflow kind.
    ///
    /// # Errors
    ///
    /// Returns host-defined failures as [`WorkerError`].
    fn bind(&self, session: WorkflowSession) -> Result<WorkflowSession, WorkerError>;
}

/// Host-owned stage advancement for an already accepted workflow.
///
/// The worker owns leases, inbox settlement and parking. Implementations own
/// application stage decisions and return at the next durable wait. They must
/// not accept another run or append the original input on recovery.
pub trait WorkflowExecution: Send + Sync {
    /// Validate asynchronous reconstruction dependencies before any owner
    /// is spawned or external effect can be dispatched.
    fn prepare<'a>(
        &'a self,
        _session: &'a WorkflowSession,
    ) -> Pin<Box<dyn Future<Output = Result<(), WorkerError>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }

    /// Drive bound ports to a durable wait or terminal state.
    ///
    /// # Errors
    /// Returns reconstruction, configuration or execution failures explicitly.
    fn advance<'a>(
        &'a self,
        session: &'a mut WorkflowSession,
    ) -> Pin<Box<dyn Future<Output = Result<WorkflowWait, WorkerError>> + Send + 'a>>;
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
    /// Cron schedules claimed and recorded as fires this tick. Always zero
    /// unless the host opted into cron through [`WorkerBuilder::cron`].
    pub cron_fires: usize,
    /// Claimed fires bridged into started runs this tick. Always zero unless
    /// the host opted into cron through [`WorkerBuilder::cron`].
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
    ///
    /// A due row whose `workflow_kind` has no registered [`PortsFactory`] is
    /// *not* counted here: it is skipped unclaimed so another worker that
    /// serves that kind can resume it, and nothing about it is written.
    pub failures: usize,
}

/// Outcome of one iteration of the loop spawned by [`WorkflowWorker::spawn`].
///
/// Published through [`WorkerHandle::last_tick`] after every tick so an
/// embedding host can observe liveness and failure codes without a logging
/// dependency.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TickOutcome {
    /// Counters of the tick, or the default when the tick itself failed.
    pub report: TickReport,
    /// Stable code of a tick-level failure (adapter load errors, clock
    /// overflow); `None` when the tick completed. Per-item failures are
    /// counted in [`TickReport::failures`] instead.
    pub error_code: Option<&'static str>,
    /// Wall-clock instant the tick started at.
    pub at: Timestamp,
}

/// Default worker identity used for leases.
const DEFAULT_WORKER_ID: &str = "worker-1";
/// Default lease time-to-live, in milliseconds.
const DEFAULT_LEASE_TTL_MS: u64 = 30_000;
/// Default per-session drive budget.
const DEFAULT_DRIVE_TIMEOUT: Duration = Duration::from_secs(2);
/// Default upper bound for each adapter scan in one tick phase.
const DEFAULT_BATCH_LIMIT: usize = 64;
/// Default number of transient submission failures a buffered response is
/// allowed before it is dead-lettered.
const DEFAULT_MAX_RESPONSE_ATTEMPTS: u32 = 8;
/// Base backoff applied after a failed resume, in milliseconds.
const BACKOFF_BASE_MS: u64 = 1_000;
/// Cap on the backoff exponent, bounding the retry delay.
const BACKOFF_MAX_SHIFT: u32 = 6;
/// Delay between state polls while a claimed row is being resumed.
const POLL_INTERVAL: Duration = Duration::from_millis(1);

enum ResumeOutcome {
    Terminal,
    Reparked,
    Rejected,
}

/// Exponential retry delay after `attempts` failures, capped at
/// `BACKOFF_BASE_MS << BACKOFF_MAX_SHIFT`.
pub(crate) fn backoff_ms(attempts: u32) -> u64 {
    BACKOFF_BASE_MS.saturating_mul(1_u64 << attempts.min(BACKOFF_MAX_SHIFT))
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

/// Opt-in cron configuration held by the builder until `build`.
type CronConfig = (Arc<dyn CronScheduleStore>, Arc<dyn FireStore>, Duration);

/// Builder for [`WorkflowWorker`].
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use std::time::Duration;
///
/// use finstack_ai_runtime::ids::Clock;
/// use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};
/// use finstack_ai_workflow_local::MemoryCronStore;
/// use finstack_ai_workflow_worker::{
///     FireStore, InboxStore, MemoryWorkerStore, WakeIndexStore, WorkerBuilder,
/// };
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let journal = Arc::new(MemoryJournalStore::try_new(MemoryStoreLimits {
///     sessions: 1,
///     batches_per_session: 16,
///     records_per_session: 64,
///     snapshot_bytes: 4_096,
/// })?);
/// let store = Arc::new(MemoryWorkerStore::new());
///
/// // Wake index and inbox are always required; cron is opt-in.
/// let worker = WorkerBuilder::new(
///     journal,
///     Arc::clone(&store) as Arc<dyn WakeIndexStore>,
///     Arc::clone(&store) as Arc<dyn InboxStore>,
/// )
/// .cron(
///     Arc::new(MemoryCronStore::new()),
///     Arc::clone(&store) as Arc<dyn FireStore>,
///     Duration::from_hours(7 * 24),
/// )
/// .build()?;
/// assert!(worker.clock().now().is_ok());
/// # Ok(())
/// # }
/// ```
pub struct WorkerBuilder {
    /// Partially configured worker, finished by [`WorkerBuilder::build`].
    worker: WorkflowWorker,
    /// Opt-in cron configuration, turned into a [`CronBridge`] by `build`.
    cron: Option<CronConfig>,
    /// Run starters keyed by schedule id, handed to the cron bridge.
    starters: BTreeMap<Arc<str>, Arc<dyn RunStarter>>,
}

impl WorkerBuilder {
    /// Builder over the journal plus the wake index and response inbox.
    ///
    /// Cron scheduling is opt-in through [`WorkerBuilder::cron`].
    #[must_use]
    pub fn new(
        journal: Arc<dyn JournalStore>,
        wake: Arc<dyn WakeIndexStore>,
        inbox: Arc<dyn InboxStore>,
    ) -> Self {
        Self {
            worker: WorkflowWorker {
                journal,
                cron: None,
                wake,
                inbox,
                tenant_scope: None,
                ports: BTreeMap::new(),
                executions: BTreeMap::new(),
                interaction_lifecycle: None,
                clock: ExternalClock::new(UNIX_EPOCH),
                worker_id: Arc::from(DEFAULT_WORKER_ID),
                lease_ttl_ms: DEFAULT_LEASE_TTL_MS,
                drive_timeout: DEFAULT_DRIVE_TIMEOUT,
                batch_limit: DEFAULT_BATCH_LIMIT,
                max_response_attempts: DEFAULT_MAX_RESPONSE_ATTEMPTS,
                pump_clock: AtomicBool::new(false),
            },
            cron: None,
            starters: BTreeMap::new(),
        }
    }

    /// Opt into the cron phases: claim due schedules from `cron` as fires in
    /// `fires`, bridge them through registered [`RunStarter`]s, and purge
    /// started fires older than `fire_retention` in bounded batches after
    /// each bridge pass.
    #[must_use]
    pub fn cron(
        mut self,
        cron: Arc<dyn CronScheduleStore>,
        fires: Arc<dyn FireStore>,
        fire_retention: Duration,
    ) -> Self {
        self.cron = Some((cron, fires, fire_retention));
        self
    }

    /// Restrict wake processing to one application-authorized tenant.
    #[must_use]
    pub fn tenant_scope(mut self, tenant_scope: &str) -> Self {
        self.worker.tenant_scope = Some(Arc::from(tenant_scope));
        self
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

    /// Number of transient submission failures a buffered response survives
    /// before it is dead-lettered with the last failure's code. Defaults to
    /// `8`.
    ///
    /// The bound is measured against the wake row's `attempts` counter, which
    /// every failed resume of that row increments (a drive timeout after the
    /// response was applied included), so it is an upper bound on retries of
    /// the submission itself.
    #[must_use]
    pub const fn max_response_attempts(mut self, attempts: u32) -> Self {
        self.worker.max_response_attempts = attempts;
        self
    }

    /// Injected clock. Defaults to a clock fixed at the unix epoch.
    #[must_use]
    pub fn clock(mut self, clock: ExternalClock) -> Self {
        self.worker.clock = clock;
        self
    }

    /// Read wall time during direct ticks, including lease checks and timers.
    /// Omit this for deterministic hosts that own the injected clock.
    #[must_use]
    pub fn system_clock(self) -> Self {
        self.worker.pump_clock.store(true, Ordering::Release);
        self
    }

    /// Register the ports factory used to resume one workflow kind.
    #[must_use]
    pub fn register_ports(mut self, kind: &str, factory: Arc<dyn PortsFactory>) -> Self {
        self.worker.ports.insert(Arc::from(kind), factory);
        self
    }

    /// Register application stage advancement for one workflow kind.
    /// The matching ports factory must reject invalid recovery configuration
    /// before any owner is spawned.
    #[must_use]
    pub fn register_execution(mut self, kind: &str, execution: Arc<dyn WorkflowExecution>) -> Self {
        self.worker.executions.insert(Arc::from(kind), execution);
        self
    }

    /// Register the run starter used to bridge one schedule's fires.
    ///
    /// Requires [`WorkerBuilder::cron`]; `build` fails closed otherwise.
    #[must_use]
    pub fn register_starter(mut self, schedule_id: &str, starter: Arc<dyn RunStarter>) -> Self {
        self.starters.insert(Arc::from(schedule_id), starter);
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
    /// empty, a duration is zero, the drive budget can outlive its lease,
    /// the batch limit or response attempt bound is zero, the fire retention
    /// does not fit a millisecond timestamp, or a run starter was registered
    /// without [`WorkerBuilder::cron`] (`cron_not_configured`).
    pub fn build(mut self) -> Result<WorkflowWorker, WorkerError> {
        match self.cron.take() {
            Some((cron, fires, fire_retention)) => {
                if fire_retention.is_zero() {
                    return Err(WorkerError::InvalidConfiguration {
                        code: "fire_retention_zero",
                    });
                }
                if i64::try_from(fire_retention.as_millis()).is_err() {
                    return Err(WorkerError::InvalidConfiguration {
                        code: "fire_retention_overflow",
                    });
                }
                self.worker.cron = Some(CronBridge::new(
                    cron,
                    fires,
                    fire_retention,
                    std::mem::take(&mut self.starters),
                ));
            }
            None if !self.starters.is_empty() => {
                return Err(WorkerError::InvalidConfiguration {
                    code: "cron_not_configured",
                });
            }
            None => {}
        }
        if self.worker.max_response_attempts == 0 {
            return Err(WorkerError::InvalidConfiguration {
                code: "max_response_attempts_zero",
            });
        }
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
/// A [`PortsFactory`] reinstalls trusted ports. An optional
/// [`WorkflowExecution`] advances application stages using the host's driver.
/// Without it, the worker advances only runtime-owned transitions and preserves
/// inbox and wake hints when an application stage cannot be advanced.
pub struct WorkflowWorker {
    /// Authoritative kernel journal.
    journal: Arc<dyn JournalStore>,
    /// Opt-in cron phases; `None` when the host never called
    /// [`WorkerBuilder::cron`].
    cron: Option<CronBridge>,
    /// Adapter-owned wake index.
    wake: Arc<dyn WakeIndexStore>,
    /// Adapter-owned response inbox.
    inbox: Arc<dyn InboxStore>,
    /// Optional application-authorized tenant restriction.
    tenant_scope: Option<Arc<str>>,
    /// Ports factories keyed by workflow kind.
    ports: BTreeMap<Arc<str>, Arc<dyn PortsFactory>>,
    /// Host-owned stage advancement keyed by workflow kind.
    executions: BTreeMap<Arc<str>, Arc<dyn WorkflowExecution>>,
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
    /// Transient submission failures allowed before a response is dead-lettered.
    max_response_attempts: u32,
    /// Whether the tick loop drives this worker from wall time.
    pump_clock: AtomicBool,
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

    /// Run one tick: when cron is configured, claim due cron fires, bridge
    /// them into runs and purge started fires past retention; then resume
    /// every due parked session.
    ///
    /// # Errors
    ///
    /// Returns [`WorkerError::TimeOverflow`] when the clock leaves the
    /// representable range, and adapter-table load failures. Per-item
    /// failures are counted in [`TickReport::failures`] instead.
    pub async fn tick(&self) -> Result<TickReport, WorkerError> {
        let now = self.clock.now().map_err(|_| WorkerError::TimeOverflow)?;
        let mut report = TickReport::default();
        if let Some(cron) = &self.cron {
            Box::pin(cron.tick(now, self.batch_limit, self.drive_timeout, &mut report)).await?;
        }
        Box::pin(self.tick_wake(now, &mut report)).await?;
        Ok(report)
    }

    /// Claim and resume every due parked session.
    ///
    /// The store already applies the dueness predicate (clock for timers and
    /// runnable rows; backoff gate plus a buffered response or a passed
    /// deadline for inbox-driven rows), so the checks repeated here are cheap
    /// re-validations against races between the page load and the claim.
    async fn tick_wake(&self, now: Timestamp, report: &mut TickReport) -> Result<(), WorkerError> {
        let wake = Arc::clone(&self.wake);
        let limit = self.batch_limit;
        let tenant = self.tenant_scope.clone();
        let due = crate::blocking::run(move || wake.load_due_scoped(tenant.as_deref(), now, limit))
            .await?;
        for row in due {
            // `load_due_scoped` already filtered; a custom store that ignores
            // the scope must still fail closed at the tenant boundary.
            if self
                .tenant_scope
                .as_ref()
                .is_some_and(|tenant| *tenant != row.tenant_scope)
            {
                continue;
            }
            // A kind this worker cannot serve is left unclaimed and
            // unwritten so a worker that does serve it is not delayed by
            // our backoff.
            if !self.ports.contains_key(row.workflow_kind.as_ref()) {
                continue;
            }
            // A past-deadline interaction is due on the clock alone, exactly
            // like a timer: nobody is going to answer it, and the kernel's own
            // expiry can only fire once the session is attached. Interaction
            // rows without a deadline, and every other inbox-driven reason,
            // still wait for a buffered response.
            let expiring = expiry_due(&row, now);
            let inbox_driven = !matches!(row.reason, WakeReason::Timer | WakeReason::Runnable);
            if inbox_driven && !expiring {
                let Ok(entry) = self.load_inbox_entry(&row).await else {
                    report.failures += 1;
                    continue;
                };
                if entry.is_none() {
                    continue;
                }
            }
            // Both the lease this takes out and the backoff written below are
            // deadlines measured from the moment they are written, so each
            // row reads the clock afresh rather than reusing the tick's.
            let claim_now = self.row_now(now);
            let wake = Arc::clone(&self.wake);
            let key = row.clone();
            let worker_id = Arc::clone(&self.worker_id);
            let ttl = self.lease_ttl_ms;
            let claim = crate::blocking::run(move || {
                wake.try_claim(
                    key.tenant_scope.as_ref(),
                    key.session_id,
                    worker_id.as_ref(),
                    claim_now,
                    ttl,
                )
            })
            .await;
            let lease = match claim {
                Ok(Some(lease)) => lease,
                Ok(None) => continue,
                Err(_) => {
                    report.failures += 1;
                    continue;
                }
            };
            // The entry is re-read under the lease: another worker may have
            // consumed it between the page load and this claim, and an
            // inbox-driven row with nothing to apply must not be resumed.
            let Ok(entry) = self.load_inbox_entry(&row).await else {
                report.failures += 1;
                drop(self.release_lease(&lease).await);
                continue;
            };
            if inbox_driven && entry.is_none() && !expiring {
                drop(self.release_lease(&lease).await);
                continue;
            }
            let mut expired = false;
            let outcome =
                Box::pin(self.resume_row(&row, &lease, entry.as_ref(), claim_now, &mut expired))
                    .await;
            match outcome {
                Ok(resumed @ (ResumeOutcome::Terminal | ResumeOutcome::Reparked)) => {
                    if expired {
                        report.sessions_expired += 1;
                    }
                    report.sessions_resumed += 1;
                    if matches!(resumed, ResumeOutcome::Reparked) {
                        report.sessions_reparked += 1;
                    }
                }
                Ok(ResumeOutcome::Rejected) => {
                    report.responses_rejected += 1;
                    drop(self.release_lease(&lease).await);
                }
                Err(WorkerError::LeaseLost) => report.failures += 1,
                Err(_) => {
                    report.failures += 1;
                    // A store that cannot record the backoff keeps the stale
                    // lease unless the holder can explicitly release it.
                    if self
                        .back_off(&row, &lease, self.row_now(now))
                        .await
                        .is_err()
                    {
                        drop(self.release_lease(&lease).await);
                    }
                }
            }
        }
        Ok(())
    }

    /// Exact-key inbox lookup for one wake row.
    async fn load_inbox_entry(&self, row: &WakeRow) -> Result<Option<InboxRow>, WorkerError> {
        let inbox = Arc::clone(&self.inbox);
        let key = row.clone();
        crate::blocking::run(move || {
            inbox.load(
                key.tenant_scope.as_ref(),
                key.session_id,
                key.pending_id.as_ref(),
            )
        })
        .await
    }

    /// Submit one buffered response and dead-letter it when the runtime
    /// durably rejects it, when the failure is permanent, or when a transient
    /// failure has recurred `max_response_attempts` times (measured on the
    /// row's `attempts`). `None` means the response was applied and the
    /// resume continues; a transient failure under the bound is returned so
    /// the caller backs the row off and the entry is retried.
    async fn apply_inbox_entry(
        &self,
        session: &WorkflowSession,
        entry: &InboxRow,
        attempts: u32,
        now: Timestamp,
    ) -> Result<Option<ResumeOutcome>, WorkerError> {
        let reason_code = match Box::pin(self.submit_response(session, entry, now)).await {
            Ok(InteractionDeliveryOutcome::Accepted) => return Ok(None),
            Ok(InteractionDeliveryOutcome::Rejected { reason_code }) => reason_code,
            Err(error) => {
                let exhausted = attempts.saturating_add(1) >= self.max_response_attempts;
                let Some(reason_code) =
                    permanent_response_error(&error).or_else(|| exhausted.then(|| error.code()))
                else {
                    return Err(error);
                };
                if entry.kind == InboxKind::Interaction
                    && let Some(lifecycle) = &self.interaction_lifecycle
                {
                    settle_interaction(
                        Arc::clone(lifecycle),
                        entry,
                        InteractionDeliveryOutcome::Rejected { reason_code },
                        now,
                    )
                    .await?;
                }
                reason_code
            }
        };
        let inbox = Arc::clone(&self.inbox);
        let entry = entry.clone();
        crate::blocking::run(move || {
            inbox.dead_letter(
                entry.tenant_scope.as_ref(),
                entry.session_id,
                entry.pending_id.as_ref(),
                entry.payload_digest,
                reason_code,
                now,
            )
        })
        .await?;
        Ok(Some(ResumeOutcome::Rejected))
    }

    /// Resume one claimed row. A run that reaches a terminal state has its
    /// wake row deleted by the park.
    ///
    /// The recorded wait is journal-authoritative:
    /// [`WorkflowSession::drive_until_wait`] would classify it and return
    /// immediately, so the row must be driven *past* its wait instead. The
    /// owner respawn arms the pending timer; the poll below then waits for
    /// the state to leave that wait, bounded by the drive timeout.
    async fn resume_row(
        &self,
        row: &WakeRow,
        lease: &WakeLease,
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
        self.ensure_lease(lease, now).await?;
        if let Some(execution) = self.executions.get(row.workflow_kind.as_ref()) {
            tokio::time::timeout(self.drive_timeout, execution.prepare(&session))
                .await
                .map_err(|_| WorkerError::Driver(WorkflowDriverError::DriveTimeout))??;
        }
        self.ensure_lease(lease, self.row_now(now)).await?;
        if let Some(entry) = inbox_entry
            && let Some(outcome) = self
                .apply_inbox_entry(&session, entry, row.attempts, now)
                .await?
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
        self.ensure_lease(lease, self.row_now(now)).await?;
        session.respawn_owner().await?;
        if was_pending {
            // `respawn_owner` refreshes *before* it spawns, so the state it
            // leaves behind predates the expiry commit the spawn just made.
            session.ensure_owner().await?;
            *expired = !pending_matches_row(&session, row);
        }
        let driven = self.drive_leased(&mut session, row, lease, now).await;
        // Always join local work before handing control back, including lease
        // loss and timeout. Shutdown cannot undo already dispatched effects.
        let shutdown = session.shutdown_owner().await;
        let wait = driven?;
        shutdown?;
        self.ensure_lease(lease, self.row_now(now)).await?;
        let terminal = matches!(wait, WorkflowWait::Terminal { .. });
        let security = session
            .last_state()
            .accepted()
            .map(|accepted| accepted.security().clone());
        let checkpoint = session.persist_handoff()?;
        let wake = Arc::clone(&self.wake);
        let lifecycle = self.interaction_lifecycle.clone();
        let kind = Arc::clone(&row.workflow_kind);
        let claim = lease.clone();
        crate::blocking::run(move || {
            index_checkpoint(wake.as_ref(), &checkpoint, &wait, &kind, Some(&claim))?;
            if let (Some(lifecycle), Some(security), WorkflowWait::Interaction { request, .. }) =
                (lifecycle, security, &wait)
            {
                lifecycle.capture(&checkpoint, request, &security, now)?;
            }
            Ok(())
        })
        .await?;
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
            let inbox = Arc::clone(&self.inbox);
            let entry = entry.clone();
            let deleted = crate::blocking::run(move || {
                inbox.delete_if_digest(
                    entry.tenant_scope.as_ref(),
                    entry.session_id,
                    entry.pending_id.as_ref(),
                    entry.payload_digest,
                )
            })
            .await?;
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

    async fn drive_leased(
        &self,
        session: &mut WorkflowSession,
        row: &WakeRow,
        lease: &WakeLease,
        now: Timestamp,
    ) -> Result<WorkflowWait, WorkerError> {
        let drive = async {
            if let Some(execution) = self.executions.get(row.workflow_kind.as_ref()) {
                execution.advance(session).await
            } else {
                self.drive_past_wait(session, row, now).await
            }
        };
        let lease = async {
            // Renewal tracks the lease duration, independently of journal polling.
            let renewal_interval = Duration::from_millis((self.lease_ttl_ms / 3).max(1));
            loop {
                tokio::time::sleep(renewal_interval).await;
                self.ensure_lease(lease, self.row_now(now)).await?;
            }
        };
        tokio::select! {
            biased;
            result = lease => result,
            result = tokio::time::timeout(self.drive_timeout, drive) => {
                result.map_err(|_| WorkerError::Driver(WorkflowDriverError::DriveTimeout))?
            }
        }
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
    ) -> Result<InteractionDeliveryOutcome, WorkerError> {
        let outcome = match entry.kind {
            InboxKind::Interaction => {
                let command: InteractionResolutionCommand =
                    serde_json::from_slice(entry.payload.as_ref()).map_err(|_| {
                        WorkerError::StoreIntegrity {
                            code: "inbox_payload",
                        }
                    })?;
                session.resolve_interaction(command, now).await?
            }
            InboxKind::External => {
                let command: ExternalEffectCompletionCommand =
                    serde_json::from_slice(entry.payload.as_ref()).map_err(|_| {
                        WorkerError::StoreIntegrity {
                            code: "inbox_payload",
                        }
                    })?;
                Box::pin(session.complete_external(command, now)).await?
            }
        };
        let outcome = match outcome {
            ExternalRouteOutcome::Committed(_) | ExternalRouteOutcome::Idempotent { .. } => {
                InteractionDeliveryOutcome::Accepted
            }
            ExternalRouteOutcome::Rejected { reason_code, .. } => {
                InteractionDeliveryOutcome::Rejected { reason_code }
            }
        };
        if entry.kind == InboxKind::Interaction
            && let Some(lifecycle) = &self.interaction_lifecycle
        {
            settle_interaction(Arc::clone(lifecycle), entry, outcome, now).await?;
        }
        Ok(outcome)
    }

    /// Record a failed resume with exponential backoff.
    async fn back_off(
        &self,
        row: &WakeRow,
        lease: &WakeLease,
        now: Timestamp,
    ) -> Result<(), WorkerError> {
        let retry_at = lease_deadline(now, backoff_ms(row.attempts))?;
        let wake = Arc::clone(&self.wake);
        let lease = lease.clone();
        crate::blocking::run(move || wake.record_failure(&lease, retry_at)).await
    }

    /// Renew and verify ownership before journal-affecting resume work.
    async fn ensure_lease(&self, lease: &WakeLease, now: Timestamp) -> Result<(), WorkerError> {
        let wake = Arc::clone(&self.wake);
        let lease = lease.clone();
        let ttl = self.lease_ttl_ms;
        if crate::blocking::run(move || wake.renew(&lease, now, ttl)).await? {
            Ok(())
        } else {
            Err(WorkerError::LeaseLost)
        }
    }

    async fn release_lease(&self, lease: &WakeLease) -> Result<bool, WorkerError> {
        let wake = Arc::clone(&self.wake);
        let lease = lease.clone();
        crate::blocking::run(move || wake.release(&lease)).await
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
    /// likewise swallowed by the loop itself — it is published instead, as
    /// the [`TickOutcome`] readable from [`WorkerHandle::last_tick`] — and
    /// the loop never panics or exits early.
    #[must_use]
    pub fn spawn(self: Arc<Self>, poll_interval: Duration) -> WorkerHandle {
        let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);
        let (outcome_tx, outcome_rx) = tokio::sync::watch::channel(None);
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
                            let outcome = match self.tick().await {
                                Ok(report) => TickOutcome { report, error_code: None, at: now },
                                Err(error) => TickOutcome {
                                    report: TickReport::default(),
                                    error_code: Some(error.code()),
                                    at: now,
                                },
                            };
                            outcome_tx.send_replace(Some(outcome));
                        }
                    }
                    _ = shutdown_rx.changed() => break,
                }
            }
        });
        WorkerHandle {
            shutdown: shutdown_tx,
            last_tick: outcome_rx,
            join: Some(join),
        }
    }
}

/// Stable code of a response failure no retry can fix, or `None` when the
/// failure is transient and the response should be resubmitted after backoff.
///
/// Permanent: the command's locator does not match the session
/// ([`WorkflowDriverError::UnknownLocator`]), the router could not build a
/// normalized command ([`ExternalRouteError::InvalidNormalizedCommand`]),
/// the pure kernel decision rejected the input
/// ([`CommitCoordinatorError::Decision`]), or the stored payload does not
/// decode (`inbox_payload`). Everything else — a non-revealing ingress
/// rejection, id allocation, a store, boundary or event-delivery fault —
/// may clear on a later tick.
fn permanent_response_error(error: &WorkerError) -> Option<&'static str> {
    match error {
        WorkerError::Driver(
            WorkflowDriverError::UnknownLocator
            | WorkflowDriverError::Ingress(
                ExternalRouteError::InvalidNormalizedCommand
                | ExternalRouteError::Runtime(CommitCoordinatorError::Decision { .. }),
            ),
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
    /// Most recent tick outcome, `None` until the first tick completes.
    last_tick: tokio::sync::watch::Receiver<Option<TickOutcome>>,
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

    /// Watch the outcome of the most recent tick.
    ///
    /// The receiver holds `None` until the first tick completes and is
    /// replaced after every tick, including ticks that failed at the
    /// adapter-load level. Await `changed()` on it to follow the loop, or
    /// `borrow()` to sample liveness.
    #[must_use]
    pub fn last_tick(&self) -> tokio::sync::watch::Receiver<Option<TickOutcome>> {
        self.last_tick.clone()
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

async fn settle_interaction(
    lifecycle: Arc<dyn InteractionLifecycle>,
    entry: &InboxRow,
    outcome: InteractionDeliveryOutcome,
    now: Timestamp,
) -> Result<(), WorkerError> {
    let entry = entry.clone();
    crate::blocking::run(move || {
        lifecycle.settled(
            entry.tenant_scope.as_ref(),
            entry.pending_id.as_ref(),
            outcome,
            now,
        )
    })
    .await
}
