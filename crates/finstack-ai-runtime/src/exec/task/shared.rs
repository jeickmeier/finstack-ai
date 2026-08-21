use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{Arc, Mutex};

use finstack_ai_kernel::{KernelInput, TransitionEnv};
use tokio::sync::{mpsc, oneshot, watch};

use crate::CommitOutcome;
use crate::event_hub::EventHubHandle;
use crate::exec::live_state::{LiveRunState, LiveStatePublisher, session_head_update};
use crate::observer::ObserverDiagnosticBuffer;
use crate::run_types::{RunHandleError, RunStatus, ShutdownReport};

pub(super) struct Shared {
    pub(super) sender: Mutex<Option<mpsc::Sender<RunCommand>>>,
    pub(super) shutting_down: AtomicBool,
    pub(super) status: watch::Sender<RunStatus>,
    pub(super) live_state: watch::Sender<LiveRunState>,
    pub(super) kernel_state: Mutex<finstack_ai_kernel::KernelState>,
    pub(super) record_kinds: Mutex<Arc<[Arc<str>]>>,
    pub(super) session_head: Mutex<Option<crate::SessionHeadUpdate>>,
    pub(super) compaction_checkpoint: Mutex<Option<crate::CompactionCheckpoint>>,
    pub(super) events: EventHubHandle,
    pub(super) shutdown_report: Mutex<Option<ShutdownReport>>,
    pub(super) timer_already_due: AtomicU64,
    pub(super) timer_backward_clock_clamped: AtomicU64,
    pub(super) observer_diagnostics: Mutex<ObserverDiagnosticBuffer>,
}

impl Shared {
    pub(super) fn publish_lifecycle(&self, status: RunStatus) {
        self.status.send_replace(status);
        let current = self.live_state.borrow().clone();
        self.live_state.send_replace(current.next_lifecycle(status));
    }
}

impl LiveStatePublisher for Shared {
    fn publish_semantic(
        &self,
        state: &finstack_ai_kernel::KernelState,
        session: &finstack_ai_kernel::SessionProjection,
        head_checksum: Option<finstack_ai_kernel::Digest>,
        fault_code: Option<&'static str>,
        record_kinds: &[Arc<str>],
    ) {
        if let Ok(mut current) = self.kernel_state.lock() {
            *current = state.clone();
        }
        if let Ok(mut current) = self.record_kinds.lock() {
            *current = record_kinds.into();
        }
        if let Ok(mut current) = self.session_head.lock() {
            *current = session_head_update(state, session, head_checksum);
        }
        let current = self.live_state.borrow().clone();
        let status = *self.status.borrow();
        self.live_state.send_replace(LiveRunState::next_semantic(
            current.revision,
            status,
            fault_code.map(Arc::from).or(current.fault_code),
            state,
        ));
    }

    fn publish_compaction_checkpoint(&self, checkpoint: Option<&crate::CompactionCheckpoint>) {
        if let Ok(mut current) = self.compaction_checkpoint.lock() {
            current.clone_from(&checkpoint.cloned());
        }
    }
}

pub(super) struct RunCommand {
    pub(super) env: TransitionEnv,
    pub(super) input: KernelInput,
    pub(super) reply: oneshot::Sender<Result<CommitOutcome, RunHandleError>>,
}
