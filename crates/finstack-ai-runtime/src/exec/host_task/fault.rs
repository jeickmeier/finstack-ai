use std::sync::atomic::Ordering;

use crate::ModelError;
use crate::run_types::{RunHandleError, RunStatus};

use super::handle::RunHandle;
use super::shared::Shared;

pub(super) fn model_cancellation_error(message: &'static str) -> ModelError {
    ModelError::frozen(
        "model_cancelled",
        finstack_ai_kernel::ErrorCategory::Cancellation,
        false,
        message,
    )
}

/// Prefer a carried settlement code; keep the host-specific fallback when the
/// variant is not a worker-tearing settlement fault. Native owners use
/// `runtime_fault` / `model_runtime_fault` instead — those sets include
/// `Timer` and `Coordinator::Decision`, which this host path does not.
pub(super) fn host_drain_fault(error: &RunHandleError, fallback: &'static str) -> &'static str {
    match error {
        RunHandleError::Faulted { code }
        | RunHandleError::ModelSettlement { code }
        | RunHandleError::ToolSettlement { code }
        | RunHandleError::InteractionSettlement { code }
        | RunHandleError::CancellationSettlement { code }
        | RunHandleError::EventDelivery { code } => code,
        _ => fallback,
    }
}

pub(super) fn fault_shared(shared: &Shared, code: &'static str) {
    shared.shutting_down.store(true, Ordering::Release);
    if let Ok(mut intake) = shared.intake.lock() {
        intake.take();
    }
    if let Ok(mut status) = shared.status.lock() {
        *status = RunStatus::Faulted { code };
    }
    shared.status_changed.notify_waiters();
    shared.work.notify_waiters();
}

pub(super) fn finish_worker(shared: &Shared) {
    if !matches!(
        shared
            .status
            .lock()
            .map_or(RunStatus::Stopped, |status| *status),
        RunStatus::Faulted { .. }
    ) {
        if let Ok(mut status) = shared.status.lock() {
            *status = RunStatus::Stopped;
        }
        shared.status_changed.notify_waiters();
    }
    shared.events.close();
}

pub(super) async fn wait_until_stopped(handle: &RunHandle) {
    loop {
        if matches!(
            handle.status(),
            RunStatus::Stopped | RunStatus::Faulted { .. }
        ) {
            return;
        }
        handle.shared.status_changed.notified().await;
    }
}
