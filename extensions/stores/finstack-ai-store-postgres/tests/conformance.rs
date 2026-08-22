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
use finstack_ai_test::{JournalStoreConformanceCase, check_journal_store_conformance};

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
