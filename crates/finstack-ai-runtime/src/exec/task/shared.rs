use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64};

use finstack_ai_kernel::{KernelInput, TransitionEnv};
use tokio::sync::{mpsc, oneshot, watch};

use crate::CommitOutcome;
use crate::event_hub::EventHubHandle;
use crate::run_types::{RunHandleError, RunStatus, ShutdownReport};

pub(super) struct Shared {
    pub(super) sender: Mutex<Option<mpsc::Sender<RunCommand>>>,
    pub(super) shutting_down: AtomicBool,
    pub(super) status: watch::Sender<RunStatus>,
    pub(super) events: EventHubHandle,
    pub(super) shutdown_report: Mutex<Option<ShutdownReport>>,
    pub(super) timer_already_due: AtomicU64,
    pub(super) timer_backward_clock_clamped: AtomicU64,
}

pub(super) struct RunCommand {
    pub(super) env: TransitionEnv,
    pub(super) input: KernelInput,
    pub(super) reply: oneshot::Sender<Result<CommitOutcome, RunHandleError>>,
}
