//! Server-gated integration tests for the multi-writer append path
//! (spec D4/D5).
//!
//! Every test skips with a notice (exit 0) when `FINSTACK_PG_TEST_URL` is
//! unset, per the crate's env-gated testing convention (spec D10).
//!
//! The single-writer semantics asserted here are deliberately the *same*
//! scenarios the sqlite store asserts in
//! `extensions/stores/finstack-ai-store-sqlite/src/tests.rs`: both backends
//! delegate admission to `finstack-ai-store-common`, so a divergence in
//! reason code or check ordering is a bug in this crate's orchestration.

mod helpers;

use std::sync::Arc;

use finstack_ai_kernel::{AppendRequest, CommittedBatch, Digest, RecordEnvelope};
use finstack_ai_runtime::ports::journal::{JournalStore, StoreError, StoreLimits};
use finstack_ai_store_postgres::{PostgresJournalStore, PostgresStoreConfig};
use finstack_ai_test::store_fixtures::{draft, id, request};

use helpers::{SchemaGuard, connect, disposable_store, fresh_schema_name, pg_test_url};

/// Generous limits: tests that do not care about admission ceilings.
fn wide_limits() -> StoreLimits {
    StoreLimits {
        sessions: 1_000,
        batches_per_session: 1_000,
        records_per_session: 1_000,
        snapshot_bytes: 1_000_000,
    }
}

/// Open a store against an explicit schema name and limits.
///
/// `tests/helpers::disposable_store` hard-codes generous limits and a fresh
/// schema; the append suite needs both knobs (limit-admission tests need
/// tight ceilings, multi-writer tests need two stores over *one* schema).
async fn open_store(url: &str, schema: &str, limits: StoreLimits) -> PostgresJournalStore {
    let mut config = PostgresStoreConfig::new(url, limits);
    config.tls_mode = finstack_ai_store_postgres::PostgresTlsMode::Disable;
    config.schema = Arc::from(schema);
    PostgresJournalStore::try_open(config)
        .await
        .expect("try_open store")
}

/// Open one store over a fresh disposable schema, returning its cleanup guard.
async fn store_with_limits(
    url: &str,
    limits: StoreLimits,
) -> (PostgresJournalStore, String, SchemaGuard) {
    let schema = fresh_schema_name();
    let store = open_store(url, &schema, limits).await;
    (store, schema.clone(), SchemaGuard::new(schema))
}

/// Every record in `batch`, and the batch's own sequence span, chains from
/// `previous`; returns the batch's head checksum.
fn assert_batch_chains(batch: &CommittedBatch, previous: Option<Digest>) -> Digest {
    let mut expected_previous = previous;
    let mut sequence = batch.first_sequence;
    for envelope in batch.records.iter() {
        assert_eq!(envelope.sequence(), sequence, "records must be contiguous");
        assert_eq!(
            envelope.previous_checksum(),
            expected_previous,
            "record {sequence} must chain from the prior checksum"
        );
        expected_previous = Some(envelope.checksum());
        sequence += 1;
    }
    assert_eq!(batch.last_sequence, sequence - 1);
    batch
        .records
        .last()
        .map(RecordEnvelope::checksum)
        .expect("non-empty batch")
}

/// Happy path: two appends to one session return checksum-chained batches.
#[tokio::test]
async fn append_returns_checksum_chained_batches() {
    let Some(url) = pg_test_url() else {
        eprintln!("skipped: FINSTACK_PG_TEST_URL unset");
        return;
    };
    let (store, guard) = disposable_store(&url).await;

    let first = store
        .append(request(1, 1, 1, vec![draft(1, 1)]))
        .await
        .expect("first append");
    let head = assert_batch_chains(&first, None);

    let second = store
        .append(request(2, 1, 2, vec![draft(2, 1), draft(3, 1)]))
        .await
        .expect("second append");
    assert_eq!((second.first_sequence, second.last_sequence), (2, 3));
    assert_batch_chains(&second, Some(head));

    guard.cleanup(&connect(&url).await).await;
}

/// An identical retry replays the committed batch rather than appending
/// again.
#[tokio::test]
async fn identical_retry_replays_the_committed_batch() {
    let Some(url) = pg_test_url() else {
        eprintln!("skipped: FINSTACK_PG_TEST_URL unset");
        return;
    };
    let (store, schema, guard) = store_with_limits(&url, wide_limits()).await;

    let frozen = request(1, 1, 1, vec![draft(1, 1), draft(2, 1)]);
    let original = store.append(frozen.clone()).await.expect("first append");
    let replayed = store.append(frozen).await.expect("retry replays");
    assert_eq!(original, replayed, "retry must return the same batch");

    let client = connect(&url).await;
    let batches: i64 = client
        .query_one(&format!("SELECT COUNT(*) FROM {schema}.batches"), &[])
        .await
        .expect("count batches")
        .get(0);
    assert_eq!(batches, 1, "a replay must not persist a second batch");
    guard.cleanup(&client).await;
}

/// The same `batch_id` carrying a different draft is corruption, not a
/// replay.
#[tokio::test]
async fn reused_batch_id_with_a_different_draft_is_corruption() {
    let Some(url) = pg_test_url() else {
        eprintln!("skipped: FINSTACK_PG_TEST_URL unset");
        return;
    };
    let (store, guard) = disposable_store(&url).await;

    store
        .append(request(1, 1, 1, vec![draft(1, 1)]))
        .await
        .expect("first append");
    let error = store
        .append(request(1, 1, 2, vec![draft(2, 1)]))
        .await
        .expect_err("batch id reuse must fail");
    assert!(
        matches!(
            error,
            StoreError::Corruption {
                reason_code: "append_batch_id_reuse"
            }
        ),
        "unexpected error: {error:?}"
    );

    guard.cleanup(&connect(&url).await).await;
}

/// An append with no records is rejected before anything is written.
#[tokio::test]
async fn empty_batch_is_rejected() {
    let Some(url) = pg_test_url() else {
        eprintln!("skipped: FINSTACK_PG_TEST_URL unset");
        return;
    };
    let (store, schema, guard) = store_with_limits(&url, wide_limits()).await;

    let error = store
        .append(request(1, 1, 1, Vec::new()))
        .await
        .expect_err("empty batch must fail");
    assert!(
        matches!(
            error,
            StoreError::InvalidRequest {
                reason_code: "empty_append_batch"
            }
        ),
        "unexpected error: {error:?}"
    );

    let client = connect(&url).await;
    let sessions: i64 = client
        .query_one(&format!("SELECT COUNT(*) FROM {schema}.sessions"), &[])
        .await
        .expect("count sessions")
        .get(0);
    assert_eq!(
        sessions, 0,
        "a rejected empty batch must not create a session"
    );
    guard.cleanup(&client).await;
}

/// A stale `expected_sequence` conflicts and reports the real next sequence.
#[tokio::test]
async fn stale_expected_sequence_conflicts_with_the_actual_next() {
    let Some(url) = pg_test_url() else {
        eprintln!("skipped: FINSTACK_PG_TEST_URL unset");
        return;
    };
    let (store, guard) = disposable_store(&url).await;

    store
        .append(request(1, 1, 1, vec![draft(1, 1), draft(2, 1)]))
        .await
        .expect("first append");
    let error = store
        .append(request(2, 1, 2, vec![draft(3, 1)]))
        .await
        .expect_err("stale expected_sequence must conflict");
    assert!(
        matches!(
            error,
            StoreError::Conflict {
                expected_sequence: 2,
                actual_next_sequence: 3
            }
        ),
        "unexpected error: {error:?}"
    );

    guard.cleanup(&connect(&url).await).await;
}

/// Mirrors sqlite's `batch_and_record_idempotency_precede_sequence_checks`:
/// both idempotency checks run (and win) before the sequence precondition is
/// ever evaluated.
#[tokio::test]
async fn batch_and_record_idempotency_precede_sequence_checks() {
    let Some(url) = pg_test_url() else {
        eprintln!("skipped: FINSTACK_PG_TEST_URL unset");
        return;
    };
    let (store, guard) = disposable_store(&url).await;

    let frozen = request(10, 1, 1, vec![draft(10, 1)]);
    let original = store.append(frozen.clone()).await.expect("append");
    assert_eq!(
        store.append(frozen.clone()).await.expect("same batch"),
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
        store
            .append(same_records_new_batch)
            .await
            .expect("record idempotency"),
        original
    );

    let unequal_batch = request(10, 1, 2, vec![draft(11, 1)]);
    let error = store
        .append(unequal_batch)
        .await
        .expect_err("batch id reuse");
    assert!(
        matches!(
            error,
            StoreError::Corruption {
                reason_code: "append_batch_id_reuse"
            }
        ),
        "unexpected error: {error:?}"
    );

    let mixed = request(12, 1, 2, vec![draft(10, 1), draft(12, 1)]);
    let error = store.append(mixed).await.expect_err("mixed record reuse");
    assert!(
        matches!(
            error,
            StoreError::Corruption {
                reason_code: "mixed_record_id_reuse"
            }
        ),
        "unexpected error: {error:?}"
    );

    guard.cleanup(&connect(&url).await).await;
}

/// A request naming one already-committed record id *twice* is full (not
/// partial) reuse, so it fails the identity comparison with
/// `record_id_reuse` — the same reason code sqlite's per-record lookup loop
/// produces. Pins the multiplicity-preserving reuse lookup: a
/// `record_id = ANY(...)` predicate would collapse the two hits into one and
/// misreport this as `mixed_record_id_reuse`.
#[tokio::test]
async fn a_repeated_committed_record_id_is_full_reuse() {
    let Some(url) = pg_test_url() else {
        eprintln!("skipped: FINSTACK_PG_TEST_URL unset");
        return;
    };
    let (store, guard) = disposable_store(&url).await;

    store
        .append(request(1, 1, 1, vec![draft(1, 1)]))
        .await
        .expect("first append");
    let repeated = request(2, 1, 2, vec![draft(1, 1), draft(1, 1)]);
    let error = store.append(repeated).await.expect_err("record id reuse");
    assert!(
        matches!(
            error,
            StoreError::Corruption {
                reason_code: "record_id_reuse"
            }
        ),
        "unexpected error: {error:?}"
    );

    guard.cleanup(&connect(&url).await).await;
}

/// The `sessions` ceiling is enforced against the store-wide session count.
#[tokio::test]
async fn sessions_limit_is_enforced() {
    let Some(url) = pg_test_url() else {
        eprintln!("skipped: FINSTACK_PG_TEST_URL unset");
        return;
    };
    let limits = StoreLimits {
        sessions: 1,
        ..wide_limits()
    };
    let (store, _schema, guard) = store_with_limits(&url, limits).await;

    store
        .append(request(1, 1, 1, vec![draft(1, 1)]))
        .await
        .expect("first session");
    let error = store
        .append(request(2, 2, 1, vec![draft(2, 2)]))
        .await
        .expect_err("second session must exceed the ceiling");
    assert!(
        matches!(
            error,
            StoreError::LimitExceeded {
                resource: "sessions",
                limit: 1
            }
        ),
        "unexpected error: {error:?}"
    );

    guard.cleanup(&connect(&url).await).await;
}

/// The `batches_per_session` ceiling is enforced from the session row's
/// maintained `batch_count`.
#[tokio::test]
async fn batches_per_session_limit_is_enforced() {
    let Some(url) = pg_test_url() else {
        eprintln!("skipped: FINSTACK_PG_TEST_URL unset");
        return;
    };
    let limits = StoreLimits {
        batches_per_session: 1,
        ..wide_limits()
    };
    let (store, _schema, guard) = store_with_limits(&url, limits).await;

    store
        .append(request(1, 1, 1, vec![draft(1, 1)]))
        .await
        .expect("first batch");
    let error = store
        .append(request(2, 1, 2, vec![draft(2, 1)]))
        .await
        .expect_err("second batch must exceed the ceiling");
    assert!(
        matches!(
            error,
            StoreError::LimitExceeded {
                resource: "batches_per_session",
                limit: 1
            }
        ),
        "unexpected error: {error:?}"
    );

    guard.cleanup(&connect(&url).await).await;
}

/// The `records_per_session` ceiling is enforced from the session row's
/// maintained `record_count`, and is checked *after* the batch ceiling.
#[tokio::test]
async fn records_per_session_limit_is_enforced() {
    let Some(url) = pg_test_url() else {
        eprintln!("skipped: FINSTACK_PG_TEST_URL unset");
        return;
    };
    let limits = StoreLimits {
        records_per_session: 2,
        ..wide_limits()
    };
    let (store, _schema, guard) = store_with_limits(&url, limits).await;

    store
        .append(request(1, 1, 1, vec![draft(1, 1), draft(2, 1)]))
        .await
        .expect("first batch fills the record ceiling");
    let error = store
        .append(request(2, 1, 3, vec![draft(3, 1)]))
        .await
        .expect_err("third record must exceed the ceiling");
    assert!(
        matches!(
            error,
            StoreError::LimitExceeded {
                resource: "records_per_session",
                limit: 2
            }
        ),
        "unexpected error: {error:?}"
    );

    guard.cleanup(&connect(&url).await).await;
}

/// Append `ordinal`'s single-record batch to session 1, retrying on the
/// conflicts and retryable serialization failures a second concurrent writer
/// causes. Mirrors the caller-side retry loop the store's contract expects
/// (spec D4: the store never retries internally).
async fn append_with_retry(store: &PostgresJournalStore, ordinal: u64) -> CommittedBatch {
    let mut expected_sequence = 1;
    loop {
        let frozen = request(ordinal, 1, expected_sequence, vec![draft(ordinal, 1)]);
        match store.append(frozen).await {
            Ok(batch) => return batch,
            Err(StoreError::Conflict {
                actual_next_sequence,
                ..
            }) => expected_sequence = actual_next_sequence,
            Err(StoreError::Unavailable {
                reason_code: "postgres_serialization",
            }) => {}
            Err(error) => panic!("unexpected append error: {error:?}"),
        }
    }
}

/// Two independent stores (two pools, one schema) interleaving 16 appends
/// each produce one linear 32-batch chain with every checksum linking.
#[tokio::test]
async fn two_stores_interleaving_appends_produce_one_linear_chain() {
    let Some(url) = pg_test_url() else {
        eprintln!("skipped: FINSTACK_PG_TEST_URL unset");
        return;
    };
    let schema = fresh_schema_name();
    let guard = SchemaGuard::new(schema.clone());
    let store_a = open_store(&url, &schema, wide_limits()).await;
    let store_b = open_store(&url, &schema, wide_limits()).await;

    let writer_a = async {
        for ordinal in 100..116 {
            append_with_retry(&store_a, ordinal).await;
        }
    };
    let writer_b = async {
        for ordinal in 200..216 {
            append_with_retry(&store_b, ordinal).await;
        }
    };
    tokio::join!(writer_a, writer_b);

    let client = connect(&url).await;
    let rows = client
        .query(
            &format!(
                "SELECT sequence, previous_checksum, envelope_checksum \
                 FROM {schema}.records ORDER BY sequence"
            ),
            &[],
        )
        .await
        .expect("read the journal");
    assert_eq!(
        rows.len(),
        32,
        "every append must be committed exactly once"
    );
    let mut expected_previous: Option<Vec<u8>> = None;
    for (index, row) in rows.iter().enumerate() {
        let sequence: i64 = row.get(0);
        let previous: Option<Vec<u8>> = row.get(1);
        let checksum: Vec<u8> = row.get(2);
        assert_eq!(
            sequence,
            i64::try_from(index + 1).expect("sequence"),
            "the journal must be one gapless linear chain"
        );
        assert_eq!(previous, expected_previous, "checksum {sequence} must link");
        expected_previous = Some(checksum);
    }

    let batches: i64 = client
        .query_one(&format!("SELECT COUNT(*) FROM {schema}.batches"), &[])
        .await
        .expect("count batches")
        .get(0);
    assert_eq!(batches, 32);
    let (batch_count, record_count): (i64, i64) = {
        let row = client
            .query_one(
                &format!("SELECT batch_count, record_count FROM {schema}.sessions"),
                &[],
            )
            .await
            .expect("read session counters");
        (row.get(0), row.get(1))
    };
    assert_eq!(
        (batch_count, record_count),
        (32, 32),
        "session counters must track the committed footprint"
    );

    guard.cleanup(&client).await;
}

/// Two stores submitting the *same* request concurrently both observe the
/// same committed batch: one commits it, the other replays it.
#[tokio::test]
async fn concurrent_identical_requests_both_return_the_same_batch() {
    let Some(url) = pg_test_url() else {
        eprintln!("skipped: FINSTACK_PG_TEST_URL unset");
        return;
    };
    let schema = fresh_schema_name();
    let guard = SchemaGuard::new(schema.clone());
    let store_a = open_store(&url, &schema, wide_limits()).await;
    let store_b = open_store(&url, &schema, wide_limits()).await;

    let frozen = request(7, 1, 1, vec![draft(7, 1), draft(8, 1)]);
    let (result_a, result_b) = tokio::join!(
        store_a.append(frozen.clone()),
        store_b.append(frozen.clone()),
    );
    let batch_a = result_a.expect("writer a");
    let batch_b = result_b.expect("writer b");
    assert_eq!(batch_a, batch_b, "both writers must observe one batch");

    let client = connect(&url).await;
    let records: i64 = client
        .query_one(&format!("SELECT COUNT(*) FROM {schema}.records"), &[])
        .await
        .expect("count records")
        .get(0);
    assert_eq!(records, 2, "the batch must be committed exactly once");
    let sessions: i64 = client
        .query_one(
            &format!("SELECT session_count FROM {schema}.store_totals"),
            &[],
        )
        .await
        .expect("read store totals")
        .get(0);
    assert_eq!(sessions, 1, "the session must be counted exactly once");

    guard.cleanup(&client).await;
}
