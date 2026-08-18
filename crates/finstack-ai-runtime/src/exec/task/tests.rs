use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use finstack_ai_kernel::{
    AcceptRun, AllocatedIds, BudgetPropagation, CancellationPropagation, Digest, Id, IdTag,
    KernelInput, PrincipalPropagation, PrincipalRef, RunAccepted, RunLimits, RunPropagationPolicy,
    RunRelation, RunSecurityContext, Timestamp, TransitionEnv,
};
use tokio::sync::Notify;

use super::fault::result_fault_code;
use super::*;
use crate::{
    CommitCoordinator, EventHubConfig, JournalStore, LoadRequest, LoadedSession, PortFuture,
    RunHandleError, RunStatus, RunTaskConfig, ShutdownOutcome, SnapshotReceipt, SnapshotRequest,
    StoreError, StoreHealth,
};

/// The other half of the folded-allocation fix (`stage_settlement.rs`'s
/// `folded_allocation_error`): `RunHandleError::Middleware` must never tear
/// the worker down. `ToolSettlement` deliberately still does — a genuine
/// tool-settlement failure is a worker fault — which is exactly why a
/// middleware fold's rejection has to be re-classified before it gets here.
#[test]
fn a_middleware_failure_is_not_a_worker_fault() {
    assert_eq!(
        result_fault_code(&Err(RunHandleError::Middleware {
            code: Arc::from("stage_allocation_model_request_contract_mismatch"),
        })),
        None,
        "a middleware fold failure must fail only the run, not the worker"
    );
    assert_eq!(
        result_fault_code(&Err(RunHandleError::ToolSettlement {
            code: "stage_allocation_model_request_contract_mismatch",
        })),
        Some(Arc::from(
            "stage_allocation_model_request_contract_mismatch"
        )),
        "tool settlement stays a worker fault, so the re-classification is load-bearing"
    );
}

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
        Box::pin(async move { Ok(LoadedSession::empty(request.session_id)) })
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
                event_hub: EventHubConfig {
                    source_capacity: 8,
                    max_subscribers: 4,
                },
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
                event_hub: EventHubConfig {
                    source_capacity: 8,
                    max_subscribers: 4,
                },
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
                event_hub: EventHubConfig {
                    source_capacity: 8,
                    max_subscribers: 4,
                },
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

#[test]
fn repeated_idle_owners_join_every_owned_task_without_abort() {
    runtime().block_on(async {
        for _ in 0..128 {
            let store = Arc::new(BlockingStore::new(false));
            let mut owner = RunTaskOwner::spawn(
                CommitCoordinator::new(store),
                RunTaskConfig {
                    command_capacity: 1,
                    event_hub: EventHubConfig {
                        source_capacity: 1,
                        max_subscribers: 1,
                    },
                    shutdown_deadline: Duration::from_millis(100),
                },
            )
            .expect("owner");
            let handle = owner.handle();

            let report = owner.shutdown().await;

            assert_eq!(report.outcome, ShutdownOutcome::Graceful);
            assert_eq!(report.signalled_effects, 0);
            assert_eq!(report.aborted_tasks, 0);
            assert_eq!(handle.status(), RunStatus::Stopped);
        }
    });
}
