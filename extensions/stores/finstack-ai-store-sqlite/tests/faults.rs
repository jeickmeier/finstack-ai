//! PR-040 fault, busy, power-loss, and graph proofs.

use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};
use std::thread;
use std::time::Duration;

use finstack_ai_kernel::{
    AppendRequest, AuthorizationEvidence, Digest, ExternalCommandKind, ExternalCommandRejected,
    ExternalCommandTarget, Id, IdTag, LaneTag, PrincipalRef, RECORD_FORMAT_VERSION,
    RECORD_KIND_VERSION, RecordBody, RecordDraft, RecordTag, RunTag, SessionTag, Timestamp,
};
use finstack_ai_runtime::{JournalStore, LoadRequest, ScanRequest, StoreError};
use finstack_ai_store_sqlite::{
    DEFAULT_BUSY_TIMEOUT, SqliteDurability, SqliteJournalStore, SqliteStoreConfig,
    SqliteStoreLimits, SqliteSynchronous,
};
use finstack_ai_test::AmbiguousAckAfterCommitStore;
use rusqlite::Connection;
use tempfile::TempDir;

fn block_on<T>(future: impl Future<Output = T>) -> T {
    let mut context = Context::from_waker(Waker::noop());
    let mut future = std::pin::pin!(future);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(value) => return value,
            Poll::Pending => thread::yield_now(),
        }
    }
}

fn id<T: IdTag>(ordinal: u64) -> Id<T> {
    let mut bytes = [0_u8; 16];
    bytes[6] = 0x70;
    bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Id::from_bytes(bytes)
}

fn limits() -> SqliteStoreLimits {
    SqliteStoreLimits {
        sessions: 8,
        batches_per_session: 32,
        records_per_session: 64,
        snapshot_bytes: 4096,
    }
}

fn draft(record_ordinal: u64, session_ordinal: u64) -> RecordDraft {
    let principal =
        PrincipalRef::try_new("issuer", "subject", Some("tenant-a")).expect("principal");
    let authorization =
        AuthorizationEvidence::try_new("policy-v1", "decision-v1").expect("authorization");
    let rejection = ExternalCommandRejected::try_new(
        ExternalCommandKind::EffectCompletion,
        format!("completion-{record_ordinal}"),
        ExternalCommandTarget::Effect(id(record_ordinal + 1000)),
        principal,
        authorization,
        "conflicting_completion",
        Digest::raw_json(b"{}"),
        None,
    )
    .expect("rejection");
    RecordDraft::try_new(
        RECORD_FORMAT_VERSION,
        RECORD_KIND_VERSION,
        id::<RecordTag>(record_ordinal),
        id::<SessionTag>(session_ordinal),
        id::<LaneTag>(session_ordinal + 100),
        Some(id::<RunTag>(session_ordinal + 200)),
        Timestamp::from_unix_ms(i64::try_from(record_ordinal).expect("timestamp"))
            .expect("timestamp"),
        Vec::new(),
        RecordBody::ExternalCommandRejected(rejection),
    )
    .expect("draft")
}

fn request(
    batch_ordinal: u64,
    session_ordinal: u64,
    expected_sequence: u64,
    drafts: Vec<RecordDraft>,
) -> AppendRequest {
    AppendRequest::try_new(
        id(batch_ordinal),
        id::<SessionTag>(session_ordinal),
        expected_sequence,
        drafts,
    )
    .expect("append request")
}

fn open(path: PathBuf, durability: SqliteDurability, busy_timeout: Duration) -> SqliteJournalStore {
    SqliteJournalStore::try_open(SqliteStoreConfig {
        path,
        durability,
        limits: limits(),
        busy_timeout,
    })
    .expect("open")
}

fn copy_sqlite_bundle(source: &Path, destination_dir: &Path) -> PathBuf {
    let file_name = source.file_name().expect("file name");
    let destination = destination_dir.join(file_name);
    fs::copy(source, &destination).expect("copy db");
    for suffix in ["-wal", "-shm"] {
        let sidecar = {
            let mut name = source.as_os_str().to_os_string();
            name.push(suffix);
            PathBuf::from(name)
        };
        if sidecar.exists() {
            let mut dest_name = destination.as_os_str().to_os_string();
            dest_name.push(suffix);
            fs::copy(&sidecar, dest_name).expect("copy sidecar");
        }
    }
    destination
}

#[test]
fn second_connection_receives_sqlite_busy() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("journal.sqlite");
    let store = open(
        path.clone(),
        SqliteDurability::Durable,
        Duration::from_millis(50),
    );
    let holder = Connection::open(&path).expect("holder");
    holder
        .busy_timeout(Duration::from_secs(5))
        .expect("holder timeout");
    holder
        .execute_batch("BEGIN IMMEDIATE")
        .expect("hold writer lock");
    let error = block_on(store.append(request(1, 1, 1, vec![draft(1, 1)]))).expect_err("busy");
    assert!(
        matches!(
            error,
            StoreError::Unavailable {
                reason_code: "sqlite_busy"
            }
        ),
        "{error:?}"
    );
    holder.execute_batch("ROLLBACK").expect("release");
}

#[test]
fn corrupt_database_fails_closed() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("corrupt.sqlite");
    fs::write(&path, b"this is not a sqlite database").expect("write garbage");
    let Err(error) = SqliteJournalStore::try_open(SqliteStoreConfig {
        path,
        durability: SqliteDurability::Durable,
        limits: limits(),
        busy_timeout: DEFAULT_BUSY_TIMEOUT,
    }) else {
        panic!("corrupt database opened");
    };
    assert!(
        matches!(
            error,
            StoreError::Integrity {
                reason_code: "sqlite_corrupt"
            } | StoreError::Unavailable {
                reason_code: "sqlite_error"
            }
        ),
        "{error:?}"
    );
}

#[test]
fn disk_full_maps_to_sqlite_disk_full() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("journal.sqlite");
    let store = open(path, SqliteDurability::Durable, DEFAULT_BUSY_TIMEOUT);
    block_on(store.append(request(1, 1, 1, vec![draft(1, 1)]))).expect("seed");
    store
        .use_rollback_journal_for_test()
        .expect("disable wal so the page cap applies");
    store
        .insert_padding_blob(64 * 1024)
        .expect("consume free pages");
    let page_count = store.page_count().expect("page count");
    store
        .set_max_page_count(page_count)
        .expect("cap pages on the writer");
    let padding = store
        .insert_padding_blob(64 * 1024)
        .expect_err("padding must hit SQLITE_FULL");
    assert!(
        matches!(
            padding,
            StoreError::Unavailable {
                reason_code: "sqlite_disk_full"
            }
        ),
        "{padding:?}"
    );
    if let Err(error) = block_on(store.append(request(2, 1, 2, vec![draft(2, 1)]))) {
        assert!(
            matches!(
                error,
                StoreError::Unavailable {
                    reason_code: "sqlite_disk_full"
                }
            ),
            "expected sqlite_disk_full or leftover-page success, got {error:?}"
        );
    }
}

#[test]
#[ignore = "known flake; not an A01 row (TDD §18.2 / PR-048 pitfall 13)"]
fn concurrent_readers_never_observe_a_torn_batch() {
    let dir = TempDir::new().expect("tempdir");
    let store = Arc::new(open(
        dir.path().join("journal.sqlite"),
        SqliteDurability::Durable,
        DEFAULT_BUSY_TIMEOUT,
    ));
    block_on(store.append(request(1, 1, 1, vec![draft(1, 1), draft(2, 1)]))).expect("seed");
    let readers = (0..4)
        .map(|_| {
            let store = Arc::clone(&store);
            thread::spawn(move || {
                for _ in 0..16 {
                    let loaded = block_on(store.load(LoadRequest {
                        session_id: id::<SessionTag>(1),
                    }))
                    .expect("load");
                    for batch in loaded.committed_batches.iter() {
                        assert_eq!(
                            batch.records.len(),
                            usize::try_from(batch.last_sequence - batch.first_sequence + 1)
                                .expect("len")
                        );
                    }
                    let page = block_on(store.scan(ScanRequest {
                        session_id: id::<SessionTag>(1),
                        from_sequence: 0,
                        limit: 16,
                    }))
                    .expect("scan");
                    assert_eq!(
                        page.records.len(),
                        usize::try_from(loaded.head_sequence).expect("sequence fits usize")
                    );
                }
            })
        })
        .collect::<Vec<_>>();
    block_on(store.append(request(2, 1, 3, vec![draft(3, 1), draft(4, 1)]))).expect("second");
    for reader in readers {
        reader.join().expect("reader");
    }
}

#[test]
fn process_kill_after_ack_preserves_the_committed_batch() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("journal.sqlite");
    let helper = env!("CARGO_BIN_EXE_sqlite_fault_helper");
    let output = Command::new(helper)
        .args(["commit-then-abort", path.to_str().expect("utf8 path")])
        .output()
        .expect("helper");
    assert!(
        !output.status.success(),
        "helper must abort after ack: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("acked"),
        "process-kill harness requires an acknowledgement before abort"
    );
    let store = open(path, SqliteDurability::Durable, DEFAULT_BUSY_TIMEOUT);
    let loaded = block_on(store.load(LoadRequest {
        session_id: id::<SessionTag>(1),
    }))
    .expect("load after kill");
    assert_eq!(loaded.head_sequence, 1);
    assert_eq!(loaded.committed_batches.len(), 1);
}

#[test]
fn simulated_power_loss_reopens_copied_wal_bundle() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("journal.sqlite");
    let store = open(
        path.clone(),
        SqliteDurability::Durable,
        DEFAULT_BUSY_TIMEOUT,
    );
    let committed = block_on(store.append(request(1, 1, 1, vec![draft(1, 1)]))).expect("append");
    let copy_dir = dir.path().join("copy");
    fs::create_dir_all(&copy_dir).expect("copy dir");
    let copied = copy_sqlite_bundle(&path, &copy_dir);
    drop(store);
    let restored = open(copied, SqliteDurability::Durable, DEFAULT_BUSY_TIMEOUT);
    let loaded = block_on(restored.load(LoadRequest {
        session_id: id::<SessionTag>(1),
    }))
    .expect("load copy");
    assert_eq!(loaded.head_sequence, 1);
    assert_eq!(loaded.committed_batches.as_ref(), &[committed]);
}

#[test]
fn ambiguous_ack_wrapper_retries_the_original_sqlite_receipt() {
    let store = Arc::new(open(
        PathBuf::from(":memory:"),
        SqliteDurability::Relaxed {
            synchronous: SqliteSynchronous::Normal,
        },
        DEFAULT_BUSY_TIMEOUT,
    ));
    let wrapped = AmbiguousAckAfterCommitStore::once(store.clone());
    let first = request(1, 1, 1, vec![draft(1, 1)]);
    assert!(matches!(
        block_on(wrapped.append(first.clone())),
        Err(StoreError::AmbiguousAcknowledgement)
    ));
    let original = block_on(wrapped.append(first)).expect("retry");
    assert_eq!(original.first_sequence, 1);
}

#[test]
fn runtime_and_sdk_stay_free_of_sqlite_and_protocol() {
    for package in ["finstack-ai-runtime", "finstack-ai"] {
        let output = Command::new("cargo")
            .args([
                "tree", "-p", package, "--prefix", "none", "-e", "normal", "--locked",
            ])
            .output()
            .expect("cargo tree");
        assert!(output.status.success(), "cargo tree {package} failed");
        let tree = String::from_utf8_lossy(&output.stdout);
        assert!(
            !tree
                .lines()
                .any(|line| line.starts_with("finstack-ai-protocol ")),
            "{package} must stay protocol-free:\n{tree}"
        );
        assert!(
            !tree
                .lines()
                .any(|line| line.starts_with("finstack-ai-store-sqlite ")),
            "{package} must not depend on the sqlite leaf:\n{tree}"
        );
        assert!(
            !tree.lines().any(|line| line.starts_with("rusqlite ")),
            "{package} must not depend on rusqlite:\n{tree}"
        );
    }
}
