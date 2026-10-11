//! Server-gated shared `JournalStore` port conformance suite over the
//! Postgres store.
//!
//! Runs `finstack_ai_test::check_journal_store_conformance` — readiness,
//! atomic append, equal-retry idempotency, and load-contains-the-committed
//! -batch — against a disposable-schema `PostgresJournalStore`. Skips with a
//! notice (exit 0) when `FINSTACK_PG_TEST_URL` is unset, per the crate's
//! env-gated testing convention (spec D10).

mod helpers;

use finstack_ai_test::store_fixtures::{draft, request};
use finstack_ai_test::{
    JournalStoreConformanceCase, PruneBoundaryConformanceCase, check_journal_store_conformance,
    check_prune_boundary_conformance,
};

use helpers::{connect, disposable_store, pg_test_url};

/// The shared `JournalStore` conformance battery passes against a fresh
/// disposable-schema Postgres store.
#[tokio::test]
async fn postgres_store_passes_journal_store_conformance() {
    let Some(url) = pg_test_url() else {
        eprintln!("skipped: FINSTACK_PG_TEST_URL unset");
        return;
    };
    let (store, guard) = disposable_store(&url).await;

    let committed = check_journal_store_conformance(
        &store,
        JournalStoreConformanceCase {
            request: request(1, 1, 1, vec![draft(1, 1)]),
            expected_first_sequence: 1,
            expected_last_sequence: 1,
        },
    )
    .await
    .expect("postgres journal store conformance");
    assert_eq!(committed.records.len(), 1);
    assert_eq!(committed.first_sequence, 1);
    assert_eq!(committed.last_sequence, 1);

    let cleanup_client = connect(&url).await;
    guard.cleanup(&cleanup_client).await;
}

/// After a snapshot-aligned prune, the Postgres store reports the retained
/// journal in the shared shape: the boundary batch (whose last record is
/// kept as the chain anchor) is omitted from `committed_batches`, exactly
/// as the in-memory and sqlite stores report it.
#[tokio::test]
async fn postgres_store_omits_the_prune_boundary_batch() {
    let Some(url) = pg_test_url() else {
        eprintln!("skipped: FINSTACK_PG_TEST_URL unset");
        return;
    };
    let (store, guard) = disposable_store(&url).await;

    let loaded = check_prune_boundary_conformance(
        &store,
        PruneBoundaryConformanceCase {
            prefix: request(1, 1, 1, vec![draft(1, 1), draft(2, 1)]),
            boundary: request(2, 1, 3, vec![draft(3, 1)]),
            tail: request(3, 1, 4, vec![draft(4, 1), draft(5, 1)]),
            snapshot_bytes: 4_096,
        },
    )
    .await
    .expect("postgres prune boundary conformance");
    assert_eq!(loaded.head_sequence, 5);
    assert_eq!(loaded.committed_batches.len(), 1);

    let cleanup_client = connect(&url).await;
    guard.cleanup(&cleanup_client).await;
}
