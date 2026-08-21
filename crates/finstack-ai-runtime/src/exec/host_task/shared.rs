use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use finstack_ai_kernel::{KernelInput, TransitionEnv};

use crate::coordinator::{ModelDispatchSeed, ToolDispatchSeed};
use crate::event_hub::EventHubHandle;
use crate::host_driver::Signal;
use crate::observer::ObserverDiagnosticBuffer;
use crate::run_types::{RunHandleError, RunStatus, ShutdownReport};
use crate::{CommitOutcome, ModelRequest, ResolvedTool, ToolCallContext};

use super::oneshot::OneshotSender;

pub(super) struct Shared {
    pub(super) intake: Mutex<Option<Arc<CommandIntake>>>,
    pub(super) shutting_down: AtomicBool,
    pub(super) status: Mutex<RunStatus>,
    pub(super) status_changed: Signal,
    pub(super) events: EventHubHandle,
    pub(super) shutdown_report: Mutex<Option<ShutdownReport>>,
    pub(super) timer_already_due: AtomicU64,
    pub(super) timer_backward_clock_clamped: AtomicU64,
    pub(super) observer_diagnostics: Mutex<ObserverDiagnosticBuffer>,
    pub(super) work: Signal,
}

impl Shared {
    pub(super) fn new(events: EventHubHandle, capacity: usize) -> Arc<Self> {
        Arc::new(Self {
            intake: Mutex::new(Some(Arc::new(CommandIntake::new(capacity)))),
            shutting_down: AtomicBool::new(false),
            status: Mutex::new(RunStatus::Running),
            status_changed: Signal::new(),
            events,
            shutdown_report: Mutex::new(None),
            timer_already_due: AtomicU64::new(0),
            timer_backward_clock_clamped: AtomicU64::new(0),
            observer_diagnostics: Mutex::new(ObserverDiagnosticBuffer::default()),
            work: Signal::new(),
        })
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

pub(super) struct RunCommand {
    pub(super) env: TransitionEnv,
    pub(super) input: KernelInput,
    pub(super) reply: OneshotSender<Result<CommitOutcome, RunHandleError>>,
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
