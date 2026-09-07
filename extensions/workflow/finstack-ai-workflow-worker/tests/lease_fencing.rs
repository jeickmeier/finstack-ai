//! Acquisition-fence and upgrade proofs for both worker stores.
use std::sync::Arc;

use finstack_ai_kernel::{LaneId, RunId, SessionId, Timestamp};
use finstack_ai_workflow_worker::{
    MemoryWorkerStore, SqliteWorkerStore, WakeIndexStore, WakeReason, WakeRow, WorkerError,
};

fn time(ms: i64) -> Timestamp {
    Timestamp::from_unix_ms(ms).expect("time")
}
fn row() -> WakeRow {
    WakeRow {
        tenant_scope: Arc::from("tenant"),
        session_id: SessionId::from_bytes([1; 16]),
        lane_id: LaneId::from_bytes([2; 16]),
        run_id: RunId::from_bytes([3; 16]),
        workflow_kind: Arc::from("test"),
        reason: WakeReason::Timer,
        wake_at: Some(time(1_000)),
        expires_at: None,
        pending_id: Arc::from("timer"),
        leased_by: None,
        lease_id: None,
        lease_expires_at: None,
        attempts: 0,
    }
}

fn stale_owner_cannot_mutate(first: &dyn WakeIndexStore, second: &dyn WakeIndexStore) {
    let row = row();
    first.upsert(&row, None).expect("initial hint");
    let a = first
        .try_claim("tenant", row.session_id, "same-worker", time(1_000), 10)
        .expect("claim")
        .expect("a");
    assert!(!first.renew(&a, time(1_010), 100).expect("expired renewal"));
    let b = second
        .try_claim("tenant", row.session_id, "same-worker", time(1_011), 1_000)
        .expect("reclaim")
        .expect("b");
    assert_ne!(a.id, b.id, "same worker and row must still get a new fence");
    let before = second.load_tenant("tenant").expect("before");
    assert_eq!(
        first.record_failure(&a, time(1_012)),
        Err(WorkerError::LeaseLost)
    );
    assert!(!first.release(&a).expect("stale release"));
    assert!(!first.renew(&a, time(1_012), 1_000).expect("stale renewal"));
    for lease in [None, Some(&a)] {
        assert_eq!(first.upsert(&row, lease), Err(WorkerError::LeaseLost));
        assert_eq!(
            first.delete("tenant", row.session_id, lease),
            Err(WorkerError::LeaseLost)
        );
    }
    assert_eq!(second.load_tenant("tenant").expect("after"), before);
    assert!(
        first
            .try_claim("tenant", row.session_id, "c", time(1_013), 1_000)
            .expect("third claim")
            .is_none()
    );
    second
        .upsert(&row, Some(&b))
        .expect("current owner reparks");
    // A completed claim cannot delete, overwrite, or back off the newly unleased hint.
    assert_eq!(
        second.delete("tenant", row.session_id, Some(&b)),
        Err(WorkerError::LeaseLost)
    );
    assert_eq!(
        second.record_failure(&b, time(9_000)),
        Err(WorkerError::LeaseLost)
    );
    let c = first
        .try_claim("tenant", row.session_id, "c", time(1_014), 1_000)
        .expect("claim c")
        .expect("c");
    first
        .record_failure(&c, time(2_000))
        .expect("current failure");
    assert_eq!(first.load_tenant("tenant").expect("backoff")[0].attempts, 1);

    let mut exhausted = row.clone();
    exhausted.attempts = u32::MAX;
    first.upsert(&exhausted, None).expect("exhausted hint");
    let claim = first
        .try_claim("tenant", row.session_id, "d", time(2_001), 100)
        .expect("claim exhausted hint")
        .expect("owner");
    let before = first.load_tenant("tenant").expect("before overflow");
    assert_eq!(
        first.record_failure(&claim, time(3_000)),
        Err(WorkerError::StoreIntegrity {
            code: "wake_attempts_overflow",
        })
    );
    assert_eq!(first.load_tenant("tenant").expect("after overflow"), before);
    assert!(
        first
            .release(&claim)
            .expect("overflow must retain the owner's fence")
    );
}

#[test]
fn memory_fences_every_wake_mutation() {
    let store = MemoryWorkerStore::new();
    stale_owner_cannot_mutate(&store, &store);
}

#[test]
fn independent_sqlite_connections_fence_every_wake_mutation() {
    let dir = tempfile::tempdir().expect("directory");
    let path = dir.path().join("worker.sqlite");
    let a = SqliteWorkerStore::try_open(&path).expect("a");
    let b = SqliteWorkerStore::try_open(&path).expect("b");
    stale_owner_cannot_mutate(&a, &b);
}

#[test]
fn schema_upgrade_preserves_hints_and_invalidates_legacy_leases() {
    let dir = tempfile::tempdir().expect("directory");
    let path = dir.path().join("worker.sqlite");
    let store = SqliteWorkerStore::try_open(&path).expect("store");
    store.upsert(&row(), None).expect("hint");
    drop(store);
    let conn = rusqlite::Connection::open(&path).expect("connection");
    conn.execute_batch(
        "ALTER TABLE finstack_workflow_worker_wake DROP COLUMN lease_id;
        UPDATE finstack_workflow_worker_schema SET version = 2;
        UPDATE finstack_workflow_worker_wake SET leased_by = 'old', lease_expires_unix_ms = 9000;",
    )
    .expect("v2 fixture");
    drop(conn);
    let store = SqliteWorkerStore::try_open(&path).expect("upgrade");
    assert_eq!(store.load_tenant("tenant").expect("hints"), vec![row()]);
    drop(store);
    SqliteWorkerStore::try_open(&path).expect("reopen v3");
}
