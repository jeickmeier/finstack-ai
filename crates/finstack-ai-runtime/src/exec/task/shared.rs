use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{Arc, Mutex};

pub(super) use crate::exec::run_control::RunCommand;
use tokio::sync::{mpsc, watch};

use crate::event_hub::EventHubHandle;
use crate::exec::live_state::{LiveRunState, LiveStatePublisher, session_head_update};
use crate::observer::ObserverDiagnosticBuffer;
use crate::run_types::{RunLifecycle, RunStatus, ShutdownReport};

use super::handle::RunHandle;

pub(super) struct Shared {
    pub(super) control: Arc<crate::exec::run_control::RunControl>,
    pub(super) sender: Mutex<Option<mpsc::Sender<RunCommand>>>,
    pub(super) shutting_down: AtomicBool,
    pub(super) status: watch::Sender<RunStatus>,
    pub(super) live_state: watch::Sender<LiveRunState>,
    pub(super) kernel_state: Mutex<finstack_ai_kernel::KernelState>,
    pub(super) record_kinds: Mutex<Arc<[Arc<str>]>>,
    pub(super) session_head: Mutex<Option<crate::session::SessionHeadUpdate>>,
    pub(super) compaction_checkpoint: Mutex<Option<crate::ports::middleware::CompactionCheckpoint>>,
    pub(super) events: EventHubHandle,
    pub(super) shutdown_report: Mutex<Option<ShutdownReport>>,
    pub(super) timer_already_due: AtomicU64,
    pub(super) timer_backward_clock_clamped: AtomicU64,
    pub(super) observer_diagnostics: Mutex<ObserverDiagnosticBuffer>,
}

impl Shared {
    /// Build the owner/handle pair over a fresh command intake and event hub.
    pub(super) fn spawn(
        sender: mpsc::Sender<RunCommand>,
        events: EventHubHandle,
        state: &finstack_ai_kernel::KernelState,
    ) -> (Arc<Self>, RunHandle) {
        let (status_sender, status_receiver) = watch::channel(RunStatus::Running);
        let (live_state_sender, live_state_receiver) = watch::channel(LiveRunState::initial(state));
        let shared = Arc::new(Self {
            control: Arc::default(),
            sender: Mutex::new(Some(sender)),
            shutting_down: AtomicBool::new(false),
            status: status_sender,
            live_state: live_state_sender,
            kernel_state: Mutex::new(state.clone()),
            record_kinds: Mutex::new(Arc::from([])),
            session_head: Mutex::new(None),
            compaction_checkpoint: Mutex::new(None),
            events,
            shutdown_report: Mutex::new(None),
            timer_already_due: AtomicU64::new(0),
            timer_backward_clock_clamped: AtomicU64::new(0),
            observer_diagnostics: Mutex::new(ObserverDiagnosticBuffer::default()),
        });
        let handle = RunHandle {
            shared: Arc::clone(&shared),
            status: status_receiver,
            live_state: live_state_receiver,
        };
        (shared, handle)
    }

    pub(super) fn publish_lifecycle(&self, status: RunStatus) {
        self.status.send_replace(status);
        let current = self.live_state.borrow().clone();
        self.live_state.send_replace(current.next_lifecycle(status));
    }
}

impl RunLifecycle for Shared {
    fn lifecycle_status(&self) -> RunStatus {
        *self.status.borrow()
    }

    fn set_lifecycle(&self, status: RunStatus) {
        self.publish_lifecycle(status);
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

    fn publish_compaction_checkpoint(
        &self,
        checkpoint: Option<&crate::ports::middleware::CompactionCheckpoint>,
    ) {
        if let Ok(mut current) = self.compaction_checkpoint.lock() {
            current.clone_from(&checkpoint.cloned());
        }
    }
}
