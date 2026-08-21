use std::sync::Arc;
use std::sync::atomic::Ordering;

use finstack_ai_kernel::{KernelInput, TransitionEnv};

use crate::run_types::{RunHandleError, RunStatus, ShutdownReport, TimerDiagnostics};
use crate::{CommitOutcome, EventSubscription, EventSubscriptionConfig, EventSubscriptionError};
use crate::{ObserverDiagnostic, ObserverDiagnostics};

use super::oneshot::oneshot;
use super::shared::{RunCommand, Shared};

/// Cloneable bounded command/status/shutdown handle.
#[derive(Clone)]
pub struct RunHandle {
    pub(super) shared: Arc<Shared>,
}

impl RunHandle {
    /// Register an interactive subscription before publishing later run events.
    ///
    /// # Errors
    ///
    /// Rejects invalid configuration, exhausted subscriber capacity, or a closed hub.
    #[expect(
        clippy::unused_async,
        reason = "matches native RunHandle so Agent can await both owners"
    )]
    pub async fn subscribe_events(
        &self,
        config: EventSubscriptionConfig,
    ) -> Result<EventSubscription, EventSubscriptionError> {
        self.shared.events.subscribe_interactive(config)
    }

    /// Register a read-only observer subscription.
    ///
    /// The wasm-host hub has a single subscriber set; isolation is enforced on
    /// the native Tokio hub.
    ///
    /// # Errors
    ///
    /// Rejects invalid configuration, exhausted subscriber capacity, or a closed hub.
    #[expect(
        clippy::unused_async,
        reason = "matches native RunHandle so Agent can await both owners"
    )]
    pub async fn subscribe_observer(
        &self,
        config: EventSubscriptionConfig,
    ) -> Result<EventSubscription, EventSubscriptionError> {
        self.shared.events.subscribe_observer(config)
    }

    /// Submit one command, awaiting bounded-channel capacity when necessary.
    ///
    /// # Errors
    ///
    /// Rejects after shutdown/fault and forwards coordinator failures.
    pub async fn submit(
        &self,
        env: TransitionEnv,
        input: KernelInput,
    ) -> Result<CommitOutcome, RunHandleError> {
        match self.status() {
            RunStatus::Running => {}
            RunStatus::ShuttingDown => return Err(RunHandleError::ShuttingDown),
            RunStatus::Stopped => return Err(RunHandleError::Stopped),
            RunStatus::Faulted { code } => return Err(RunHandleError::Faulted { code }),
        }
        let intake = self
            .shared
            .intake
            .lock()
            .map_err(|_| RunHandleError::IntakeClosed)?
            .clone()
            .ok_or(RunHandleError::ShuttingDown)?;
        let (reply, receive) = oneshot();
        intake.push(RunCommand { env, input, reply }).await?;
        receive.await
    }

    /// Initiate idempotent shutdown and close shared intake.
    pub fn shutdown(&self) {
        if !self.shared.shutting_down.swap(true, Ordering::AcqRel) {
            if let Ok(intake) = self.shared.intake.lock()
                && let Some(intake) = intake.as_ref()
            {
                intake.close();
            }
            if !matches!(
                self.status(),
                RunStatus::Faulted { .. } | RunStatus::Stopped
            ) {
                self.set_status(RunStatus::ShuttingDown);
            }
            self.shared.work.notify_waiters();
        }
    }

    /// Read the latest lifecycle state without blocking.
    #[must_use]
    pub fn status(&self) -> RunStatus {
        self.shared.status.lock().map_or(
            RunStatus::Faulted {
                code: "run_status_lock_poisoned",
            },
            |status| *status,
        )
    }

    /// Read the shutdown report after the owner has settled.
    #[must_use]
    pub fn shutdown_report(&self) -> Option<ShutdownReport> {
        self.shared
            .shutdown_report
            .lock()
            .ok()
            .and_then(|report| *report)
    }

    /// Read cumulative safe timer diagnostics for this process-local run owner.
    #[must_use]
    pub fn timer_diagnostics(&self) -> TimerDiagnostics {
        TimerDiagnostics {
            already_due: self.shared.timer_already_due.load(Ordering::Acquire),
            backward_clock_clamped: self
                .shared
                .timer_backward_clock_clamped
                .load(Ordering::Acquire),
        }
    }

    /// Snapshot bounded, redacted observer-delivery diagnostics.
    #[must_use]
    pub fn observer_diagnostics(&self) -> ObserverDiagnostics {
        self.shared.observer_diagnostics.lock().map_or_else(
            |_| ObserverDiagnostics::unavailable(),
            |diagnostics| diagnostics.snapshot(),
        )
    }

    /// Retain one non-semantic observer diagnostic.
    pub(crate) fn record_observer_diagnostic(&self, diagnostic: ObserverDiagnostic) {
        if let Ok(mut diagnostics) = self.shared.observer_diagnostics.lock() {
            diagnostics.record(diagnostic);
        }
    }

    pub(super) fn set_status(&self, status: RunStatus) {
        if let Ok(mut current) = self.shared.status.lock() {
            *current = status;
        }
        self.shared.status_changed.notify_waiters();
    }
}
