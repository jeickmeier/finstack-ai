//! Optional cron phases of a tick: claim due schedules as fires, bridge
//! claimed fires into started runs, and purge started fires past retention.
//!
//! The bridge exists only when the host opts in through
//! [`WorkerBuilder::cron`](crate::WorkerBuilder::cron). A worker without it
//! never touches a cron or fire table and reports zero for
//! [`TickReport::cron_fires`](crate::TickReport::cron_fires) and
//! [`TickReport::runs_started`](crate::TickReport::runs_started).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use finstack_ai_kernel::Timestamp;
use finstack_ai_workflow_local::{CronFire, CronSchedule, CronScheduleStore};

use crate::error::WorkerError;
use crate::fires::{FireRow, FireStatus, FireStore, idempotency_key};
use crate::wake::lease_deadline;
use crate::worker::{RunStarter, TickReport, backoff_ms};

/// Cron schedule claiming and fire bridging, owned by the worker when the
/// host configures it.
pub(crate) struct CronBridge {
    /// Adapter-owned cron schedule table.
    cron: Arc<dyn CronScheduleStore>,
    /// Adapter-owned cron-fire records.
    fires: Arc<dyn FireStore>,
    /// How long started fires are retained before the tick purges them.
    fire_retention: Duration,
    /// Run starters keyed by schedule id.
    starters: BTreeMap<Arc<str>, Arc<dyn RunStarter>>,
    /// Process-local start backoff per fire: attempts and next eligible time.
    start_backoff: Mutex<BTreeMap<String, (u32, Timestamp)>>,
}

impl CronBridge {
    pub(crate) fn new(
        cron: Arc<dyn CronScheduleStore>,
        fires: Arc<dyn FireStore>,
        fire_retention: Duration,
        starters: BTreeMap<Arc<str>, Arc<dyn RunStarter>>,
    ) -> Self {
        Self {
            cron,
            fires,
            fire_retention,
            starters,
            start_backoff: Mutex::new(BTreeMap::new()),
        }
    }

    /// Run the cron phases of one tick: claim, bridge, then purge.
    pub(crate) async fn tick(
        &self,
        now: Timestamp,
        batch_limit: usize,
        drive_timeout: Duration,
        report: &mut TickReport,
    ) -> Result<(), WorkerError> {
        self.tick_cron(now, batch_limit, report).await?;
        Box::pin(self.tick_bridge(now, batch_limit, drive_timeout, report)).await?;
        self.purge_started(now, batch_limit, report).await
    }

    /// Record a bounded batch of claimed fires before advancing their schedules.
    async fn tick_cron(
        &self,
        now: Timestamp,
        limit: usize,
        report: &mut TickReport,
    ) -> Result<(), WorkerError> {
        let cron = Arc::clone(&self.cron);
        let fires = Arc::clone(&self.fires);
        let (claimed, failures) = crate::blocking::run(move || {
            let mut claimed = 0;
            let mut failures = 0;
            for schedule in cron.load_due(now, limit)? {
                match claim_schedule(cron.as_ref(), fires.as_ref(), &schedule, now) {
                    Ok(true) => claimed += 1,
                    Ok(false) => {}
                    Err(_) => failures += 1,
                }
            }
            Ok((claimed, failures))
        })
        .await?;
        report.cron_fires += claimed;
        report.failures += failures;
        Ok(())
    }

    /// Bridge claimed-but-unstarted fires into started runs.
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
    ///
    /// [`FireStore::load_unstarted`] pages on a rotating cursor, so one fire
    /// that can never start cannot starve the fires behind it.
    async fn tick_bridge(
        &self,
        now: Timestamp,
        limit: usize,
        drive_timeout: Duration,
        report: &mut TickReport,
    ) -> Result<(), WorkerError> {
        let fires = Arc::clone(&self.fires);
        let rows = crate::blocking::run(move || fires.load_unstarted(limit)).await?;
        for row in rows {
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
                tokio::time::timeout(drive_timeout, starter.start(&fire, key.as_str())).await;
            let Ok(Ok(run)) = outcome else {
                report.failures += 1;
                self.defer_start(key.as_str().to_owned(), now);
                continue;
            };
            self.clear_start_backoff(key.as_str());
            let fires = Arc::clone(&self.fires);
            match crate::blocking::run(move || {
                fires.mark_started(
                    row.tenant_scope.as_ref(),
                    row.schedule_id.as_ref(),
                    row.fire_count,
                    run.session_id,
                )
            })
            .await
            {
                Ok(_) => report.runs_started += 1,
                Err(_) => report.failures += 1,
            }
        }
        Ok(())
    }

    /// Purge one bounded batch of started fires older than the retention
    /// window. A purge that fails is one counted failure, never a tick
    /// error, and a horizon before the start of representable time means
    /// nothing can be that old, so the purge is skipped rather than failed.
    async fn purge_started(
        &self,
        now: Timestamp,
        limit: usize,
        report: &mut TickReport,
    ) -> Result<(), WorkerError> {
        // `build` already bounded the retention to an `i64` of milliseconds.
        let Ok(retention) = i64::try_from(self.fire_retention.as_millis()) else {
            return Ok(());
        };
        let Some(before) = now
            .as_unix_ms()
            .checked_sub(retention)
            .and_then(|ms| Timestamp::from_unix_ms(ms).ok())
        else {
            return Ok(());
        };
        let fires = Arc::clone(&self.fires);
        if crate::blocking::run(move || fires.purge_started(before, limit))
            .await
            .is_err()
        {
            report.failures += 1;
        }
        Ok(())
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
        let Ok(retry_at) = lease_deadline(now, backoff_ms(attempts)) else {
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
}

// Fire intent precedes schedule CAS so a crash can reconcile the same fire key.
fn claim_schedule(
    cron: &dyn CronScheduleStore,
    fires: &dyn FireStore,
    schedule: &CronSchedule,
    now: Timestamp,
) -> Result<bool, WorkerError> {
    let expected_next = schedule.next_fire_at.as_unix_ms();
    let mut claimed = schedule.clone();
    claimed.last_fired_at = Some(now);
    claimed.fire_count = claimed.fire_count.saturating_add(1);
    claimed.next_fire_at = claimed.expression.next_after(claimed.origin, now)?;
    fires.record_claimed(&FireRow {
        tenant_scope: Arc::clone(&claimed.tenant_scope),
        schedule_id: Arc::clone(&claimed.schedule_id),
        fire_count: claimed.fire_count,
        fired_at: now,
        status: FireStatus::Claimed,
        started_session: None,
    })?;
    Ok(cron.try_claim(
        claimed.tenant_scope.as_ref(),
        claimed.schedule_id.as_ref(),
        expected_next,
        now,
        &claimed,
    )?)
}
