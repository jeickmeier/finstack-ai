use std::ops::{Deref, DerefMut};
use std::sync::Arc;

use finstack_ai_runtime::{
    Clock, ExternalClock, JournalStore, LockedModelContextProfile, Model, OperationLocator,
    ResolvedToolCatalog, WorkflowDriverError, WorkflowSession,
};

use crate::cron::{CronError, CronFire, CronSchedule, IntervalSchedule, validate_schedule_id};
use crate::store::CronScheduleStore;

/// Shipped in-process driver of [`WorkflowSession`] plus adapter-owned cron.
pub struct LocalWorkflowDriver {
    session: WorkflowSession,
    cron: Arc<dyn CronScheduleStore>,
    catch_up: Vec<CronFire>,
}

impl LocalWorkflowDriver {
    /// Wrap a recovered session without firing catch-up.
    #[must_use]
    pub fn wrap(session: WorkflowSession, cron: Arc<dyn CronScheduleStore>) -> Self {
        Self {
            session,
            cron,
            catch_up: Vec::new(),
        }
    }

    /// Attach to an accepted run and load tenant-scoped cron rows.
    ///
    /// Overdue schedules fire once against [`WorkflowSession::clock`], then
    /// advance to the next future tick. This is adapter state, not a kernel
    /// record.
    ///
    /// # Errors
    ///
    /// Returns [`WorkflowDriverError`] from session attach, or a recover
    /// failure when catch-up cannot persist.
    pub async fn attach(
        store: Arc<dyn JournalStore>,
        locator: OperationLocator,
        clock: ExternalClock,
        random_seed: u64,
        cron: Arc<dyn CronScheduleStore>,
    ) -> Result<Self, WorkflowDriverError> {
        let session = WorkflowSession::trusted(store, locator, clock, random_seed).await?;
        let mut driver = Self::wrap(session, cron);
        driver.catch_up = driver
            .fire_due()
            .map_err(|_| WorkflowDriverError::Recover {
                code: "cron_catch_up",
            })?;
        Ok(driver)
    }

    /// Bind model/tool ports used when the driver must spawn a run owner.
    #[must_use]
    pub fn with_ports(
        mut self,
        model: Arc<dyn Model>,
        profile: LockedModelContextProfile,
        catalog: Option<Arc<ResolvedToolCatalog>>,
    ) -> Self {
        self.session = self.session.with_ports(model, profile, catalog);
        self
    }

    /// Borrow the inner session.
    #[must_use]
    pub const fn session(&self) -> &WorkflowSession {
        &self.session
    }

    /// Borrow the inner session mutably.
    #[must_use]
    pub const fn session_mut(&mut self) -> &mut WorkflowSession {
        &mut self.session
    }

    /// Catch-up fires recorded by the last [`Self::attach`].
    #[must_use]
    pub fn catch_up_fires(&self) -> &[CronFire] {
        &self.catch_up
    }

    /// Persist a tenant-scoped schedule. Next-fire uses the injected clock.
    ///
    /// # Errors
    ///
    /// Returns invalid identity, expression, clock, or store failures.
    pub fn schedule_cron(
        &self,
        schedule_id: &str,
        expression: IntervalSchedule,
    ) -> Result<CronSchedule, CronError> {
        let schedule_id = validate_schedule_id(schedule_id)?;
        let now = self
            .session
            .clock()
            .now()
            .map_err(|_| CronError::TimeOverflow)?;
        let next_fire_at = expression.next_after(now, now)?;
        let schedule = CronSchedule {
            tenant_scope: Arc::from(self.session.tenant_scope()),
            schedule_id,
            expression,
            origin: now,
            next_fire_at,
            last_fired_at: None,
            fire_count: 0,
        };
        self.cron.upsert(&schedule)?;
        Ok(schedule)
    }

    /// Tenant-scoped schedules loaded from the adapter table.
    ///
    /// # Errors
    ///
    /// Returns store failures.
    pub fn schedules(&self) -> Result<Vec<CronSchedule>, CronError> {
        self.cron.load_tenant(self.session.tenant_scope())
    }

    /// Fire each schedule whose next-fire is at or before the injected clock.
    ///
    /// Each overdue schedule fires once. The next tick is the first future
    /// origin-aligned instant. Missed ticks are not backfilled.
    ///
    /// # Errors
    ///
    /// Returns clock or store failures.
    pub fn fire_due(&self) -> Result<Vec<CronFire>, CronError> {
        let now = self
            .session
            .clock()
            .now()
            .map_err(|_| CronError::TimeOverflow)?;
        let tenant = self.session.tenant_scope();
        let mut fires = Vec::new();
        for schedule in self.cron.load_tenant(tenant)? {
            if schedule.next_fire_at > now {
                continue;
            }
            let expected_next = schedule.next_fire_at.as_unix_ms();
            let mut claimed = schedule.clone();
            claimed.last_fired_at = Some(now);
            claimed.fire_count = claimed.fire_count.saturating_add(1);
            claimed.next_fire_at = claimed.expression.next_after(claimed.origin, now)?;
            if !self.cron.try_claim(
                tenant,
                claimed.schedule_id.as_ref(),
                expected_next,
                now,
                &claimed,
            )? {
                continue;
            }
            fires.push(CronFire {
                tenant_scope: Arc::clone(&claimed.tenant_scope),
                schedule_id: Arc::clone(&claimed.schedule_id),
                fired_at: now,
            });
        }
        Ok(fires)
    }
}

impl Deref for LocalWorkflowDriver {
    type Target = WorkflowSession;

    fn deref(&self) -> &Self::Target {
        &self.session
    }
}

impl DerefMut for LocalWorkflowDriver {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.session
    }
}
