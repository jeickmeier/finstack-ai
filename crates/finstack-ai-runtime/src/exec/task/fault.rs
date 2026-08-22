use std::sync::atomic::Ordering;

use tokio::sync::mpsc;

use crate::commit::CommitCoordinatorError;
use crate::run_types::{RunHandleError, RunStatus};

use super::shared::{RunCommand, Shared};

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
    shared.publish_lifecycle(RunStatus::Faulted { code });
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
