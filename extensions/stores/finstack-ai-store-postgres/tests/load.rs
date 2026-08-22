//! Server-gated integration tests for the load path: full loads, chain
//! verification (spec D9), the `VerifiedHead` suffix cache, and the
//! `load_from` windows.
//!
//! Every test skips with a notice (exit 0) when `FINSTACK_PG_TEST_URL` is
//! unset, per the crate's env-gated testing convention (spec D10).
//!
//! The window semantics asserted here are the ones
//! `finstack-ai-store-common` defines (`FROM_SEQUENCE_WINDOW`,
//! `SNAPSHOT_WINDOW`), so the reason codes must match sqlite's suite in
//! `extensions/stores/finstack-ai-store-sqlite/src/tests.rs` exactly.

mod helpers;

use std::sync::Arc;

use finstack_ai_kernel::{Digest, SessionTag};
use finstack_ai_runtime::ports::journal::{
    JournalStore, LoadFromRequest, LoadRequest, LoadWindow, LoadedSession, StoreError, StoreLimits,
};
use finstack_ai_store_postgres::{PostgresJournalStore, PostgresStoreConfig};
use finstack_ai_test::store_fixtures::{draft, id, request};

use helpers::{SchemaGuard, connect, disposable_store, fresh_schema_name, pg_test_url};

/// Generous limits: these tests care about verification, not ceilings.
fn wide_limits() -> StoreLimits {
    StoreLimits {
        sessions: 1_000,
        batches_per_session: 1_000,
        records_per_session: 1_000,
        snapshot_bytes: 1_000_000,
    }
}

/// Open a store against an explicit schema name (two stores over one schema
/// is what the multi-writer test needs).
async fn open_store(url: &str, schema: &str) -> PostgresJournalStore {
    let mut config = PostgresStoreConfig::new(url, wide_limits());
    config.tls_mode = finstack_ai_store_postgres::PostgresTlsMode::Disable;
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

/// Append the standard three-batch journal to session 1: sequences
/// `1 | 2,3 | 4`.
async fn append_three_batches(store: &PostgresJournalStore) {
    store
        .append(request(1, 1, 1, vec![draft(1, 1)]))
        .await
        .expect("batch one");
    store
        .append(request(2, 1, 2, vec![draft(2, 1), draft(3, 1)]))
        .await
        .expect("batch two");
    store
        .append(request(3, 1, 4, vec![draft(4, 1)]))
        .await
        .expect("batch three");
}

/// Load session 1 in full.
async fn load_session_one(store: &PostgresJournalStore) -> Result<LoadedSession, StoreError> {
    store
        .load(LoadRequest {
            session_id: id::<SessionTag>(1),
        })
        .await
}

/// The checksum stored for `sequence` in session 1.
async fn stored_checksum(client: &tokio_postgres::Client, schema: &str, sequence: i64) -> Vec<u8> {
    client
        .query_one(
            &format!(
                "SELECT envelope_checksum FROM {schema}.records \
                 WHERE session_id = $1 AND sequence = $2"
            ),
            &[&id::<SessionTag>(1).as_bytes().as_slice(), &sequence],
        )
        .await
        .expect("read stored checksum")
        .get(0)
}

/// Overwrite the checksum stored for `sequence` in session 1.
async fn set_checksum(client: &tokio_postgres::Client, schema: &str, sequence: i64, bytes: &[u8]) {
    client
        .execute(
            &format!(
                "UPDATE {schema}.records SET envelope_checksum = $1 \
                 WHERE session_id = $2 AND sequence = $3"
            ),
            &[
                &bytes,
                &id::<SessionTag>(1).as_bytes().as_slice(),
                &sequence,
            ],
        )
        .await
        .expect("rewrite stored checksum");
}

/// A session that was never appended to loads as an empty journal, not an
/// error.
#[tokio::test]
async fn load_of_an_unknown_session_is_empty() {
    let Some(url) = pg_test_url() else {
        eprintln!("skipped: FINSTACK_PG_TEST_URL unset");
        return;
    };
    let (store, guard) = disposable_store(&url).await;

    let loaded = load_session_one(&store).await.expect("load");
    assert_eq!(loaded, LoadedSession::empty(id::<SessionTag>(1)));

    guard.cleanup(&connect(&url).await).await;
}

/// A full load returns every committed batch on its stored boundaries, with
/// the session head.
#[tokio::test]
async fn load_returns_every_committed_batch_on_its_stored_boundaries() {
    let Some(url) = pg_test_url() else {
        eprintln!("skipped: FINSTACK_PG_TEST_URL unset");
        return;
    };
    let (store, guard) = disposable_store(&url).await;
    append_three_batches(&store).await;

    let loaded = load_session_one(&store).await.expect("load");
    assert_eq!(loaded.session_id, id::<SessionTag>(1));
    assert_eq!(loaded.head_sequence, 4);
    assert_eq!(loaded.committed_batches.len(), 3);
    let spans = loaded
        .committed_batches
        .iter()
        .map(|batch| (batch.first_sequence, batch.last_sequence))
        .collect::<Vec<_>>();
    assert_eq!(spans, vec![(1, 1), (2, 3), (4, 4)]);
    let head = loaded
        .committed_batches
        .last()
        .and_then(|batch| batch.records.last())
        .map(finstack_ai_kernel::RecordEnvelope::checksum);
    assert_eq!(loaded.head_checksum, head);
    assert!(!loaded.omits_prefix());
    assert!(loaded.snapshot.is_none());
    assert!(loaded.accelerated.is_none());

    guard.cleanup(&connect(&url).await).await;
}

/// A tampered `envelope_checksum` fails the load *and* drops the cached
/// verified head.
///
/// The drop is what the third load discriminates: after the head record is
/// repaired, a *mid-journal* record is corrupted instead. A surviving cache
/// entry would anchor on the (now valid again) head record and suffix-verify
/// nothing, masking that corruption; only a genuinely dropped entry forces
/// the full re-verification that catches it.
#[tokio::test]
async fn a_corrupt_envelope_checksum_fails_the_load_and_drops_the_cache() {
    let Some(url) = pg_test_url() else {
        eprintln!("skipped: FINSTACK_PG_TEST_URL unset");
        return;
    };
    let (store, schema, guard) = store_over_fresh_schema(&url).await;
    append_three_batches(&store).await;
    let client = connect(&url).await;

    // First load verifies the full chain and caches the verified head.
    let clean = load_session_one(&store).await.expect("first load");

    // Corrupting the head record breaks the load however it is verified, so
    // this step establishes only that the load fails and the entry is gone.
    let head_bytes = stored_checksum(&client, &schema, 4).await;
    set_checksum(&client, &schema, 4, &[0xAB_u8; 32]).await;
    let error = load_session_one(&store)
        .await
        .expect_err("a broken head must fail the load");
    assert!(
        matches!(error, StoreError::Integrity { .. }),
        "unexpected error: {error:?}"
    );

    // Repair the head and break sequence 2 instead: reachable only by a
    // verification that actually walks the chain again.
    set_checksum(&client, &schema, 4, &head_bytes).await;
    let middle_bytes = stored_checksum(&client, &schema, 2).await;
    set_checksum(&client, &schema, 2, &[0xCD_u8; 32]).await;
    let error = load_session_one(&store)
        .await
        .expect_err("mid-journal corruption must be caught after the cache drop");
    assert!(
        matches!(error, StoreError::Integrity { .. }),
        "unexpected error: {error:?}"
    );

    // Fully repaired, the journal loads exactly as it did before.
    set_checksum(&client, &schema, 2, &middle_bytes).await;
    let repaired = load_session_one(&store).await.expect("repaired load");
    assert_eq!(repaired, clean);

    guard.cleanup(&client).await;
}

/// `FromSequence` at a batch boundary returns only the suffix.
#[tokio::test]
async fn load_from_a_batch_boundary_returns_only_the_suffix() {
    let Some(url) = pg_test_url() else {
        eprintln!("skipped: FINSTACK_PG_TEST_URL unset");
        return;
    };
    let (store, guard) = disposable_store(&url).await;
    append_three_batches(&store).await;
    let full = load_session_one(&store).await.expect("full load");
    let prior = full.committed_batches[0].records[0].checksum();

    let loaded = store
        .load_from(LoadFromRequest {
            session_id: id::<SessionTag>(1),
            window: LoadWindow::FromSequence {
                from_sequence: 2,
                prior_checksum: prior,
            },
        })
        .await
        .expect("load_from");
    assert_eq!(loaded.head_sequence, 4);
    assert_eq!(loaded.head_checksum, full.head_checksum);
    assert_eq!(loaded.first_loaded_sequence(), 2);
    assert!(loaded.omits_prefix());
    let spans = loaded
        .committed_batches
        .iter()
        .map(|batch| (batch.first_sequence, batch.last_sequence))
        .collect::<Vec<_>>();
    assert_eq!(spans, vec![(2, 3), (4, 4)]);

    guard.cleanup(&connect(&url).await).await;
}

/// A `FromSequence` window whose prior checksum does not match the record
/// before the window fails closed.
#[tokio::test]
async fn load_from_with_a_wrong_prior_checksum_is_integrity() {
    let Some(url) = pg_test_url() else {
        eprintln!("skipped: FINSTACK_PG_TEST_URL unset");
        return;
    };
    let (store, guard) = disposable_store(&url).await;
    append_three_batches(&store).await;

    let error = store
        .load_from(LoadFromRequest {
            session_id: id::<SessionTag>(1),
            window: LoadWindow::FromSequence {
                from_sequence: 2,
                prior_checksum: Digest::raw_json(b"{}"),
            },
        })
        .await
        .expect_err("a wrong prior checksum must fail");
    assert!(
        matches!(
            error,
            StoreError::Integrity {
                reason_code: "load_from_prior_checksum_mismatch"
            }
        ),
        "unexpected error: {error:?}"
    );

    guard.cleanup(&connect(&url).await).await;
}

/// A `FromSequence` window starting inside a committed batch is a split.
#[tokio::test]
async fn load_from_inside_a_committed_batch_splits_it() {
    let Some(url) = pg_test_url() else {
        eprintln!("skipped: FINSTACK_PG_TEST_URL unset");
        return;
    };
    let (store, guard) = disposable_store(&url).await;
    append_three_batches(&store).await;
    let full = load_session_one(&store).await.expect("full load");
    // Sequence 3 sits strictly inside the batch covering sequences 2-3.
    let prior = full.committed_batches[1].records[0].checksum();

    let error = store
        .load_from(LoadFromRequest {
            session_id: id::<SessionTag>(1),
            window: LoadWindow::FromSequence {
                from_sequence: 3,
                prior_checksum: prior,
            },
        })
        .await
        .expect_err("a mid-batch start must fail");
    assert!(
        matches!(
            error,
            StoreError::Integrity {
                reason_code: "load_from_splits_batch"
            }
        ),
        "unexpected error: {error:?}"
    );

    guard.cleanup(&connect(&url).await).await;
}

/// A `FromSequence` window starting past the head is a gap.
#[tokio::test]
async fn load_from_past_the_head_is_a_gap() {
    let Some(url) = pg_test_url() else {
        eprintln!("skipped: FINSTACK_PG_TEST_URL unset");
        return;
    };
    let (store, guard) = disposable_store(&url).await;
    append_three_batches(&store).await;
    let full = load_session_one(&store).await.expect("full load");
    let head = full.head_checksum.expect("head checksum");

    let error = store
        .load_from(LoadFromRequest {
            session_id: id::<SessionTag>(1),
            window: LoadWindow::FromSequence {
                from_sequence: 10,
                prior_checksum: head,
            },
        })
        .await
        .expect_err("a missing start must fail");
    assert!(
        matches!(
            error,
            StoreError::Integrity {
                reason_code: "load_from_sequence_gap"
            }
        ),
        "unexpected error: {error:?}"
    );

    guard.cleanup(&connect(&url).await).await;
}

/// `SnapshotPlusTail` falls back to a full load when the session has no
/// snapshot row.
#[tokio::test]
async fn snapshot_plus_tail_without_a_snapshot_loads_the_whole_journal() {
    let Some(url) = pg_test_url() else {
        eprintln!("skipped: FINSTACK_PG_TEST_URL unset");
        return;
    };
    let (store, guard) = disposable_store(&url).await;
    append_three_batches(&store).await;
    let full = load_session_one(&store).await.expect("full load");

    let loaded = store
        .load_from(LoadFromRequest {
            session_id: id::<SessionTag>(1),
            window: LoadWindow::SnapshotPlusTail,
        })
        .await
        .expect("snapshot plus tail");
    assert_eq!(loaded, full);
    assert!(!loaded.omits_prefix());

    guard.cleanup(&connect(&url).await).await;
}

/// The verified-head cache is suffix-safe under multi-writer: a batch
/// appended by a *second* store instance is verified as a new tail on the
/// first store's next load, and the load returns the extended journal.
#[tokio::test]
async fn a_second_store_instances_append_is_verified_as_a_new_tail() {
    let Some(url) = pg_test_url() else {
        eprintln!("skipped: FINSTACK_PG_TEST_URL unset");
        return;
    };
    let schema = fresh_schema_name();
    let guard = SchemaGuard::new(schema.clone());
    let store_a = open_store(&url, &schema).await;
    let store_b = open_store(&url, &schema).await;

    store_a
        .append(request(1, 1, 1, vec![draft(1, 1)]))
        .await
        .expect("store a append");
    let first = load_session_one(&store_a).await.expect("first load");
    assert_eq!(first.head_sequence, 1);

    let appended = store_b
        .append(request(2, 1, 2, vec![draft(2, 1), draft(3, 1)]))
        .await
        .expect("store b append");
    let extended = load_session_one(&store_a).await.expect("second load");
    assert_eq!(extended.head_sequence, 3);
    assert_eq!(extended.committed_batches.len(), 2);
    assert_eq!(
        extended.head_checksum,
        appended
            .records
            .last()
            .map(finstack_ai_kernel::RecordEnvelope::checksum)
    );

    guard.cleanup(&connect(&url).await).await;
}
