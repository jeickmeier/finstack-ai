use std::sync::atomic::Ordering;

use tokio::sync::mpsc;

use crate::run_types::{RunHandleError, RunStatus};
use crate::{CommitCoordinatorError, CommitOutcome};

use super::shared::{RunCommand, Shared};

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

pub(super) fn fault_worker(
    shared: &Shared,
    receiver: &mut mpsc::Receiver<RunCommand>,
    code: &'static str,
) {
    shared.shutting_down.store(true, Ordering::Release);
    if let Ok(mut sender) = shared.sender.lock() {
        sender.take();
    }
    receiver.close();
    shared.status.send_replace(RunStatus::Faulted { code });
}

pub(super) fn model_runtime_fault(error: &RunHandleError) -> &'static str {
    match error {
        RunHandleError::Faulted { code }
        | RunHandleError::ModelSettlement { code }
        | RunHandleError::Timer { code }
        | RunHandleError::CancellationSettlement { code }
        | RunHandleError::EventDelivery { code }
        | RunHandleError::Coordinator(
            CommitCoordinatorError::BoundaryFault { code }
            | CommitCoordinatorError::Decision { code }
            | CommitCoordinatorError::Faulted { code }
            | CommitCoordinatorError::EventDelivery { code },
        ) => code,
        _ => "model_runtime_failed",
    }
}

pub(super) fn runtime_fault(error: &RunHandleError) -> &'static str {
    match error {
        RunHandleError::ToolSettlement { code }
        | RunHandleError::InteractionSettlement { code } => code,
        _ => model_runtime_fault(error),
    }
}
