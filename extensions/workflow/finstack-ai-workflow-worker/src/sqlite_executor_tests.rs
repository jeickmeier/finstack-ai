//! A real `SQLite` write lock must not block the current-thread executor.
use super::*;
use crate::{FireStore, InboxStore, WorkerBuilder};
use finstack_ai_runtime::ids::ExternalClock;
use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};
use finstack_ai_workflow_local::MemoryCronStore;
use std::sync::atomic::{AtomicBool, Ordering};

static CONTENDED: tokio::sync::Notify = tokio::sync::Notify::const_new();

#[tokio::test(flavor = "current_thread")]
async fn tick_remains_responsive_during_sqlite_contention() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("worker.sqlite");
    let store = Arc::new(SqliteWorkerStore::try_open(&path).expect("store"));
    let now = Timestamp::from_unix_ms(1_000).expect("time");
    let row = WakeRow {
        tenant_scope: Arc::from("tenant"),
        session_id: SessionId::from_bytes([1; 16]),
        lane_id: LaneId::from_bytes([2; 16]),
        run_id: RunId::from_bytes([3; 16]),
        workflow_kind: Arc::from("missing"),
        reason: WakeReason::Runnable,
        wake_at: None,
        expires_at: None,
        pending_id: Arc::from("run"),
        leased_by: None,
        lease_id: None,
        lease_expires_at: None,
        attempts: 0,
    };
    store.upsert(&row, None).expect("hint");
    store
        .conn
        .lock()
        .expect("connection")
        .busy_handler(Some(|_| {
            CONTENDED.notify_one();
            std::thread::sleep(Duration::from_millis(1));
            true
        }))
        .expect("busy handler");
    let journal = Arc::new(
        MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions: 1,
            batches_per_session: 16,
            records_per_session: 64,
            snapshot_bytes: 4096,
        })
        .expect("journal"),
    );
    let worker = WorkerBuilder::new(
        journal,
        Arc::new(MemoryCronStore::new()),
        store.clone() as Arc<dyn WakeIndexStore>,
        store.clone() as Arc<dyn FireStore>,
        store.clone() as Arc<dyn InboxStore>,
    )
    .clock(ExternalClock::new(now))
    .build()
    .expect("worker");
    let blocker = Connection::open(path).expect("blocker");
    blocker.execute_batch("BEGIN IMMEDIATE").expect("lock");
    let (release, released) = std::sync::mpsc::channel();
    let timed_out = Arc::new(AtomicBool::new(false));
    let fallback = Arc::clone(&timed_out);
    let thread = std::thread::spawn(move || {
        if released.recv_timeout(Duration::from_secs(5)).is_err() {
            fallback.store(true, Ordering::Release);
        }
        blocker.execute_batch("ROLLBACK").expect("unlock");
    });
    let heartbeat = async {
        CONTENDED.notified().await;
        tokio::time::sleep(Duration::from_millis(1)).await;
        release.send(()).expect("release from executor");
    };
    let (tick, ()) = tokio::join!(worker.tick(), heartbeat);
    thread.join().expect("joined blocker");
    assert!(
        !timed_out.load(Ordering::Acquire),
        "executor could not release the database lock"
    );
    assert_eq!(
        tick.expect("tick").failures,
        1,
        "unknown kind backs off after claiming"
    );
    assert_eq!(store.load_tenant("tenant").expect("hint")[0].attempts, 1);
}
