//! Server-gated integration tests for snapshot writes, `scan`, and the
//! `write_metadata` CAS.
//!
//! Every test skips with a notice (exit 0) when `FINSTACK_PG_TEST_URL` is
//! unset, per the crate's env-gated testing convention (spec D10). The
//! scenarios asserted here are the ones
//! `extensions/stores/finstack-ai-store-sqlite/src/tests.rs` asserts for the
//! sqlite backend (`scan_and_metadata_cas_are_session_local`,
//! `configured_limits_and_snapshot_cache_are_enforced`,
//! `discarding_sqlite_snapshots_still_recovers_from_the_journal`): both
//! backends delegate admission to `finstack-ai-store-common`, so a
//! divergence in reason code or check ordering is a bug in this crate's
//! orchestration.

mod helpers;

use std::sync::Arc;

use finstack_ai_kernel::{
    AcceptRun, AllocatedIds, BudgetPropagation, CancellationPropagation, DeadlinePropagation,
    Digest, KernelInput, LaneTag, Metadata, PrincipalPropagation, PrincipalRef, RunAccepted,
    RunLimits, RunPropagationPolicy, RunRelation, RunSecurityContext, RunTag, SessionTag,
    Timestamp, TransitionEnv,
};
use finstack_ai_runtime::{
    CommitCoordinator, JournalStore, LoadRequest, OpaqueSnapshot, ScanRequest, SnapshotRequest,
    StateSnapshotRequest, StoreError, StoreLimits, WriteMetadataRequest,
};
use finstack_ai_store_postgres::{PostgresJournalStore, PostgresStoreConfig};
use finstack_ai_test::store_fixtures::{draft, id, request};

use helpers::{SchemaGuard, connect, disposable_store, fresh_schema_name, pg_test_url};

/// Generous limits: most of these tests care about admission/CAS logic, not
/// ceilings.
fn wide_limits() -> StoreLimits {
    StoreLimits {
        sessions: 1_000,
        batches_per_session: 1_000,
        records_per_session: 1_000,
        snapshot_bytes: 1_000_000,
    }
}

/// Open a store against an explicit schema name, so a test can also open a
/// raw connection to the same schema (to corrupt a stored row directly).
async fn open_store(url: &str, schema: &str) -> PostgresJournalStore {
    let mut config = PostgresStoreConfig::new(url, wide_limits());
    config.schema = Arc::from(schema);
    PostgresJournalStore::try_open(config)
        .await
        .expect("try_open store")
}

/// One store over a fresh disposable schema, plus its schema name and guard.
async fn store_over_fresh_schema(url: &str) -> (PostgresJournalStore, String, SchemaGuard) {
    let schema = fresh_schema_name();
    let store = open_store(url, &schema).await;
    (store, schema.clone(), SchemaGuard::new(schema))
}

/// Append a real `AcceptRun` record to session 1, so [`CommitCoordinator`]
/// has kernel state worth recovering. Mirrors sqlite's `accept_root_run`
/// fixture exactly (`extensions/stores/finstack-ai-store-sqlite/src/tests.rs`).
async fn accept_root_run(store: &Arc<PostgresJournalStore>) {
    let mut coordinator = CommitCoordinator::new(Arc::clone(store) as Arc<dyn JournalStore>);
    let run_id = id::<RunTag>(3);
    coordinator
        .submit(
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
        )
        .await
        .expect("accept");
}

/// A state snapshot round-trips through `load`: after writing it, `load`
/// reports `accelerated` covering the recovered kernel state, and its head
/// checksum matches the record the snapshot was taken at.
#[tokio::test]
async fn state_snapshot_round_trips_through_load_as_accelerated() {
    let Some(url) = pg_test_url() else {
        eprintln!("skipped: FINSTACK_PG_TEST_URL unset");
        return;
    };
    let (store, guard) = disposable_store(&url).await;
    let store = Arc::new(store);
    accept_root_run(&store).await;

    let recovered = CommitCoordinator::recover(
        Arc::clone(&store) as Arc<dyn JournalStore>,
        id::<SessionTag>(1),
    )
    .await
    .expect("recover");
    let loaded = store
        .load(LoadRequest {
            session_id: id::<SessionTag>(1),
        })
        .await
        .expect("load before snapshot");
    let head_checksum = loaded.head_checksum.expect("non-empty journal has a head");

    store
        .write_state_snapshot(StateSnapshotRequest {
            session_id: id::<SessionTag>(1),
            state: recovered.state().clone(),
            head_checksum,
            pending_timer_scheduled_at: None,
            last_model_continuation: None,
        })
        .await
        .expect("write_state_snapshot");

    let loaded = store
        .load(LoadRequest {
            session_id: id::<SessionTag>(1),
        })
        .await
        .expect("load after snapshot");
    let accelerated = loaded.accelerated.expect("snapshot decodes to accelerated");
    assert_eq!(accelerated.head_checksum, head_checksum);

    guard.cleanup(&connect(&url).await).await;
}

/// A snapshot whose payload exceeds the configured `snapshot_bytes` ceiling
/// is rejected before any write.
#[tokio::test]
async fn oversized_snapshot_is_limit_exceeded() {
    let Some(url) = pg_test_url() else {
        eprintln!("skipped: FINSTACK_PG_TEST_URL unset");
        return;
    };
    let limits = StoreLimits {
        snapshot_bytes: 3,
        ..wide_limits()
    };
    let mut config = finstack_ai_store_postgres::PostgresStoreConfig::new(url.as_str(), limits);
    config.schema = Arc::from(helpers::fresh_schema_name().as_str());
    let schema = config.schema.to_string();
    let store = PostgresJournalStore::try_open(config)
        .await
        .expect("try_open store");
    store
        .append(request(1, 1, 1, vec![draft(1, 1)]))
        .await
        .expect("append");

    let oversized = OpaqueSnapshot::try_new(1, Digest::raw_json(b"four"), b"four".as_slice(), 8)
        .expect("caller ceiling");
    let error = store
        .write_snapshot(SnapshotRequest {
            session_id: id::<SessionTag>(1),
            snapshot: oversized,
        })
        .await
        .expect_err("oversized snapshot must be rejected");
    assert!(
        matches!(
            error,
            StoreError::LimitExceeded {
                resource: "snapshot_bytes",
                limit: 3
            }
        ),
        "unexpected error: {error:?}"
    );

    let client = connect(&url).await;
    client
        .batch_execute(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"))
        .await
        .expect("drop disposable test schema");
}

/// A snapshot sequence beyond the journal head is rejected.
#[tokio::test]
async fn snapshot_ahead_of_journal_is_rejected() {
    let Some(url) = pg_test_url() else {
        eprintln!("skipped: FINSTACK_PG_TEST_URL unset");
        return;
    };
    let (store, guard) = disposable_store(&url).await;
    store
        .append(request(1, 1, 1, vec![draft(1, 1)]))
        .await
        .expect("append");

    let ahead = OpaqueSnapshot::try_new(2, Digest::raw_json(b"one"), b"one".as_slice(), 1_000)
        .expect("snapshot");
    let error = store
        .write_snapshot(SnapshotRequest {
            session_id: id::<SessionTag>(1),
            snapshot: ahead,
        })
        .await
        .expect_err("a snapshot past the head must be rejected");
    assert!(
        matches!(
            error,
            StoreError::InvalidRequest {
                reason_code: "snapshot_ahead_of_journal"
            }
        ),
        "unexpected error: {error:?}"
    );

    guard.cleanup(&connect(&url).await).await;
}

/// A snapshot sequence lower than the currently stored one is a regression.
#[tokio::test]
async fn snapshot_sequence_regression_is_rejected() {
    let Some(url) = pg_test_url() else {
        eprintln!("skipped: FINSTACK_PG_TEST_URL unset");
        return;
    };
    let (store, guard) = disposable_store(&url).await;
    store
        .append(request(1, 1, 1, vec![draft(1, 1)]))
        .await
        .expect("batch one");
    store
        .append(request(2, 1, 2, vec![draft(2, 1)]))
        .await
        .expect("batch two");

    let at_two = OpaqueSnapshot::try_new(2, Digest::raw_json(b"two"), b"two".as_slice(), 1_000)
        .expect("snapshot");
    store
        .write_snapshot(SnapshotRequest {
            session_id: id::<SessionTag>(1),
            snapshot: at_two,
        })
        .await
        .expect("advance to sequence 2");

    let regressed = OpaqueSnapshot::try_new(1, Digest::raw_json(b"one"), b"one".as_slice(), 1_000)
        .expect("snapshot");
    let error = store
        .write_snapshot(SnapshotRequest {
            session_id: id::<SessionTag>(1),
            snapshot: regressed,
        })
        .await
        .expect_err("a regression must be rejected");
    assert!(
        matches!(
            error,
            StoreError::InvalidRequest {
                reason_code: "snapshot_sequence_regression"
            }
        ),
        "unexpected error: {error:?}"
    );

    guard.cleanup(&connect(&url).await).await;
}

/// Scanning five records two at a time walks `next_sequence` from `Some`
/// down to `None` at the session end.
#[tokio::test]
async fn scan_pages_walk_next_sequence_to_none() {
    let Some(url) = pg_test_url() else {
        eprintln!("skipped: FINSTACK_PG_TEST_URL unset");
        return;
    };
    let (store, guard) = disposable_store(&url).await;
    store
        .append(request(
            1,
            1,
            1,
            vec![
                draft(1, 1),
                draft(2, 1),
                draft(3, 1),
                draft(4, 1),
                draft(5, 1),
            ],
        ))
        .await
        .expect("append five records");

    let first = store
        .scan(ScanRequest {
            session_id: id::<SessionTag>(1),
            from_sequence: 0,
            limit: 2,
        })
        .await
        .expect("first page");
    assert_eq!(first.records.len(), 2);
    assert_eq!(first.records[0].sequence(), 1);
    assert_eq!(first.records[1].sequence(), 2);
    assert_eq!(first.next_sequence, Some(3));

    let second = store
        .scan(ScanRequest {
            session_id: id::<SessionTag>(1),
            from_sequence: first.next_sequence.expect("has more"),
            limit: 2,
        })
        .await
        .expect("second page");
    assert_eq!(second.records.len(), 2);
    assert_eq!(second.records[0].sequence(), 3);
    assert_eq!(second.records[1].sequence(), 4);
    assert_eq!(second.next_sequence, Some(5));

    let third = store
        .scan(ScanRequest {
            session_id: id::<SessionTag>(1),
            from_sequence: second.next_sequence.expect("has more"),
            limit: 2,
        })
        .await
        .expect("third page");
    assert_eq!(third.records.len(), 1);
    assert_eq!(third.records[0].sequence(), 5);
    assert_eq!(third.next_sequence, None);

    guard.cleanup(&connect(&url).await).await;
}

/// `limit: 0` is an invalid scan request.
#[tokio::test]
async fn scan_with_zero_limit_is_invalid() {
    let Some(url) = pg_test_url() else {
        eprintln!("skipped: FINSTACK_PG_TEST_URL unset");
        return;
    };
    let (store, guard) = disposable_store(&url).await;
    store
        .append(request(1, 1, 1, vec![draft(1, 1)]))
        .await
        .expect("append");

    let error = store
        .scan(ScanRequest {
            session_id: id::<SessionTag>(1),
            from_sequence: 0,
            limit: 0,
        })
        .await
        .expect_err("zero limit must be rejected");
    assert!(
        matches!(
            error,
            StoreError::InvalidRequest {
                reason_code: "scan_limit_zero"
            }
        ),
        "unexpected error: {error:?}"
    );

    guard.cleanup(&connect(&url).await).await;
}

/// `write_metadata`'s compare-and-swap succeeds against the true head
/// checksum and fails cleanly against a stale one.
#[tokio::test]
async fn write_metadata_cas_succeeds_and_fails_on_stale_head() {
    let Some(url) = pg_test_url() else {
        eprintln!("skipped: FINSTACK_PG_TEST_URL unset");
        return;
    };
    let (store, guard) = disposable_store(&url).await;
    let committed = store
        .append(request(1, 1, 1, vec![draft(1, 1)]))
        .await
        .expect("append");
    let head_checksum = committed
        .records
        .last()
        .expect("non-empty batch")
        .checksum();
    let metadata = Metadata::parse(br#"{"label":"demo"}"#).expect("metadata");

    let stale = store
        .write_metadata(WriteMetadataRequest {
            session_id: id::<SessionTag>(1),
            expected_head_checksum: None,
            metadata: metadata.clone(),
        })
        .await
        .expect_err("a stale expected head must be rejected");
    assert!(
        matches!(
            stale,
            StoreError::InvalidRequest {
                reason_code: "metadata_cas_mismatch"
            }
        ),
        "unexpected error: {stale:?}"
    );

    let receipt = store
        .write_metadata(WriteMetadataRequest {
            session_id: id::<SessionTag>(1),
            expected_head_checksum: Some(head_checksum),
            metadata: metadata.clone(),
        })
        .await
        .expect("cas against the true head must succeed");
    assert_eq!(receipt.metadata, metadata);
    assert_eq!(receipt.head_checksum, Some(head_checksum));

    let loaded = store
        .load(LoadRequest {
            session_id: id::<SessionTag>(1),
        })
        .await
        .expect("load");
    assert_eq!(loaded.metadata, metadata);

    guard.cleanup(&connect(&url).await).await;
}

/// A scan page verifies against the stored chain: a record tampered
/// directly in storage (not through this store's own writers) fails the
/// scan that would otherwise return it, mirroring sqlite's
/// `scan_verifies_page_from_stored_checkpoint` exactly (three single-record
/// batches, corrupt the middle record's `envelope_checksum` via a raw
/// connection, then scan a page starting at that record).
#[tokio::test]
async fn scan_verifies_page_from_stored_checkpoint() {
    let Some(url) = pg_test_url() else {
        eprintln!("skipped: FINSTACK_PG_TEST_URL unset");
        return;
    };
    let (store, schema, guard) = store_over_fresh_schema(&url).await;
    store
        .append(request(1, 1, 1, vec![draft(1, 1)]))
        .await
        .expect("batch one");
    store
        .append(request(2, 1, 2, vec![draft(2, 1)]))
        .await
        .expect("batch two");
    store
        .append(request(3, 1, 3, vec![draft(3, 1)]))
        .await
        .expect("batch three");

    let client = connect(&url).await;
    client
        .execute(
            &format!(
                "UPDATE {schema}.records SET envelope_checksum = $1 \
                 WHERE session_id = $2 AND sequence = 2"
            ),
            &[&vec![0_u8; 32], &id::<SessionTag>(1).as_bytes().as_slice()],
        )
        .await
        .expect("corrupt sequence 2");

    let error = store
        .scan(ScanRequest {
            session_id: id::<SessionTag>(1),
            from_sequence: 2,
            limit: 2,
        })
        .await
        .expect_err("a corrupted record must fail the scan");
    assert!(
        matches!(error, StoreError::Integrity { .. }),
        "unexpected error: {error:?}"
    );

    guard.cleanup(&client).await;
}

/// A record missing *at* the requested scan start fails closed with
/// `Integrity{scan_sequence_gap}` when the session has never been pruned.
///
/// This is where the postgres store is deliberately stricter than sqlite,
/// which masks the hole and returns the page starting at the next surviving
/// record. Eight records in four batches, one mid-journal row deleted
/// straight out of `records` (a hole this store's own writers can never
/// produce), then a scan that starts exactly at the deleted sequence.
#[tokio::test]
async fn scan_from_a_deleted_sequence_is_a_gap() {
    let Some(url) = pg_test_url() else {
        eprintln!("skipped: FINSTACK_PG_TEST_URL unset");
        return;
    };
    let (store, schema, guard) = store_over_fresh_schema(&url).await;
    for batch in 0..4_u64 {
        let first = batch * 2 + 1;
        store
            .append(request(
                batch + 1,
                1,
                first,
                vec![draft(first, 1), draft(first + 1, 1)],
            ))
            .await
            .expect("batch appends");
    }

    let client = connect(&url).await;
    let deleted = client
        .execute(
            &format!("DELETE FROM {schema}.records WHERE session_id = $1 AND sequence = 4"),
            &[&id::<SessionTag>(1).as_bytes().as_slice()],
        )
        .await
        .expect("delete sequence 4");
    assert_eq!(deleted, 1, "the fixture must have removed exactly one row");

    let error = store
        .scan(ScanRequest {
            session_id: id::<SessionTag>(1),
            from_sequence: 4,
            limit: 4,
        })
        .await
        .expect_err("a record missing at the scan start must fail closed");
    assert!(
        matches!(
            error,
            StoreError::Integrity {
                reason_code: "scan_sequence_gap"
            }
        ),
        "unexpected error: {error:?}"
    );

    guard.cleanup(&client).await;
}
