//! Real `SQLite` contention through the asynchronous evaluation store boundary.
use super::*;
use crate::{EvalSpec, async_store::AsyncEvalStore};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

static CONTENDED: tokio::sync::Notify = tokio::sync::Notify::const_new();

#[tokio::test(flavor = "current_thread")]
async fn eval_store_keeps_executor_live_and_runner_owned_during_contention() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("eval.sqlite");
    let store = Arc::new(SqliteEvalStore::try_open(&path).expect("store"));
    store
        .inner
        .lock()
        .expect("connection")
        .connection
        .busy_handler(Some(|_| {
            CONTENDED.notify_one();
            std::thread::sleep(Duration::from_millis(1));
            true
        }))
        .expect("busy handler");
    let async_store = AsyncEvalStore::new(store.clone());
    let lease = async_store.acquire_runner().await.expect("lease");
    let spec: EvalSpec = serde_json::from_str(include_str!(
        "../../../../fixtures/compatibility/eval/v1/valid--spec-minimal.json"
    ))
    .expect("spec");
    let blocker = Connection::open(&path).expect("blocker");
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
        assert!(matches!(store.acquire_runner(), Err(error) if error.code() == EVAL_RUNNER_BUSY));
        release.send(()).expect("release from executor");
    };
    let (frozen, ()) = tokio::join!(async_store.freeze(&spec), heartbeat);
    thread.join().expect("joined blocker");
    assert!(
        !timed_out.load(Ordering::Acquire),
        "executor could not release the database lock"
    );
    assert_eq!(frozen.expect("freeze").spec, spec);
    drop(lease);
    let reopened = SqliteEvalStore::try_open(path).expect("reopen");
    let _lease = reopened.acquire_runner().expect("new owner");
    assert_eq!(
        reopened
            .snapshot()
            .expect("snapshot")
            .frozen
            .expect("frozen")
            .spec,
        spec
    );
}
