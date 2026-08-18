use std::sync::Arc;
use std::sync::atomic::Ordering;

use tokio::sync::mpsc;

use crate::run_types::{RunHandleError, RunStatus};
use crate::{CommitCoordinatorError, CommitOutcome};

use super::shared::{RunCommand, Shared};

pub(super) fn result_fault_code(
    result: &Result<CommitOutcome, RunHandleError>,
) -> Option<Arc<str>> {
    match result {
        Ok(outcome) => outcome.fault.map(|fault| Arc::from(fault.code)),
        Err(RunHandleError::Faulted { code } | RunHandleError::Tool { code }) => {
            Some(Arc::clone(code))
        }
        Err(
            RunHandleError::ModelSettlement { code }
            | RunHandleError::ToolSettlement { code }
            | RunHandleError::InteractionSettlement { code }
            | RunHandleError::EventDelivery { code }
            | RunHandleError::Coordinator(
                CommitCoordinatorError::BoundaryFault { code }
                | CommitCoordinatorError::Faulted { code }
                | CommitCoordinatorError::EventDelivery { code },
            ),
        ) => Some(Arc::from(*code)),
        _ => None,
    }
}

pub(super) fn fault_worker(
    shared: &Shared,
    receiver: &mut mpsc::Receiver<RunCommand>,
    code: impl Into<Arc<str>>,
) {
    shared.shutting_down.store(true, Ordering::Release);
    if let Ok(mut sender) = shared.sender.lock() {
        sender.take();
    }
    receiver.close();
    shared
        .status
        .send_replace(RunStatus::Faulted { code: code.into() });
}

pub(super) fn model_runtime_fault(error: &RunHandleError) -> Arc<str> {
    match error {
        RunHandleError::Faulted { code } | RunHandleError::Model { code } => Arc::clone(code),
        RunHandleError::ModelSettlement { code }
        | RunHandleError::Timer { code }
        | RunHandleError::CancellationSettlement { code }
        | RunHandleError::EventDelivery { code }
        | RunHandleError::Coordinator(
            CommitCoordinatorError::BoundaryFault { code }
            | CommitCoordinatorError::Decision { code }
            | CommitCoordinatorError::Faulted { code }
            | CommitCoordinatorError::EventDelivery { code },
        ) => Arc::from(*code),
        _ => Arc::from("model_runtime_failed"),
    }
}

pub(super) fn runtime_fault(error: &RunHandleError) -> Arc<str> {
    match error {
        RunHandleError::ToolSettlement { code }
        | RunHandleError::InteractionSettlement { code } => Arc::from(*code),
        RunHandleError::Tool { code } => Arc::clone(code),
        _ => model_runtime_fault(error),
    }
}
