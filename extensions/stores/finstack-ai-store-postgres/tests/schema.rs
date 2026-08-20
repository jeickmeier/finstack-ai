//! Server-gated integration tests for `ensure_schema` (spec D3/D8).
//!
//! Skips with a notice (exit 0) when `FINSTACK_PG_TEST_URL` is unset, per
//! the crate's env-gated testing convention (spec D10).

mod helpers;

use finstack_ai_runtime::StoreError;
use finstack_ai_store_postgres::SchemaPolicy;
use finstack_ai_store_postgres::schema::{SCHEMA_VERSION, ensure_schema};

use helpers::{SchemaGuard, connect, fresh_schema_name, pg_test_url};

/// A fresh schema: `ensure_schema` under `Manage` creates every v1 table
/// and writes schema version 1.
#[tokio::test]
async fn manage_creates_schema_and_writes_version() {
    let Some(url) = pg_test_url() else {
        eprintln!("skipped: FINSTACK_PG_TEST_URL unset");
        return;
    };
    let client = connect(&url).await;
    let schema = fresh_schema_name();
    let guard = SchemaGuard::new(schema.clone());

    ensure_schema(&client, &schema, SchemaPolicy::Manage)
        .await
        .expect("ensure_schema should create the schema");

    for table in [
        "sessions",
        "batches",
        "records",
        "snapshots",
        "store_totals",
        "fa_schema_version",
    ] {
        let exists: bool = client
            .query_one(
                "SELECT EXISTS (SELECT 1 FROM information_schema.tables \
                 WHERE table_schema = $1 AND table_name = $2)",
                &[&schema.as_str(), &table],
            )
            .await
            .expect("check table existence")
            .get(0);
        assert!(exists, "table {table} should exist after ensure_schema");
    }

    let version: i32 = client
        .query_one(
            &format!("SELECT version FROM {schema}.fa_schema_version"),
            &[],
        )
        .await
        .expect("read fa_schema_version")
        .get(0);
    assert_eq!(version, SCHEMA_VERSION);

    let session_count: i64 = client
        .query_one(
            &format!("SELECT session_count FROM {schema}.store_totals"),
            &[],
        )
        .await
        .expect("read store_totals")
        .get(0);
    assert_eq!(session_count, 0);

    guard.cleanup(&client).await;
}

/// A second `ensure_schema` call against an already-migrated schema is a
/// no-op: it succeeds without re-applying DDL.
#[tokio::test]
async fn second_call_is_idempotent() {
    let Some(url) = pg_test_url() else {
        eprintln!("skipped: FINSTACK_PG_TEST_URL unset");
        return;
    };
    let client = connect(&url).await;
    let schema = fresh_schema_name();
    let guard = SchemaGuard::new(schema.clone());

    ensure_schema(&client, &schema, SchemaPolicy::Manage)
        .await
        .expect("first ensure_schema should succeed");
    ensure_schema(&client, &schema, SchemaPolicy::Manage)
        .await
        .expect("second ensure_schema should be a no-op success");

    let version: i32 = client
        .query_one(
            &format!("SELECT version FROM {schema}.fa_schema_version"),
            &[],
        )
        .await
        .expect("read fa_schema_version")
        .get(0);
    assert_eq!(version, SCHEMA_VERSION);

    guard.cleanup(&client).await;
}

/// A schema whose `fa_schema_version` row was hand-set to an unsupported
/// version fails closed rather than being read forward.
#[tokio::test]
async fn unsupported_version_fails_closed() {
    let Some(url) = pg_test_url() else {
        eprintln!("skipped: FINSTACK_PG_TEST_URL unset");
        return;
    };
    let client = connect(&url).await;
    let schema = fresh_schema_name();
    let guard = SchemaGuard::new(schema.clone());

    ensure_schema(&client, &schema, SchemaPolicy::Manage)
        .await
        .expect("initial ensure_schema should succeed");

    client
        .execute(
            &format!("UPDATE {schema}.fa_schema_version SET version = 2"),
            &[],
        )
        .await
        .expect("hand-set schema version to 2");

    let error = ensure_schema(&client, &schema, SchemaPolicy::Manage)
        .await
        .expect_err("unsupported version should fail closed");
    assert!(
        matches!(
            error,
            StoreError::Integrity {
                reason_code: "postgres_schema_unsupported"
            }
        ),
        "unexpected error: {error:?}"
    );

    guard.cleanup(&client).await;
}

/// `SchemaPolicy::Require` on an empty (unmigrated) schema fails closed
/// without issuing any DDL.
#[tokio::test]
async fn require_policy_fails_on_missing_schema() {
    let Some(url) = pg_test_url() else {
        eprintln!("skipped: FINSTACK_PG_TEST_URL unset");
        return;
    };
    let client = connect(&url).await;
    let schema = fresh_schema_name();
    let guard = SchemaGuard::new(schema.clone());

    let error = ensure_schema(&client, &schema, SchemaPolicy::Require)
        .await
        .expect_err("Require policy should fail on a missing schema");
    assert!(
        matches!(
            error,
            StoreError::Unavailable {
                reason_code: "postgres_schema_missing"
            }
        ),
        "unexpected error: {error:?}"
    );

    let exists: bool = client
        .query_one(
            "SELECT EXISTS (SELECT 1 FROM information_schema.schemata WHERE schema_name = $1)",
            &[&schema.as_str()],
        )
        .await
        .expect("check schema existence")
        .get(0);
    assert!(!exists, "Require policy must not create the schema");

    guard.cleanup(&client).await;
}

/// Two concurrent `ensure_schema` calls against the same schema, over two
/// independent connections, both succeed: the advisory `pg_advisory_xact_lock`
/// serializes the DDL rather than racing it.
#[tokio::test]
async fn concurrent_ensure_schema_calls_both_succeed() {
    let Some(url) = pg_test_url() else {
        eprintln!("skipped: FINSTACK_PG_TEST_URL unset");
        return;
    };
    let client_a = connect(&url).await;
    let client_b = connect(&url).await;
    let cleanup_client = connect(&url).await;
    let schema = fresh_schema_name();
    let guard = SchemaGuard::new(schema.clone());

    let (result_a, result_b) = tokio::join!(
        ensure_schema(&client_a, &schema, SchemaPolicy::Manage),
        ensure_schema(&client_b, &schema, SchemaPolicy::Manage),
    );
    result_a.expect("first concurrent ensure_schema should succeed");
    result_b.expect("second concurrent ensure_schema should succeed");

    let version: i32 = cleanup_client
        .query_one(
            &format!("SELECT version FROM {schema}.fa_schema_version"),
            &[],
        )
        .await
        .expect("read fa_schema_version")
        .get(0);
    assert_eq!(version, SCHEMA_VERSION);

    guard.cleanup(&cleanup_client).await;
}
