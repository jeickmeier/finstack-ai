//! Bounded native Tokio task ownership for one runtime coordinator.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use finstack_ai_kernel::{KernelInput, TransitionEnv};
use thiserror::Error;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinSet;
use tokio::time::timeout;

use crate::{CommitCoordinator, CommitCoordinatorError, CommitOutcome};

/// Observable lifecycle of one owned runtime task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStatus {
    /// Intake and commit processing are active.
    Running,
    /// New intake is closed while the current boundary finishes.
    ShuttingDown,
    /// The owned worker has stopped.
    Stopped,
    /// Runtime uncertainty faulted this run only.
    Faulted {
        /// Stable fault code.
        code: &'static str,
    },
}

/// Configuration for a bounded native run task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunTaskConfig {
    /// Bounded command queue capacity.
    pub command_capacity: usize,
    /// Maximum time the owner waits before aborting owned tasks.
    pub shutdown_deadline: Duration,
}

impl RunTaskConfig {
    /// Validate non-zero queue and shutdown bounds.
    ///
    /// # Errors
    ///
    /// Returns [`RunHandleError::InvalidConfiguration`] for a zero bound.
    pub fn validate(self) -> Result<Self, RunHandleError> {
        if self.command_capacity == 0 || self.shutdown_deadline.is_zero() {
            return Err(RunHandleError::InvalidConfiguration);
        }
        Ok(self)
    }
}

/// Cloneable bounded command/status/shutdown handle.
#[derive(Clone)]
pub struct RunHandle {
    shared: Arc<Shared>,
    status: watch::Receiver<RunStatus>,
}

impl RunHandle {
    /// Submit one command, awaiting bounded-channel capacity when necessary.
    ///
    /// # Errors
    ///
    /// Rejects after shutdown/fault and forwards coordinator failures.
    pub async fn submit(
        &self,
        env: TransitionEnv,
        input: KernelInput,
    ) -> Result<CommitOutcome, RunHandleError> {
        match self.status() {
            RunStatus::Running => {}
            RunStatus::ShuttingDown => return Err(RunHandleError::ShuttingDown),
            RunStatus::Stopped => return Err(RunHandleError::Stopped),
            RunStatus::Faulted { code } => return Err(RunHandleError::Faulted { code }),
        }
        let sender = self
            .shared
            .sender
            .lock()
            .map_err(|_| RunHandleError::IntakeClosed)?
            .clone()
            .ok_or(RunHandleError::ShuttingDown)?;
        let (reply, receive) = oneshot::channel();
        sender
            .send(RunCommand { env, input, reply })
            .await
            .map_err(|_| self.closed_error())?;
        receive.await.map_err(|_| self.closed_error())?
    }

    /// Initiate idempotent shutdown and close shared intake.
    pub fn shutdown(&self) {
        if !self.shared.shutting_down.swap(true, Ordering::AcqRel) {
            if let Ok(mut sender) = self.shared.sender.lock() {
                sender.take();
            }
            if !matches!(
                self.status(),
                RunStatus::Faulted { .. } | RunStatus::Stopped
            ) {
                self.shared.status.send_replace(RunStatus::ShuttingDown);
            }
        }
    }

    /// Read the latest lifecycle state without blocking.
    #[must_use]
    pub fn status(&self) -> RunStatus {
        *self.status.borrow()
    }

    /// Clone a watch receiver for asynchronous status observation.
    #[must_use]
    pub fn observe_status(&self) -> watch::Receiver<RunStatus> {
        self.status.clone()
    }

    fn closed_error(&self) -> RunHandleError {
        match self.status() {
            RunStatus::Running => RunHandleError::IntakeClosed,
            RunStatus::ShuttingDown => RunHandleError::ShuttingDown,
            RunStatus::Stopped => RunHandleError::Stopped,
            RunStatus::Faulted { code } => RunHandleError::Faulted { code },
        }
    }
}

/// Single owner of all tasks spawned for one run.
pub struct RunTaskOwner {
    handle: RunHandle,
    tasks: JoinSet<()>,
    shutdown_deadline: Duration,
    joined: bool,
}

impl RunTaskOwner {
    /// Spawn one bounded run worker on the current Tokio runtime.
    ///
    /// # Errors
    ///
    /// Returns [`RunHandleError::InvalidConfiguration`] for zero bounds.
    pub fn spawn(
        coordinator: CommitCoordinator,
        config: RunTaskConfig,
    ) -> Result<Self, RunHandleError> {
        let config = config.validate()?;
        let (sender, receiver) = mpsc::channel(config.command_capacity);
        let (status_sender, status_receiver) = watch::channel(RunStatus::Running);
        let shared = Arc::new(Shared {
            sender: Mutex::new(Some(sender)),
            shutting_down: AtomicBool::new(false),
            status: status_sender,
        });
        let handle = RunHandle {
            shared: Arc::clone(&shared),
            status: status_receiver,
        };
        let mut tasks = JoinSet::new();
        tasks.spawn(run_worker(coordinator, receiver, shared));
        Ok(Self {
            handle,
            tasks,
            shutdown_deadline: config.shutdown_deadline,
            joined: false,
        })
    }

    /// Clone the run handle without transferring task ownership.
    #[must_use]
    pub fn handle(&self) -> RunHandle {
        self.handle.clone()
    }

    /// Close intake, join normally, and abort on deadline expiry.
    pub async fn shutdown(&mut self) {
        if self.joined {
            return;
        }
        self.handle.shutdown();
        let joined = timeout(self.shutdown_deadline, async {
            while self.tasks.join_next().await.is_some() {}
        })
        .await
        .is_ok();
        if !joined {
            self.tasks.abort_all();
            while self.tasks.join_next().await.is_some() {}
            if !matches!(self.handle.status(), RunStatus::Faulted { .. }) {
                self.handle.shared.status.send_replace(RunStatus::Stopped);
            }
        }
        self.joined = true;
    }
}

impl Drop for RunTaskOwner {
    fn drop(&mut self) {
        if !self.joined {
            self.handle.shutdown();
            self.tasks.abort_all();
            if !matches!(self.handle.status(), RunStatus::Faulted { .. }) {
                self.handle.shared.status.send_replace(RunStatus::Stopped);
            }
        }
    }
}

/// Bounded run-handle failures.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RunHandleError {
    /// Queue capacity or shutdown deadline was zero.
    #[error("invalid run task configuration")]
    InvalidConfiguration,
    /// Shutdown has closed command intake.
    #[error("run is shutting down")]
    ShuttingDown,
    /// Owned worker has stopped.
    #[error("run has stopped")]
    Stopped,
    /// Runtime uncertainty faulted the run.
    #[error("run faulted: {code}")]
    Faulted {
        /// Stable fault code.
        code: &'static str,
    },
    /// Worker intake or reply path closed unexpectedly.
    #[error("run intake closed")]
    IntakeClosed,
    /// Coordinator rejected or faulted the submission.
    #[error(transparent)]
    Coordinator(CommitCoordinatorError),
}

struct Shared {
    sender: Mutex<Option<mpsc::Sender<RunCommand>>>,
    shutting_down: AtomicBool,
    status: watch::Sender<RunStatus>,
}

struct RunCommand {
    env: TransitionEnv,
    input: KernelInput,
    reply: oneshot::Sender<Result<CommitOutcome, RunHandleError>>,
}

async fn run_worker(
    mut coordinator: CommitCoordinator,
    mut receiver: mpsc::Receiver<RunCommand>,
    shared: Arc<Shared>,
) {
    while let Some(command) = receiver.recv().await {
        if shared.shutting_down.load(Ordering::Acquire) {
            let _ = command.reply.send(Err(RunHandleError::ShuttingDown));
            continue;
        }
        let result = coordinator
            .submit(command.env, command.input)
            .await
            .map_err(RunHandleError::Coordinator);
        let fault_code = match &result {
            Ok(outcome) => outcome.fault.map(|fault| fault.code),
            Err(RunHandleError::Coordinator(
                CommitCoordinatorError::BoundaryFault { code }
                | CommitCoordinatorError::Faulted { code },
            )) => Some(*code),
            _ => None,
        };
        let _ = command.reply.send(result);
        if let Some(code) = fault_code {
            shared.shutting_down.store(true, Ordering::Release);
            if let Ok(mut sender) = shared.sender.lock() {
                sender.take();
            }
            receiver.close();
            shared.status.send_replace(RunStatus::Faulted { code });
        }
    }
    if !matches!(*shared.status.borrow(), RunStatus::Faulted { .. }) {
        shared.status.send_replace(RunStatus::Stopped);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use finstack_ai_kernel::{
        AcceptRun, AllocatedIds, BudgetPropagation, CancellationPropagation, Digest, Id, IdTag,
        PrincipalPropagation, PrincipalRef, RunAccepted, RunLimits, RunPropagationPolicy,
        RunRelation, RunSecurityContext, Timestamp,
    };
    use tokio::sync::Notify;

    use super::*;
    use crate::{
        JournalStore, LoadRequest, LoadedSession, PortFuture, SnapshotReceipt, SnapshotRequest,
        StoreError, StoreHealth,
    };

    struct BlockingStore {
        calls: AtomicUsize,
        started: AtomicUsize,
        release: Arc<Notify>,
        block_every_call: bool,
    }

    impl BlockingStore {
        fn new(block_every_call: bool) -> Self {
            Self {
                calls: AtomicUsize::new(0),
                started: AtomicUsize::new(0),
                release: Arc::new(Notify::new()),
                block_every_call,
            }
        }
    }

    impl JournalStore for BlockingStore {
        fn append(
            &self,
            _request: finstack_ai_kernel::AppendRequest,
        ) -> PortFuture<Result<finstack_ai_kernel::CommittedBatch, StoreError>> {
            let call = self.calls.fetch_add(1, Ordering::AcqRel);
            self.started.fetch_add(1, Ordering::Release);
            let release = Arc::clone(&self.release);
            let block = self.block_every_call || call == 0;
            Box::pin(async move {
                if block {
                    release.notified().await;
                }
                Err(StoreError::Unavailable {
                    reason_code: "test_store_unavailable",
                })
            })
        }

        fn load(&self, request: LoadRequest) -> PortFuture<Result<LoadedSession, StoreError>> {
            Box::pin(async move {
                Ok(LoadedSession {
                    session_id: request.session_id,
                    head_sequence: 0,
                    committed_batches: Arc::from([]),
                    snapshot: None,
                })
            })
        }

        fn write_snapshot(
            &self,
            _request: SnapshotRequest,
        ) -> PortFuture<Result<SnapshotReceipt, StoreError>> {
            Box::pin(async {
                Err(StoreError::Unavailable {
                    reason_code: "not_used",
                })
            })
        }

        fn health(&self) -> PortFuture<Result<StoreHealth, StoreError>> {
            Box::pin(async {
                Ok(StoreHealth {
                    ready: true,
                    durable: false,
                    detail: Arc::from("test"),
                })
            })
        }
    }

    fn id<T: IdTag>(ordinal: u64) -> Id<T> {
        let mut bytes = [0_u8; 16];
        bytes[6] = 0x70;
        bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        Id::from_bytes(bytes)
    }

    fn accepted() -> RunAccepted {
        let run_id = id(3);
        RunAccepted::try_new(
            run_id,
            RunRelation::root(run_id).expect("relation"),
            RunSecurityContext::try_new(
                "tenant-a",
                PrincipalRef::try_new("issuer", "subject", Some("tenant-a")).expect("principal"),
                "oidc",
                "high",
                "policy-v1",
                "decision-v1",
                None,
            )
            .expect("security"),
            None,
            RunLimits::empty(),
            RunPropagationPolicy {
                cancellation: CancellationPropagation::Cascade,
                deadline: finstack_ai_kernel::DeadlinePropagation::MinimumOfParentAndChild,
                budget: BudgetPropagation::SharedScope,
                principal: PrincipalPropagation::Inherit,
            },
            Digest::raw_json(b"agent"),
            None,
        )
        .expect("accepted")
    }

    fn command(ordinal: u64) -> (TransitionEnv, KernelInput) {
        (
            TransitionEnv {
                now: Timestamp::from_unix_ms(1_000).expect("timestamp"),
                ids: AllocatedIds::try_new(
                    vec![id(ordinal)],
                    vec![id(ordinal)],
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    vec![id(ordinal)],
                    Vec::new(),
                )
                .expect("ids"),
            },
            KernelInput::AcceptRun(AcceptRun {
                session_id: id(1),
                lane_id: id(2),
                accepted: accepted(),
            }),
        )
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("runtime")
    }

    #[test]
    fn bounded_channel_applies_backpressure() {
        runtime().block_on(async {
            let store = Arc::new(BlockingStore::new(false));
            let mut owner = RunTaskOwner::spawn(
                CommitCoordinator::new(store.clone()),
                RunTaskConfig {
                    command_capacity: 1,
                    shutdown_deadline: Duration::from_millis(100),
                },
            )
            .expect("owner");
            let handle = owner.handle();
            let first_handle = handle.clone();
            let first = tokio::spawn(async move {
                let (env, input) = command(10);
                first_handle.submit(env, input).await
            });
            while store.started.load(Ordering::Acquire) == 0 {
                tokio::task::yield_now().await;
            }
            let second_handle = handle.clone();
            let second = tokio::spawn(async move {
                let (env, input) = command(11);
                second_handle.submit(env, input).await
            });
            tokio::task::yield_now().await;
            let (env, input) = command(12);
            assert!(
                tokio::time::timeout(Duration::from_millis(5), handle.submit(env, input),)
                    .await
                    .is_err()
            );
            store.release.notify_waiters();
            let _ = first.await.expect("first task");
            let _ = second.await.expect("second task");
            owner.shutdown().await;
            assert_eq!(handle.status(), RunStatus::Stopped);
        });
    }

    #[test]
    fn shutdown_is_idempotent_and_handle_drop_does_not_cancel() {
        runtime().block_on(async {
            let store = Arc::new(BlockingStore::new(false));
            let mut owner = RunTaskOwner::spawn(
                CommitCoordinator::new(store),
                RunTaskConfig {
                    command_capacity: 1,
                    shutdown_deadline: Duration::from_millis(100),
                },
            )
            .expect("owner");
            let handle = owner.handle();
            let clone = handle.clone();
            drop(clone);
            assert_eq!(handle.status(), RunStatus::Running);
            handle.shutdown();
            handle.shutdown();
            assert_eq!(handle.status(), RunStatus::ShuttingDown);
            let (env, input) = command(20);
            assert_eq!(
                handle.submit(env, input).await.expect_err("closed"),
                RunHandleError::ShuttingDown
            );
            owner.shutdown().await;
            owner.shutdown().await;
            assert_eq!(handle.status(), RunStatus::Stopped);
        });
    }

    #[test]
    fn shutdown_deadline_aborts_active_store_wait() {
        runtime().block_on(async {
            let store = Arc::new(BlockingStore::new(true));
            let mut owner = RunTaskOwner::spawn(
                CommitCoordinator::new(store.clone()),
                RunTaskConfig {
                    command_capacity: 1,
                    shutdown_deadline: Duration::from_millis(5),
                },
            )
            .expect("owner");
            let handle = owner.handle();
            let submit_handle = handle.clone();
            let submit = tokio::spawn(async move {
                let (env, input) = command(30);
                submit_handle.submit(env, input).await
            });
            while store.started.load(Ordering::Acquire) == 0 {
                tokio::task::yield_now().await;
            }
            owner.shutdown().await;
            assert_eq!(handle.status(), RunStatus::Stopped);
            assert!(matches!(
                submit.await.expect("submit task"),
                Err(RunHandleError::Stopped | RunHandleError::ShuttingDown)
            ));
        });
    }
}
