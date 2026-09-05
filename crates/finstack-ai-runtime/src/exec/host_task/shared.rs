use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

pub(super) use crate::exec::run_control::RunCommand;

use crate::coordinator::{ModelDispatchSeed, ToolDispatchSeed};
use crate::driver::host_driver::Signal;
use crate::event_hub::EventHubHandle;
use crate::exec::live_state::{LiveRunState, LiveStatePublisher, session_head_update};
use crate::observer::ObserverDiagnosticBuffer;
use crate::ports::model::ModelRequest;
use crate::ports::tool::{ResolvedTool, ToolCallContext};
use crate::run_types::{RunHandleError, RunLifecycle, RunStatus, ShutdownReport};

pub(super) struct Shared {
    pub(super) control: Arc<crate::exec::run_control::RunControl>,
    pub(super) intake: Mutex<Option<Arc<CommandIntake>>>,
    pub(super) shutting_down: AtomicBool,
    pub(super) status: Mutex<RunStatus>,
    pub(super) status_changed: Signal,
    pub(super) live_state: Mutex<LiveRunState>,
    pub(super) kernel_state: Mutex<finstack_ai_kernel::KernelState>,
    pub(super) record_kinds: Mutex<Arc<[Arc<str>]>>,
    pub(super) session_head: Mutex<Option<crate::session::SessionHeadUpdate>>,
    pub(super) compaction_checkpoint: Mutex<Option<crate::ports::middleware::CompactionCheckpoint>>,
    pub(super) live_state_changed: Signal,
    pub(super) events: EventHubHandle,
    pub(super) shutdown_report: Mutex<Option<ShutdownReport>>,
    pub(super) timer_already_due: AtomicU64,
    pub(super) timer_backward_clock_clamped: AtomicU64,
    pub(super) observer_diagnostics: Mutex<ObserverDiagnosticBuffer>,
    pub(super) work: Signal,
}

impl Shared {
    pub(super) fn new(
        events: EventHubHandle,
        capacity: usize,
        initial_live_state: LiveRunState,
        initial_kernel_state: finstack_ai_kernel::KernelState,
    ) -> Arc<Self> {
        Arc::new(Self {
            control: Arc::default(),
            intake: Mutex::new(Some(Arc::new(CommandIntake::new(capacity)))),
            shutting_down: AtomicBool::new(false),
            status: Mutex::new(RunStatus::Running),
            status_changed: Signal::new(),
            live_state: Mutex::new(initial_live_state),
            kernel_state: Mutex::new(initial_kernel_state),
            record_kinds: Mutex::new(Arc::from([])),
            session_head: Mutex::new(None),
            compaction_checkpoint: Mutex::new(None),
            live_state_changed: Signal::new(),
            events,
            shutdown_report: Mutex::new(None),
            timer_already_due: AtomicU64::new(0),
            timer_backward_clock_clamped: AtomicU64::new(0),
            observer_diagnostics: Mutex::new(ObserverDiagnosticBuffer::default()),
            work: Signal::new(),
        })
    }

    pub(super) fn publish_lifecycle(&self, status: RunStatus) {
        if let Ok(mut current) = self.status.lock() {
            *current = status;
        }
        self.status_changed.notify_waiters();
        if let Ok(mut current) = self.live_state.lock() {
            *current = current.next_lifecycle(status);
        }
        self.live_state_changed.notify_waiters();
    }
}

impl RunLifecycle for Shared {
    fn lifecycle_status(&self) -> RunStatus {
        self.status.lock().map_or(
            RunStatus::Faulted {
                code: "run_status_lock_poisoned",
            },
            |status| *status,
        )
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
        let status = self.lifecycle_status();
        if let Ok(mut current) = self.live_state.lock() {
            *current = LiveRunState::next_semantic(
                current.revision,
                status,
                fault_code
                    .map(Arc::from)
                    .or_else(|| current.fault_code.clone()),
                state,
            );
        }
        self.live_state_changed.notify_waiters();
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

pub(super) struct CommandIntake {
    capacity: usize,
    queue: Mutex<VecDeque<RunCommand>>,
    available: Signal,
    space: Signal,
    closed: AtomicBool,
}

impl CommandIntake {
    pub(super) fn new(capacity: usize) -> Self {
        Self {
            capacity,
            queue: Mutex::new(VecDeque::new()),
            available: Signal::new(),
            space: Signal::new(),
            closed: AtomicBool::new(false),
        }
    }

    pub(super) fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.available.notify_waiters();
        self.space.notify_waiters();
    }

    pub(super) async fn push(&self, command: RunCommand) -> Result<(), RunHandleError> {
        loop {
            if self.closed.load(Ordering::Acquire) {
                return Err(RunHandleError::ShuttingDown);
            }
            {
                let mut queue = self
                    .queue
                    .lock()
                    .map_err(|_| RunHandleError::IntakeClosed)?;
                if queue.len() < self.capacity {
                    queue.push_back(command);
                    self.available.notify_waiters();
                    return Ok(());
                }
            }
            self.space.notified().await;
        }
    }

    pub(super) async fn recv(&self) -> Option<RunCommand> {
        loop {
            let notified = self.available.notified();
            if let Ok(mut queue) = self.queue.lock()
                && let Some(command) = queue.pop_front()
            {
                self.space.notify_waiters();
                return Some(command);
            }
            if self.closed.load(Ordering::Acquire) {
                return None;
            }
            notified.await;
        }
    }
}

pub(super) enum HostWork {
    Model {
        seed: ModelDispatchSeed,
        request: ModelRequest,
    },
    Tool {
        seed: ToolDispatchSeed,
        context: ToolCallContext,
        resolved: Arc<ResolvedTool>,
    },
}
