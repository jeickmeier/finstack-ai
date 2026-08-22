//! Server-gated integration tests for `PostgresJournalStore::try_open` and
//! `health()` (spec D2/D6).
//!
//! Skips with a notice (exit 0) when `FINSTACK_PG_TEST_URL` is unset, per
//! the crate's env-gated testing convention (spec D10). The unroutable-URL
//! test does not need a live server and always runs.

mod helpers;

use std::sync::Arc;
use std::time::Duration;

use finstack_ai_runtime::ports::journal::{JournalStore, StoreError, StoreLimits};
use finstack_ai_store_postgres::{PostgresDurability, PostgresJournalStore, PostgresStoreConfig};

use helpers::{connect, disposable_store, pg_test_url};

fn limits() -> StoreLimits {
    StoreLimits {
        sessions: 1_000,
        batches_per_session: 1_000,
        records_per_session: 1_000,
        snapshot_bytes: 1_000_000,
    }
}

/// `try_open` against a disposable schema succeeds, and `health()` reports
/// `ready: true`, `durable: true` with the durable detail string under the
/// default (`Durable`) config.
#[tokio::test]
async fn try_open_succeeds_and_health_reports_durable() {
    let Some(url) = pg_test_url() else {
        eprintln!("skipped: FINSTACK_PG_TEST_URL unset");
        return;
    };
    let (store, guard) = disposable_store(&url).await;

    let health = store.health().await.expect("health() should not error");
    assert!(health.ready, "health() should report ready after try_open");
    assert!(health.durable, "Durable config should report durable: true");
    assert_eq!(&*health.detail, "postgres synchronous_commit=on");

    let cleanup_client = connect(&url).await;
    guard.cleanup(&cleanup_client).await;
}

/// `PostgresDurability::Relaxed` reports `durable: false` with the relaxed
/// detail string — the store must never claim durability it does not
/// provide.
#[tokio::test]
async fn relaxed_durability_reports_not_durable() {
    let Some(url) = pg_test_url() else {
        eprintln!("skipped: FINSTACK_PG_TEST_URL unset");
        return;
    };
    let schema = helpers::fresh_schema_name();
    let mut config = PostgresStoreConfig::new(url.clone(), limits());
    config.tls_mode = finstack_ai_store_postgres::PostgresTlsMode::Disable;
    config.durability = PostgresDurability::Relaxed;
    config.schema = Arc::from(schema.as_str());

    let store = PostgresJournalStore::try_open(config)
        .await
        .expect("try_open should succeed under Relaxed durability");

    let health = store.health().await.expect("health() should not error");
    assert!(health.ready);
    assert!(!health.durable, "Relaxed config must report durable: false");
    assert_eq!(&*health.detail, "postgres synchronous_commit=off");

    let cleanup_client = connect(&url).await;
    cleanup_client
        .batch_execute(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"))
        .await
        .expect("drop disposable test schema");
}

/// An unroutable connection target with a short `connect_timeout` fails
/// `try_open` with `Unavailable{postgres_unavailable}`. Does not need a live
/// server, so it always runs (not gated on `FINSTACK_PG_TEST_URL`).
#[tokio::test]
async fn unroutable_url_fails_open_as_unavailable() {
    let mut config = PostgresStoreConfig::new("postgres://127.0.0.1:1@/x", limits());
    config.tls_mode = finstack_ai_store_postgres::PostgresTlsMode::Disable;
    config.connect_timeout = Duration::from_millis(200);

    let result = PostgresJournalStore::try_open(config).await;
    let Err(error) = result else {
        panic!("an unroutable URL must fail try_open");
    };

    assert!(
        matches!(
            error,
            StoreError::Unavailable {
                reason_code: "postgres_unavailable"
            }
        ),
        "unexpected error: {error:?}"
    );
}
