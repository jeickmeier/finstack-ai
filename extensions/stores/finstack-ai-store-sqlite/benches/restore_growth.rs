//! Warning-only restore-time and storage-growth benches (`SQLite` restore-growth benchmark).
//!
//! Budgets remain unratified until performance-budget calibration.

use std::future::Future;
use std::hint::black_box;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll, Waker};
use std::thread;
use std::time::{Duration, Instant};

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use finstack_ai_kernel::{
    AppendRequest, AuthorizationEvidence, BudgetPropagation, CancellationPropagation,
    DeadlinePropagation, Digest, EventTag, ExternalCommandKind, ExternalCommandRejected,
    ExternalCommandTarget, Id, IdTag, LaneTag, PrincipalPropagation, PrincipalRef,
    RECORD_FORMAT_VERSION, RECORD_KIND_VERSION, RecordBody, RecordDraft, RecordTag, RunAccepted,
    RunLimits, RunPropagationPolicy, RunRelation, RunSecurityContext, RunTag, SessionTag,
    Timestamp,
};
use finstack_ai_runtime::{
    CommitCoordinator, JournalStore, LoadRequest, SCAN_PAGE_MAX_RECORDS, ScanRequest,
    StateSnapshotRequest,
};
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

fn accept_request() -> AppendRequest {
    let run_id = id::<RunTag>(201);
    let accepted = RunAccepted::try_new(
        run_id,
        RunRelation::root(run_id).expect("relation"),
        RunSecurityContext::try_new(
            "tenant-a",
            PrincipalRef::try_new("issuer-a", "subject-a", Some("tenant-a")).expect("principal"),
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
            deadline: DeadlinePropagation::MinimumOfParentAndChild,
            budget: BudgetPropagation::SharedScope,
            principal: PrincipalPropagation::Inherit,
        },
        Digest::raw_json(br#"{"agent":"fixture"}"#),
        None,
    )
    .expect("accepted");
    AppendRequest::try_new(
        id(1),
        id::<SessionTag>(1),
        1,
        vec![
            RecordDraft::try_new(
                RECORD_FORMAT_VERSION,
                RECORD_KIND_VERSION,
                id::<RecordTag>(1),
                id::<SessionTag>(1),
                id::<LaneTag>(101),
                Some(run_id),
                Timestamp::from_unix_ms(1).expect("timestamp"),
                vec![id::<EventTag>(301)],
                RecordBody::RunAccepted(accepted),
            )
            .expect("draft"),
        ],
    )
    .expect("accept request")
}

fn seed(path: &Path, records: u64) {
    let store = SqliteJournalStore::try_open(SqliteStoreConfig {
        path: path.to_path_buf(),
        durability: SqliteDurability::Durable,
        limits: SqliteStoreLimits {
            sessions: 1,
            batches_per_session: usize::try_from(records).expect("batches"),
            records_per_session: usize::try_from(records).expect("records"),
            snapshot_bytes: 256 * 1024,
        },
        busy_timeout: DEFAULT_BUSY_TIMEOUT,
    })
    .expect("open");
    for sequence in 1..=records {
        let append = if sequence == 1 {
            accept_request()
        } else {
            request(sequence, sequence)
        };
        block_on(store.append(append)).expect("append");
    }
}

/// 64 records was too small for snapshot+tail to beat full replay.
const RESTORE_RECORDS: u64 = 1_024;
const SNAPSHOT_AT: u64 = 768;

fn restore_and_growth(criterion: &mut Criterion) {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("journal.sqlite");
    seed(&path, RESTORE_RECORDS);
    let db_bytes = std::fs::metadata(&path).expect("metadata").len();
    let wal_bytes =
        std::fs::metadata(path.with_file_name("journal.sqlite-wal")).map_or(0, |meta| meta.len());
    eprintln!(
        "sqlite_storage_growth records={RESTORE_RECORDS} db_bytes={db_bytes} wal_bytes={wal_bytes} (warning-only; performance-budget calibration owns budgets)"
    );

    criterion.bench_function("sqlite_restore_1024_records", |bencher| {
        bencher.iter(|| {
            let store = open(&path, RESTORE_RECORDS);
            let loaded = block_on(store.load(LoadRequest {
                session_id: id::<SessionTag>(1),
            }))
            .expect("load");
            assert_eq!(loaded.head_sequence, RESTORE_RECORDS);
        });
    });
}

fn open(path: &Path, records: u64) -> SqliteJournalStore {
    let limit = usize::try_from(records).expect("records");
    SqliteJournalStore::try_open(SqliteStoreConfig {
        path: path.to_path_buf(),
        durability: SqliteDurability::Durable,
        limits: SqliteStoreLimits {
            sessions: 1,
            batches_per_session: limit,
            records_per_session: limit,
            snapshot_bytes: 256 * 1024,
        },
        busy_timeout: Duration::from_secs(1),
    })
    .expect("open")
}

fn snapshot_versus_full_replay(criterion: &mut Criterion) {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("journal.sqlite");
    seed(&path, SNAPSHOT_AT);
    let store = open(&path, RESTORE_RECORDS);
    let recovered = block_on(CommitCoordinator::recover(
        std::sync::Arc::new(store) as std::sync::Arc<dyn JournalStore>,
        id::<SessionTag>(1),
    ))
    .expect("recover prefix");
    let store = open(&path, RESTORE_RECORDS);
    let loaded = block_on(store.load(LoadRequest {
        session_id: id::<SessionTag>(1),
    }))
    .expect("load prefix");
    block_on(store.write_state_snapshot(StateSnapshotRequest {
        session_id: id::<SessionTag>(1),
        state: recovered.state().clone(),
        head_checksum: loaded.head_checksum.expect("head"),
        pending_timer_scheduled_at: None,
        last_model_continuation: None,
    }))
    .expect("snapshot");
    for sequence in (SNAPSHOT_AT + 1)..=RESTORE_RECORDS {
        block_on(store.append(request(sequence, sequence))).expect("tail");
    }
    drop(store);

    criterion.bench_function("sqlite_recover_snapshot_plus_tail_1024", |bencher| {
        bencher.iter(|| {
            let store = open(&path, RESTORE_RECORDS);
            let recovered = block_on(CommitCoordinator::recover(
                std::sync::Arc::new(store) as std::sync::Arc<dyn JournalStore>,
                id::<SessionTag>(1),
            ))
            .expect("snapshot recover");
            assert_eq!(recovered.state().last_applied_sequence, RESTORE_RECORDS);
        });
    });

    let store = open(&path, RESTORE_RECORDS);
    store
        .discard_snapshot(id::<SessionTag>(1))
        .expect("discard");
    drop(store);

    criterion.bench_function("sqlite_recover_full_replay_1024", |bencher| {
        bencher.iter(|| {
            let store = open(&path, RESTORE_RECORDS);
            let recovered = block_on(CommitCoordinator::recover(
                std::sync::Arc::new(store) as std::sync::Arc<dyn JournalStore>,
                id::<SessionTag>(1),
            ))
            .expect("full recover");
            assert_eq!(recovered.state().last_applied_sequence, RESTORE_RECORDS);
        });
    });
    eprintln!(
        "sqlite_snapshot_vs_full records={RESTORE_RECORDS} snapshot_at={SNAPSHOT_AT} (warning-only; performance-budget calibration owns budgets)"
    );
}

fn bench_quick() -> bool {
    std::env::args().any(|argument| argument == "--quick")
}

fn scan_record_counts() -> &'static [u64] {
    if bench_quick() {
        &[256, 1_024]
    } else {
        &[10_000, 100_000]
    }
}

fn scan_page_sizes() -> &'static [u32] {
    &[64, 256]
}

fn decoded_record_bytes(records: &[finstack_ai_kernel::RecordEnvelope]) -> usize {
    records
        .iter()
        .map(|record| serde_json::to_vec(record).map_or(0, |bytes| bytes.len()))
        .sum()
}

fn traverse(
    store: &SqliteJournalStore,
    session: finstack_ai_kernel::SessionId,
    page: u32,
) -> (u64, usize, usize) {
    let mut from_sequence = 0;
    let mut envelopes = 0_u64;
    let mut decoded_bytes = 0_usize;
    let mut pages = 0_usize;
    loop {
        let scanned = block_on(store.scan(ScanRequest {
            session_id: session,
            from_sequence,
            limit: page,
        }))
        .expect("scan");
        pages += 1;
        envelopes += u64::try_from(scanned.records.len()).expect("page");
        decoded_bytes += decoded_record_bytes(scanned.records.as_ref());
        match scanned.next_sequence {
            Some(next) => from_sequence = next,
            None => break,
        }
    }
    (envelopes, decoded_bytes, pages)
}

fn sqlite_scan_paging(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("sqlite_scan_paging");
    if bench_quick() {
        group.sample_size(10);
        group.warm_up_time(Duration::from_millis(100));
        group.measurement_time(Duration::from_millis(400));
    }
    eprintln!(
        "sqlite_scan_paging requested_page_1024 rejected_by_api SCAN_PAGE_MAX_RECORDS={SCAN_PAGE_MAX_RECORDS}"
    );
    for &records in scan_record_counts() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("scan.sqlite");
        seed(&path, records);
        let store = SqliteJournalStore::try_open(SqliteStoreConfig {
            path: path.clone(),
            durability: SqliteDurability::Durable,
            limits: SqliteStoreLimits {
                sessions: 1,
                batches_per_session: usize::try_from(records).expect("records"),
                records_per_session: usize::try_from(records).expect("records"),
                snapshot_bytes: 256 * 1024,
            },
            busy_timeout: Duration::from_secs(1),
        })
        .expect("open");
        for &page in scan_page_sizes() {
            group.throughput(Throughput::Elements(records));
            group.bench_with_input(
                BenchmarkId::new(format!("records{records}"), page),
                &page,
                |bencher, &page| {
                    bencher.iter(|| {
                        let (envelopes, decoded_bytes, pages) =
                            traverse(&store, id::<SessionTag>(1), page);
                        assert_eq!(envelopes, records);
                        black_box((decoded_bytes, pages));
                    });
                },
            );
        }
    }
    group.finish();
}

fn percentile(sorted: &[f64], hundredths: u8) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let last = sorted.len().saturating_sub(1);
    let index = last
        .saturating_mul(usize::from(hundredths))
        .saturating_add(50)
        / 100;
    sorted[index.min(last)]
}

#[allow(clippy::too_many_lines)]
fn sqlite_concurrent(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("sqlite_concurrent");
    if bench_quick() {
        group.sample_size(10);
        group.warm_up_time(Duration::from_millis(100));
        group.measurement_time(Duration::from_millis(400));
    }
    let session_counts: &[usize] = if bench_quick() { &[1, 8] } else { &[1, 8, 64] };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("runtime");
    for durable in [true, false] {
        let label = if durable { "durable" } else { "relaxed" };
        for &sessions in session_counts {
            let dir = TempDir::new().expect("tempdir");
            let path = dir.path().join("concurrent.sqlite");
            let durability = if durable {
                SqliteDurability::Durable
            } else {
                SqliteDurability::Relaxed {
                    synchronous: finstack_ai_store_sqlite::SqliteSynchronous::Normal,
                }
            };
            let store = Arc::new(
                SqliteJournalStore::try_open(SqliteStoreConfig {
                    path,
                    durability,
                    limits: SqliteStoreLimits {
                        sessions,
                        batches_per_session: 4_096,
                        records_per_session: 4_096,
                        snapshot_bytes: 64 * 1024,
                    },
                    busy_timeout: Duration::from_secs(2),
                })
                .expect("open"),
            );
            runtime.block_on(async {
                for session in 1..=sessions {
                    let ordinal = u64::try_from(session).expect("session");
                    store
                        .append(
                            AppendRequest::try_new(
                                id(10_000 + ordinal),
                                id::<SessionTag>(ordinal),
                                1,
                                vec![draft_for_session(ordinal, 1)],
                            )
                            .expect("seed request"),
                        )
                        .await
                        .expect("seed");
                }
            });
            let next_sequence = Arc::new(AtomicU64::new(2));
            group.bench_with_input(
                BenchmarkId::new(label, sessions),
                &sessions,
                |bencher, &sessions| {
                    bencher.iter(|| {
                        let store = Arc::clone(&store);
                        let expected = next_sequence.fetch_add(1, Ordering::Relaxed);
                        runtime.block_on(async move {
                            let started_loop = Instant::now();
                            let delay = tokio::spawn(async {
                                tokio::time::sleep(Duration::from_millis(1)).await;
                                Instant::now()
                            });
                            let mut joins = Vec::with_capacity(sessions);
                            for session in 1..=sessions {
                                let store = Arc::clone(&store);
                                joins.push(tokio::spawn(async move {
                                    let ordinal = u64::try_from(session).expect("session");
                                    let append_started = Instant::now();
                                    store
                                        .append(
                                            AppendRequest::try_new(
                                                id(20_000 + ordinal * 10_000 + expected),
                                                id::<SessionTag>(ordinal),
                                                expected,
                                                vec![draft_for_session(ordinal, expected)],
                                            )
                                            .expect("append request"),
                                        )
                                        .await
                                        .expect("append");
                                    let append_ms =
                                        append_started.elapsed().as_secs_f64() * 1_000.0;
                                    let load_started = Instant::now();
                                    let loaded = store
                                        .load(LoadRequest {
                                            session_id: id::<SessionTag>(ordinal),
                                        })
                                        .await
                                        .expect("load");
                                    let load_ms = load_started.elapsed().as_secs_f64() * 1_000.0;
                                    assert!(loaded.head_sequence >= 1);
                                    (append_ms, load_ms)
                                }));
                            }
                            let mut appends = Vec::new();
                            let mut loads = Vec::new();
                            for join in joins {
                                let (append_ms, load_ms) = join.await.expect("join");
                                appends.push(append_ms);
                                loads.push(load_ms);
                            }
                            let loop_ms = delay
                                .await
                                .expect("delay")
                                .saturating_duration_since(started_loop)
                                .as_secs_f64()
                                * 1_000.0
                                - 1.0;
                            appends.sort_by(|left, right| left.partial_cmp(right).expect("ord"));
                            loads.sort_by(|left, right| left.partial_cmp(right).expect("ord"));
                            black_box((
                                percentile(&appends, 50),
                                percentile(&appends, 95),
                                percentile(&appends, 99),
                                percentile(&loads, 50),
                                percentile(&loads, 95),
                                percentile(&loads, 99),
                                loop_ms,
                            ));
                        });
                    });
                },
            );
        }
    }
    group.finish();
}

fn draft_for_session(session: u64, record_ordinal: u64) -> RecordDraft {
    let draft = draft(session * 1_000 + record_ordinal);
    // `draft` hard-codes session 1; rebuild against the requested session.
    RecordDraft::try_new(
        draft.format_version(),
        draft.kind_version(),
        draft.record_id(),
        id::<SessionTag>(session),
        draft.lane_id(),
        draft.run_id(),
        draft.timestamp(),
        draft.derived_event_ids().to_vec(),
        draft.body().clone(),
    )
    .expect("session draft")
}

criterion_group!(
    benches,
    restore_and_growth,
    snapshot_versus_full_replay,
    sqlite_scan_paging,
    sqlite_concurrent
);
criterion_main!(benches);
