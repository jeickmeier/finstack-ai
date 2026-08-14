//! Warning-only restore-time and storage-growth benches (PR-040-A04).
//!
//! Budgets remain unratified until PR-063.

use std::future::Future;
use std::path::Path;
use std::task::{Context, Poll, Waker};
use std::thread;
use std::time::Duration;

use criterion::{Criterion, criterion_group, criterion_main};
use finstack_ai_kernel::{
    AppendRequest, AuthorizationEvidence, Digest, ExternalCommandKind, ExternalCommandRejected,
    ExternalCommandTarget, Id, IdTag, LaneTag, PrincipalRef, RECORD_FORMAT_VERSION,
    RECORD_KIND_VERSION, RecordBody, RecordDraft, RecordTag, RunTag, SessionTag, Timestamp,
};
use finstack_ai_runtime::{JournalStore, LoadRequest};
use finstack_ai_store_sqlite::{
    DEFAULT_BUSY_TIMEOUT, SqliteDurability, SqliteJournalStore, SqliteStoreConfig,
    SqliteStoreLimits,
};
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

fn draft(record_ordinal: u64) -> RecordDraft {
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
        id::<SessionTag>(1),
        id::<LaneTag>(101),
        Some(id::<RunTag>(201)),
        Timestamp::from_unix_ms(i64::try_from(record_ordinal).expect("timestamp"))
            .expect("timestamp"),
        Vec::new(),
        RecordBody::ExternalCommandRejected(rejection),
    )
    .expect("draft")
}

fn request(batch_ordinal: u64, expected_sequence: u64) -> AppendRequest {
    AppendRequest::try_new(
        id(batch_ordinal),
        id::<SessionTag>(1),
        expected_sequence,
        vec![draft(batch_ordinal)],
    )
    .expect("request")
}

fn seed(path: &Path, records: u64) {
    let store = SqliteJournalStore::try_open(SqliteStoreConfig {
        path: path.to_path_buf(),
        durability: SqliteDurability::Durable,
        limits: SqliteStoreLimits {
            sessions: 1,
            batches_per_session: usize::try_from(records).expect("batches"),
            records_per_session: usize::try_from(records).expect("records"),
            snapshot_bytes: 64,
        },
        busy_timeout: DEFAULT_BUSY_TIMEOUT,
    })
    .expect("open");
    for sequence in 1..=records {
        block_on(store.append(request(sequence, sequence))).expect("append");
    }
}

fn restore_and_growth(criterion: &mut Criterion) {
    const RECORDS: u64 = 64;
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("journal.sqlite");
    seed(&path, RECORDS);
    let db_bytes = std::fs::metadata(&path).expect("metadata").len();
    let wal_bytes =
        std::fs::metadata(path.with_file_name("journal.sqlite-wal")).map_or(0, |meta| meta.len());
    eprintln!(
        "sqlite_storage_growth records={RECORDS} db_bytes={db_bytes} wal_bytes={wal_bytes} (warning-only; PR-063 owns budgets)"
    );

    criterion.bench_function("sqlite_restore_64_records", |bencher| {
        bencher.iter(|| {
            let store = SqliteJournalStore::try_open(SqliteStoreConfig {
                path: path.clone(),
                durability: SqliteDurability::Durable,
                limits: SqliteStoreLimits {
                    sessions: 1,
                    batches_per_session: 64,
                    records_per_session: 64,
                    snapshot_bytes: 64,
                },
                busy_timeout: Duration::from_secs(1),
            })
            .expect("reopen");
            let loaded = block_on(store.load(LoadRequest {
                session_id: id::<SessionTag>(1),
            }))
            .expect("load");
            assert_eq!(loaded.head_sequence, RECORDS);
        });
    });
}

criterion_group!(benches, restore_and_growth);
criterion_main!(benches);
