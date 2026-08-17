use std::sync::atomic::Ordering;

use crate::run_types::{RunHandleError, RunStatus};
use crate::{CommitCoordinatorError, CommitOutcome, ModelError};

use super::handle::RunHandle;
use super::shared::Shared;

pub(super) fn model_cancellation_error(message: &'static str) -> ModelError {
    ModelError::try_new(
        "model_cancelled",
        finstack_ai_kernel::ErrorCategory::Cancellation,
        false,
        message,
        finstack_ai_kernel::Metadata::empty(),
    )
    .expect("frozen cancellation error")
}

pub(super) fn stable_dispatch_code(code: &str) -> &'static str {
    match code {
        crate::MODEL_REQUEST_INVALID => crate::MODEL_REQUEST_INVALID,
        crate::MODEL_PROFILE_INVALID => crate::MODEL_PROFILE_INVALID,
        crate::MODEL_PROFILE_RELAXATION => crate::MODEL_PROFILE_RELAXATION,
        crate::MODEL_PROFILE_OVERRIDE_NOT_ALLOWED => crate::MODEL_PROFILE_OVERRIDE_NOT_ALLOWED,
        crate::MODEL_ESTIMATOR_MISMATCH => crate::MODEL_ESTIMATOR_MISMATCH,
        crate::MODEL_CONTEXT_LIMIT_EXCEEDED => crate::MODEL_CONTEXT_LIMIT_EXCEEDED,
        _ => "model_request_invalid",
    }
}

pub(super) fn result_fault_code(
    result: &Result<CommitOutcome, RunHandleError>,
) -> Option<&'static str> {
    match result {
        Ok(outcome) => outcome.fault.map(|fault| fault.code),
        Err(
            RunHandleError::Faulted { code }
            | RunHandleError::ModelSettlement { code }
            | RunHandleError::ToolSettlement { code }
            | RunHandleError::InteractionSettlement { code }
            | RunHandleError::EventDelivery { code }
            | RunHandleError::Coordinator(
                CommitCoordinatorError::BoundaryFault { code }
                | CommitCoordinatorError::Faulted { code }
                | CommitCoordinatorError::EventDelivery { code },
            ),
        ) => Some(*code),
        _ => None,
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
