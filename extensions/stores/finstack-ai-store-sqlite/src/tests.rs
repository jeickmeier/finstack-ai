use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, Barrier};
use std::task::{Context, Poll, Waker};
use std::thread;

use finstack_ai_kernel::{
    AcceptRun, AllocatedIds, AppendRequest, BudgetPropagation, CancellationPropagation,
    DeadlinePropagation, Digest, KernelInput, LaneCreated, LaneTag, Metadata, PrincipalPropagation,
    PrincipalRef, RECORD_FORMAT_VERSION, RECORD_KIND_VERSION, RecordBody, RecordDraft, RecordTag,
    RunAccepted, RunLimits, RunPropagationPolicy, RunRelation, RunSecurityContext, RunTag,
    SessionCreated, SessionTag, Timestamp, TransitionEnv,
};
use finstack_ai_protocol::{envelope_checksum, payload_digest, verify_envelope};
use finstack_ai_runtime::commit::CommitCoordinator;
use finstack_ai_runtime::ports::journal::{
    JournalStore, LoadFromRequest, LoadRequest, LoadWindow, OpaqueSnapshot, SCAN_PAGE_MAX_RECORDS,
    ScanRequest, SnapshotRequest, StateSnapshotRequest, StoreError, WriteMetadataRequest,
};
use finstack_ai_test::store_fixtures::{draft, id, request};
use finstack_ai_test::{JournalStoreConformanceCase, check_journal_store_conformance};
use rusqlite::{Connection, params};
use tempfile::TempDir;

use super::*;

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

fn limits() -> SqliteStoreLimits {
    SqliteStoreLimits {
        sessions: 4,
        batches_per_session: 8,
        records_per_session: 16,
        snapshot_bytes: 1024,
    }
}

fn snapshot_capable_store() -> Arc<SqliteJournalStore> {
    Arc::new(
        SqliteJournalStore::try_open(SqliteStoreConfig {
            path: PathBuf::from(":memory:"),
            durability: SqliteDurability::Relaxed {
                synchronous: SqliteSynchronous::Normal,
            },
            limits: SqliteStoreLimits {
                sessions: 4,
                batches_per_session: 8,
                records_per_session: 16,
                snapshot_bytes: 256 * 1024,
            },
            busy_timeout: DEFAULT_BUSY_TIMEOUT,
        })
        .expect("store"),
    )
}

fn accept_root_run(store: &Arc<SqliteJournalStore>) {
    let mut coordinator = CommitCoordinator::new(Arc::clone(store) as Arc<dyn JournalStore>);
    let run_id = id::<RunTag>(3);
    block_on(
        coordinator.submit(
            TransitionEnv {
                now: Timestamp::from_unix_ms(1_000).expect("ts"),
                ids: AllocatedIds::try_new(
                    vec![id(1)],
                    vec![id(1)],
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    vec![id(101)],
                    Vec::new(),
                )
                .expect("ids"),
            },
            KernelInput::AcceptRun(AcceptRun {
                session_id: id::<SessionTag>(1),
                lane_id: id::<LaneTag>(2),
                accepted: RunAccepted::try_new(
                    run_id,
                    RunRelation::root(run_id).expect("relation"),
                    RunSecurityContext::try_new(
                        "tenant-a",
                        PrincipalRef::try_new("issuer-a", "subject-a", Some("tenant-a"))
                            .expect("principal"),
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
                .expect("accepted"),
            }),
        ),
    )
    .expect("accept");
}

fn memory_store() -> SqliteJournalStore {
    SqliteJournalStore::try_open(SqliteStoreConfig {
        path: PathBuf::from(":memory:"),
        durability: SqliteDurability::Relaxed {
            synchronous: SqliteSynchronous::Normal,
        },
        limits: limits(),
        busy_timeout: DEFAULT_BUSY_TIMEOUT,
    })
    .expect("memory store")
}

fn file_store(dir: &TempDir, durability: SqliteDurability) -> SqliteJournalStore {
    SqliteJournalStore::try_open(SqliteStoreConfig {
        path: dir.path().join("journal.sqlite"),
        durability,
        limits: limits(),
        busy_timeout: DEFAULT_BUSY_TIMEOUT,
    })
    .expect("file store")
}

#[test]
fn durable_mode_is_rejected_for_memory_and_labeled_on_files() {
    assert!(matches!(
        SqliteJournalStore::try_open(SqliteStoreConfig {
            path: PathBuf::from(":memory:"),
            durability: SqliteDurability::Durable,
            limits: limits(),
            busy_timeout: DEFAULT_BUSY_TIMEOUT,
        }),
        Err(StoreError::InvalidRequest {
            reason_code: "sqlite_durable_requires_file"
        })
    ));
    let dir = TempDir::new().expect("tempdir");
    let store = file_store(&dir, SqliteDurability::Durable);
    let health = block_on(store.health()).expect("health");
    assert!(health.ready);
    assert!(health.durable);
    assert_eq!(health.detail.as_ref(), "sqlite_durable_wal_full");
}

#[test]
fn relaxed_modes_never_advertise_nfr_rel_001() {
    let memory = memory_store();
    let health = block_on(memory.health()).expect("health");
    assert!(health.ready);
    assert!(!health.durable);
    assert_eq!(health.detail.as_ref(), "sqlite_relaxed_in_memory");

    let dir = TempDir::new().expect("tempdir");
    let store = file_store(
        &dir,
        SqliteDurability::Relaxed {
            synchronous: SqliteSynchronous::Off,
        },
    );
    let health = block_on(store.health()).expect("health");
    assert!(!health.durable);
    assert_eq!(health.detail.as_ref(), "sqlite_relaxed_synchronous_off");
}

#[test]
fn unknown_user_version_fails_closed() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("journal.sqlite");
    drop(file_store(&dir, SqliteDurability::Durable));
    let connection = Connection::open(&path).expect("reopen");
    connection
        .pragma_update(None, "user_version", 99)
        .expect("bump");
    drop(connection);
    assert!(matches!(
        SqliteJournalStore::try_open(SqliteStoreConfig {
            path,
            durability: SqliteDurability::Durable,
            limits: limits(),
            busy_timeout: DEFAULT_BUSY_TIMEOUT,
        }),
        Err(StoreError::Integrity {
            reason_code: "sqlite_schema_unsupported"
        })
    ));
}

#[test]
fn append_load_and_health_preserve_boundaries() {
    let store = memory_store();
    let first = block_on(store.append(request(1, 1, 1, vec![draft(1, 1)]))).expect("append");
    let second =
        block_on(store.append(request(2, 1, 2, vec![draft(2, 1), draft(3, 1)]))).expect("append");
    assert_eq!((first.first_sequence, first.last_sequence), (1, 1));
    assert_eq!((second.first_sequence, second.last_sequence), (2, 3));
    verify_envelope(&first.records[0]).expect("first envelope");
    verify_envelope(&second.records[0]).expect("second envelope");
    assert_eq!(
        second.records[0].previous_checksum(),
        Some(first.records[0].checksum())
    );
    assert_eq!(
        first.records[0].payload_digest(),
        payload_digest(first.records[0].body()).expect("payload")
    );
    assert_eq!(
        first.records[0].checksum(),
        envelope_checksum(&first.records[0]).expect("checksum")
    );

    let loaded = block_on(store.load(LoadRequest {
        session_id: id::<SessionTag>(1),
    }))
    .expect("load");
    assert_eq!(loaded.head_sequence, 3);
    assert_eq!(loaded.head_checksum, Some(second.records[1].checksum()));
    assert_eq!(loaded.metadata, Metadata::empty());
    assert_eq!(loaded.committed_batches.as_ref(), &[first, second]);
    assert!(loaded.snapshot.is_none());
}

#[test]
fn scan_and_metadata_cas_are_session_local() {
    let store = memory_store();
    block_on(store.append(request(1, 1, 1, vec![draft(1, 1)]))).expect("append");
    let second =
        block_on(store.append(request(2, 1, 2, vec![draft(2, 1), draft(3, 1)]))).expect("append");
    let page = block_on(store.scan(ScanRequest {
        session_id: id::<SessionTag>(1),
        from_sequence: 0,
        limit: 2,
    }))
    .expect("scan");
    assert_eq!(page.records.len(), 2);
    assert_eq!(page.next_sequence, Some(3));
    let tail = block_on(store.scan(ScanRequest {
        session_id: id::<SessionTag>(1),
        from_sequence: 3,
        limit: 2,
    }))
    .expect("mid scan");
    assert_eq!(tail.records.len(), 1);
    assert_eq!(tail.records[0].sequence(), 3);
    assert_eq!(tail.next_sequence, None);
    assert!(matches!(
        block_on(store.scan(ScanRequest {
            session_id: id::<SessionTag>(1),
            from_sequence: 1,
            limit: 0,
        })),
        Err(StoreError::InvalidRequest {
            reason_code: "scan_limit_zero"
        })
    ));
    assert!(matches!(
        block_on(store.scan(ScanRequest {
            session_id: id::<SessionTag>(1),
            from_sequence: 1,
            limit: SCAN_PAGE_MAX_RECORDS + 1,
        })),
        Err(StoreError::InvalidRequest {
            reason_code: "scan_limit_exceeded"
        })
    ));

    let metadata = Metadata::parse(br#"{"label":"demo"}"#).expect("metadata");
    assert!(matches!(
        block_on(store.write_metadata(WriteMetadataRequest {
            session_id: id::<SessionTag>(1),
            expected_head_checksum: None,
            metadata: metadata.clone(),
        })),
        Err(StoreError::InvalidRequest {
            reason_code: "metadata_cas_mismatch"
        })
    ));
    let receipt = block_on(store.write_metadata(WriteMetadataRequest {
        session_id: id::<SessionTag>(1),
        expected_head_checksum: Some(second.records[1].checksum()),
        metadata: metadata.clone(),
    }))
    .expect("cas");
    assert_eq!(receipt.metadata, metadata);
    let loaded = block_on(store.load(LoadRequest {
        session_id: id::<SessionTag>(1),
    }))
    .expect("load");
    assert_eq!(loaded.metadata, metadata);
}

#[test]
fn scan_verifies_page_from_stored_checkpoint() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("journal.sqlite");
    let store = file_store(&dir, SqliteDurability::Durable);
    block_on(store.append(request(1, 1, 1, vec![draft(1, 1)]))).expect("append");
    block_on(store.append(request(2, 1, 2, vec![draft(2, 1)]))).expect("append");
    block_on(store.append(request(3, 1, 3, vec![draft(3, 1)]))).expect("append");
    let connection = Connection::open(&path).expect("second connection");
    connection
        .execute(
            "UPDATE records SET envelope_checksum = ?1 WHERE sequence = 2",
            params![vec![0_u8; 32]],
        )
        .expect("corrupt");
    drop(connection);
    assert!(matches!(
        block_on(store.scan(ScanRequest {
            session_id: id::<SessionTag>(1),
            from_sequence: 2,
            limit: 2,
        })),
        Err(StoreError::Integrity { .. })
    ));
}

#[test]
fn structural_session_and_lane_records_commit_without_run_id() {
    let store = memory_store();
    let session = RecordDraft::try_new(
        RECORD_FORMAT_VERSION,
        RECORD_KIND_VERSION,
        id::<RecordTag>(90),
        id::<SessionTag>(9),
        id::<LaneTag>(91),
        None,
        Timestamp::from_unix_ms(1).expect("ts"),
        Vec::new(),
        RecordBody::SessionCreated(SessionCreated::new(Metadata::empty())),
    )
    .expect("session");
    let lane = RecordDraft::try_new(
        RECORD_FORMAT_VERSION,
        RECORD_KIND_VERSION,
        id::<RecordTag>(91),
        id::<SessionTag>(9),
        id::<LaneTag>(91),
        None,
        Timestamp::from_unix_ms(1).expect("ts"),
        Vec::new(),
        RecordBody::LaneCreated(LaneCreated::try_new("main").expect("lane")),
    )
    .expect("lane");
    let committed = block_on(store.append(request(90, 9, 1, vec![session, lane]))).expect("append");
    assert_eq!(committed.records.len(), 2);
    verify_envelope(&committed.records[0]).expect("session envelope");
    verify_envelope(&committed.records[1]).expect("lane envelope");
}

#[test]
fn batch_and_record_idempotency_precede_sequence_checks() {
    let store = memory_store();
    let frozen = request(10, 1, 1, vec![draft(10, 1)]);
    let original = block_on(store.append(frozen.clone())).expect("append");
    assert_eq!(
        block_on(store.append(frozen.clone())).expect("same batch"),
        original
    );

    let same_records_new_batch = AppendRequest::try_new(
        id(11),
        frozen.session_id(),
        frozen.expected_sequence(),
        frozen.records().to_vec(),
    )
    .expect("request");
    assert_eq!(
        block_on(store.append(same_records_new_batch)).expect("record idempotency"),
        original
    );

    let unequal_batch = request(10, 1, 2, vec![draft(11, 1)]);
    assert!(matches!(
        block_on(store.append(unequal_batch)),
        Err(StoreError::Corruption {
            reason_code: "append_batch_id_reuse"
        })
    ));
    let mixed = request(12, 1, 2, vec![draft(10, 1), draft(12, 1)]);
    assert!(matches!(
        block_on(store.append(mixed)),
        Err(StoreError::Corruption {
            reason_code: "mixed_record_id_reuse"
        })
    ));
}

#[test]
fn conflicts_are_atomic_and_concurrent_writers_have_one_winner() {
    let store = Arc::new(memory_store());
    let barrier = Arc::new(Barrier::new(3));
    let mut joins = Vec::new();
    for ordinal in [20_u64, 21] {
        let store = Arc::clone(&store);
        let barrier = Arc::clone(&barrier);
        joins.push(thread::spawn(move || {
            barrier.wait();
            block_on(store.append(request(ordinal, 1, 1, vec![draft(ordinal, 1)])))
        }));
    }
    barrier.wait();
    let results = joins
        .into_iter()
        .map(|join| join.join().expect("writer"))
        .collect::<Vec<_>>();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Err(StoreError::Conflict { .. })))
            .count(),
        1
    );

    let loaded = block_on(store.load(LoadRequest {
        session_id: id::<SessionTag>(1),
    }))
    .expect("load");
    assert_eq!(loaded.head_sequence, 1);
    assert_eq!(loaded.committed_batches.len(), 1);
}

#[test]
fn configured_limits_and_snapshot_cache_are_enforced() {
    assert!(
        SqliteJournalStore::try_open(SqliteStoreConfig {
            path: PathBuf::from(":memory:"),
            durability: SqliteDurability::Relaxed {
                synchronous: SqliteSynchronous::Normal,
            },
            limits: SqliteStoreLimits {
                sessions: 0,
                ..limits()
            },
            busy_timeout: DEFAULT_BUSY_TIMEOUT,
        })
        .is_err()
    );
    let store = SqliteJournalStore::try_open(SqliteStoreConfig {
        path: PathBuf::from(":memory:"),
        durability: SqliteDurability::Relaxed {
            synchronous: SqliteSynchronous::Normal,
        },
        limits: SqliteStoreLimits {
            sessions: 1,
            batches_per_session: 1,
            records_per_session: 1,
            snapshot_bytes: 3,
        },
        busy_timeout: DEFAULT_BUSY_TIMEOUT,
    })
    .expect("store");
    block_on(store.append(request(30, 1, 1, vec![draft(30, 1)]))).expect("append");
    assert!(matches!(
        block_on(store.append(request(31, 1, 2, vec![draft(31, 1)]))),
        Err(StoreError::LimitExceeded {
            resource: "batches_per_session",
            limit: 1
        })
    ));
    assert!(matches!(
        block_on(store.append(request(32, 2, 1, vec![draft(32, 2)]))),
        Err(StoreError::LimitExceeded {
            resource: "sessions",
            limit: 1
        })
    ));

    let oversized = OpaqueSnapshot::try_new(1, Digest::raw_json(b"four"), b"four".as_slice(), 8)
        .expect("caller ceiling");
    assert!(matches!(
        block_on(store.write_snapshot(SnapshotRequest {
            session_id: id::<SessionTag>(1),
            snapshot: oversized,
        })),
        Err(StoreError::LimitExceeded {
            resource: "snapshot_bytes",
            limit: 3
        })
    ));

    let snapshot = OpaqueSnapshot::try_new(1, Digest::raw_json(b"one"), b"one".as_slice(), 3)
        .expect("snapshot");
    let receipt = block_on(store.write_snapshot(SnapshotRequest {
        session_id: id::<SessionTag>(1),
        snapshot: snapshot.clone(),
    }))
    .expect("snapshot write");
    assert_eq!(receipt.bytes, 3);
    let loaded = block_on(store.load(LoadRequest {
        session_id: id::<SessionTag>(1),
    }))
    .expect("load");
    assert_eq!(loaded.snapshot, Some(snapshot));
}

#[test]
fn injected_rollback_leaves_no_partial_batch() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("journal.sqlite");
    let store = file_store(&dir, SqliteDurability::Durable);
    store
        .append_then_rollback(&request(1, 1, 1, vec![draft(1, 1), draft(2, 1)]))
        .expect("rollback");
    drop(store);
    let reopened = SqliteJournalStore::try_open(SqliteStoreConfig {
        path,
        durability: SqliteDurability::Durable,
        limits: limits(),
        busy_timeout: DEFAULT_BUSY_TIMEOUT,
    })
    .expect("reopen");
    let loaded = block_on(reopened.load(LoadRequest {
        session_id: id::<SessionTag>(1),
    }))
    .expect("load");
    assert_eq!(loaded.head_sequence, 0);
    assert!(loaded.committed_batches.is_empty());
}

#[test]
fn conformance_and_ambiguous_ack_use_the_sqlite_store() {
    let store = memory_store();
    block_on(check_journal_store_conformance(
        &store,
        JournalStoreConformanceCase {
            request: request(1, 1, 1, vec![draft(1, 1)]),
            expected_first_sequence: 1,
            expected_last_sequence: 1,
        },
    ))
    .expect("conformance");
}

#[test]
fn tail_window_rejects_mid_batch_starts() {
    let store = memory_store();
    let first =
        block_on(store.append(request(1, 1, 1, vec![draft(1, 1), draft(2, 1)]))).expect("append");
    block_on(store.append(request(2, 1, 3, vec![draft(3, 1)]))).expect("append");
    assert!(matches!(
        block_on(store.load_from(LoadFromRequest {
            session_id: id::<SessionTag>(1),
            window: LoadWindow::FromSequence {
                from_sequence: 2,
                prior_checksum: first.records[0].checksum(),
            },
        })),
        Err(StoreError::Integrity {
            reason_code: "load_from_splits_batch"
        })
    ));
}

#[test]
fn discarding_sqlite_snapshots_still_recovers_from_the_journal() {
    let store = snapshot_capable_store();
    accept_root_run(&store);
    let recovered = block_on(CommitCoordinator::recover(
        Arc::clone(&store) as Arc<dyn JournalStore>,
        id::<SessionTag>(1),
    ))
    .expect("recover");
    let expected = recovered.state().state_hash().expect("hash");
    let loaded = block_on(store.load(LoadRequest {
        session_id: id::<SessionTag>(1),
    }))
    .expect("load");
    block_on(store.write_state_snapshot(StateSnapshotRequest {
        session_id: id::<SessionTag>(1),
        state: recovered.state().clone(),
        head_checksum: loaded.head_checksum.expect("head"),
        pending_timer_scheduled_at: None,
        last_model_continuation: None,
    }))
    .expect("snapshot");
    let loaded = block_on(store.load(LoadRequest {
        session_id: id::<SessionTag>(1),
    }))
    .expect("accelerated");
    assert!(loaded.accelerated.is_some());
    store
        .discard_snapshot(id::<SessionTag>(1))
        .expect("discard");
    let loaded = block_on(store.load(LoadRequest {
        session_id: id::<SessionTag>(1),
    }))
    .expect("after discard");
    assert!(loaded.snapshot.is_none());
    assert!(loaded.accelerated.is_none());
    let rebuilt =
        block_on(CommitCoordinator::recover(store, id::<SessionTag>(1))).expect("rebuild");
    assert_eq!(rebuilt.state().state_hash().expect("hash"), expected);
}

#[test]
fn load_from_omits_the_verified_prefix() {
    let store = snapshot_capable_store();
    accept_root_run(&store);
    let full = block_on(store.load(LoadRequest {
        session_id: id::<SessionTag>(1),
    }))
    .expect("full");
    let first = full.committed_batches.first().expect("first batch");
    let prior = first.records.last().expect("prior").checksum();
    let from_sequence = first.last_sequence.saturating_add(1);
    if from_sequence <= full.head_sequence {
        let tail = block_on(store.load_from(LoadFromRequest {
            session_id: id::<SessionTag>(1),
            window: LoadWindow::FromSequence {
                from_sequence,
                prior_checksum: prior,
            },
        }))
        .expect("tail");
        assert!(tail.omits_prefix());
        assert_eq!(tail.head_sequence, full.head_sequence);
        assert_eq!(
            tail.committed_batches
                .first()
                .map(|batch| batch.first_sequence),
            Some(from_sequence)
        );
    }
    block_on(
        store.write_state_snapshot(StateSnapshotRequest {
            session_id: id::<SessionTag>(1),
            state: block_on(CommitCoordinator::recover(
                Arc::clone(&store) as Arc<dyn JournalStore>,
                id::<SessionTag>(1),
            ))
            .expect("recover")
            .state()
            .clone(),
            head_checksum: full.head_checksum.expect("head"),
            pending_timer_scheduled_at: None,
            last_model_continuation: None,
        }),
    )
    .expect("snapshot");
    let snapshot_plus_tail = block_on(store.load_from(LoadFromRequest {
        session_id: id::<SessionTag>(1),
        window: LoadWindow::SnapshotPlusTail,
    }))
    .expect("snapshot plus tail");
    assert!(snapshot_plus_tail.accelerated.is_some());
    assert!(snapshot_plus_tail.committed_batches.is_empty());
    assert_eq!(snapshot_plus_tail.head_sequence, full.head_sequence);
}

#[test]
fn windowed_load_must_not_cache_an_unverified_prefix() {
    // A windowed load's tail verification chains from a caller-supplied
    // prior checksum and proves nothing about the omitted prefix. If it were
    // allowed to populate the process-local verified-head cache, a later
    // `Full` load could anchor on that head in
    // `finstack_ai_store_common::verify_head_against_cache` and verify only
    // the suffix after it, skipping a prefix corruption this process never
    // actually checked (fail-open).
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("journal.sqlite");
    let setup = file_store(&dir, SqliteDurability::Durable);
    let first = block_on(setup.append(request(1, 1, 1, vec![draft(1, 1)]))).expect("append");
    let second = block_on(setup.append(request(2, 1, 2, vec![draft(2, 1)]))).expect("append");
    let prior = first.records[0].checksum();
    // Appending legitimately populates this process's verified-head cache
    // from the in-memory chain it just built, which would mask the bug
    // under test. Drop the store (and its worker/cache) before corrupting
    // the file and reopening, so the load path below starts with an empty
    // cache — exactly the fresh-process scenario the defect targets.
    drop(setup);

    let connection = Connection::open(&path).expect("second connection");
    connection
        .execute(
            "UPDATE records SET envelope_checksum = ?1 WHERE sequence = 1",
            params![vec![0_u8; 32]],
        )
        .expect("corrupt prefix");
    drop(connection);

    let store = SqliteJournalStore::try_open(SqliteStoreConfig {
        path,
        durability: SqliteDurability::Durable,
        limits: limits(),
        busy_timeout: DEFAULT_BUSY_TIMEOUT,
    })
    .expect("reopen store");

    // The windowed load starts after the corrupted prefix and chains from
    // the caller-supplied `prior` checksum, so it never reads the corrupted
    // record and succeeds.
    let tail = block_on(store.load_from(LoadFromRequest {
        session_id: id::<SessionTag>(1),
        window: LoadWindow::FromSequence {
            from_sequence: second.first_sequence,
            prior_checksum: prior,
        },
    }))
    .expect("windowed load succeeds despite corrupted prefix");
    assert_eq!(tail.head_sequence, second.last_sequence);

    // A subsequent full load must independently verify the whole chain and
    // catch the corrupted prefix record — it must not be short-circuited by
    // a verified-head cache entry the windowed load had no business writing.
    let result = block_on(store.load(LoadRequest {
        session_id: id::<SessionTag>(1),
    }));
    assert!(
        matches!(result, Err(StoreError::Integrity { .. })),
        "full load must fail-closed on the corrupted prefix, got {result:?}"
    );
}

#[test]
fn v1_schema_applies_from_user_version_zero() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("fresh.sqlite");
    let connection = Connection::open(&path).expect("create");
    let version: i32 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .expect("version");
    assert_eq!(version, 0);
    drop(connection);
    let store = SqliteJournalStore::try_open(SqliteStoreConfig {
        path: path.clone(),
        durability: SqliteDurability::Durable,
        limits: limits(),
        busy_timeout: DEFAULT_BUSY_TIMEOUT,
    })
    .expect("apply v1");
    drop(store);
    let connection = Connection::open(&path).expect("reopen");
    let version: i32 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .expect("version");
    assert_eq!(version, SCHEMA_USER_VERSION);
}

#[test]
fn append_identity_encoding_is_stable() {
    // Pins the persisted `batches.request_cbor` encoding. If this test fails,
    // existing databases will mis-detect batch-id replays as
    // `append_batch_id_reuse`. Do not update the constant without a schema
    // migration story.
    let frozen = request(7, 3, 1, vec![draft(70, 3), draft(71, 3)]);
    let bytes = crate::append::request_cbor(&frozen).expect("encode identity");
    let digest = finstack_ai_kernel::Digest::raw_json(&bytes);
    assert_eq!(
        digest.to_hex(),
        "c7aceba03fb3311d46d5be1e5647be58e9161b52c803c04bc82d13cfd64a1290"
    );
}
