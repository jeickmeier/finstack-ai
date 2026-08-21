use std::sync::Arc;
use std::sync::atomic::Ordering;

use finstack_ai_kernel::{KernelInput, TransitionEnv};
use tokio::sync::{oneshot, watch};

use crate::LiveRunState;
use crate::run_types::{RunHandleError, RunStatus, ShutdownReport, TimerDiagnostics};
use crate::{CommitOutcome, EventSubscription, EventSubscriptionConfig, EventSubscriptionError};
use crate::{ObserverDiagnostic, ObserverDiagnostics};

use super::shared::{RunCommand, Shared};

/// Cloneable bounded command/status/shutdown handle.
#[derive(Clone)]
pub struct RunHandle {
    pub(super) shared: Arc<Shared>,
    pub(super) status: watch::Receiver<RunStatus>,
    pub(super) live_state: watch::Receiver<LiveRunState>,
}

impl RunHandle {
    /// Register an interactive subscription before publishing later run events.
    ///
    /// # Errors
    ///
    /// Rejects invalid configuration, exhausted subscriber capacity, or a closed hub.
    pub async fn subscribe_events(
        &self,
        config: EventSubscriptionConfig,
    ) -> Result<EventSubscription, EventSubscriptionError> {
        self.shared.events.subscribe_interactive(config).await
    }

    /// Register a read-only observer subscription isolated from interactive delivery.
    ///
    /// # Errors
    ///
    /// Rejects invalid configuration, exhausted subscriber capacity, or a closed hub.
    pub async fn subscribe_observer(
        &self,
        config: EventSubscriptionConfig,
    ) -> Result<EventSubscription, EventSubscriptionError> {
        self.shared.events.subscribe_observer(config).await
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
        let sender = self
            .shared
            .sender
            .lock()
            .map_err(|_| RunHandleError::IntakeClosed)?
            .clone()
            .ok_or(RunHandleError::ShuttingDown)?;
        let (reply, receive) = oneshot::channel();
        sender
            .send(RunCommand { env, input, reply })
            .await
            .map_err(|_| self.closed_error())?;
        receive.await.map_err(|_| self.closed_error())?
    }

    /// Initiate idempotent shutdown and close shared intake.
    pub fn shutdown(&self) {
        if !self.shared.shutting_down.swap(true, Ordering::AcqRel) {
            if let Ok(mut sender) = self.shared.sender.lock() {
                sender.take();
            }
            if !matches!(
                self.status(),
                RunStatus::Faulted { .. } | RunStatus::Stopped
            ) {
                self.shared.publish_lifecycle(RunStatus::ShuttingDown);
            }
        }
    }

    /// Read the latest lifecycle state without blocking.
    #[must_use]
    pub fn status(&self) -> RunStatus {
        *self.status.borrow()
    }

    /// Clone a watch receiver for asynchronous status observation.
    #[must_use]
    pub fn observe_status(&self) -> watch::Receiver<RunStatus> {
        self.status.clone()
    }

    /// Read the latest confirmed semantic and lifecycle snapshot.
    #[must_use]
    pub fn live_state(&self) -> LiveRunState {
        self.live_state.borrow().clone()
    }

    /// Wait until the latest-only view advances beyond `after_revision`.
    ///
    /// # Errors
    ///
    /// Returns [`RunHandleError::Stopped`] if the owner closes before a newer
    /// revision is published.
    pub async fn wait_for_live_state(
        &self,
        after_revision: u64,
    ) -> Result<LiveRunState, RunHandleError> {
        let mut receiver = self.live_state.clone();
        loop {
            let current = receiver.borrow_and_update().clone();
            if current.revision > after_revision {
                return Ok(current);
            }
            receiver
                .changed()
                .await
                .map_err(|_| RunHandleError::Stopped)?;
        }
    }

    pub(crate) fn kernel_state(&self) -> Option<finstack_ai_kernel::KernelState> {
        self.shared
            .kernel_state
            .lock()
            .ok()
            .map(|state| state.clone())
    }

    #[doc(hidden)]
    #[must_use]
    pub fn record_kinds(&self) -> Arc<[Arc<str>]> {
        self.shared
            .record_kinds
            .lock()
            .map_or_else(|_| Arc::from([]), |kinds| Arc::clone(&kinds))
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

    fn closed_error(&self) -> RunHandleError {
        match self.status() {
            RunStatus::Running => RunHandleError::IntakeClosed,
            RunStatus::ShuttingDown => RunHandleError::ShuttingDown,
            RunStatus::Stopped => RunHandleError::Stopped,
            RunStatus::Faulted { code } => RunHandleError::Faulted { code },
        }
    }
}
