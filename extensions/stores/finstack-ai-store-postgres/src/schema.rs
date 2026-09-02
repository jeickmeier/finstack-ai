//! Schema v1 DDL and advisory-locked, fail-closed migrations.
//!
//! Mirrors `extensions/stores/finstack-ai-store-sqlite/src/schema.rs` v1
//! column-for-column (`BLOB`→`BYTEA`, `INTEGER`→`BIGINT` except
//! `format_version`/`kind_version`, same PK/unique/index set) plus the
//! multi-writer bookkeeping columns/tables documented in spec D3/D8.
//!
//! [`ensure_schema`] and [`SCHEMA_VERSION`] are `pub(crate)` per spec
//! D3/D8: the only production caller is
//! [`crate::store::PostgresJournalStore::try_open`].

use crate::config::SchemaPolicy;
use crate::error::map_postgres_error;
use finstack_ai_runtime::ports::journal::StoreError;

/// Schema version written to `fa_schema_version` once v1 DDL has been
/// applied. Any other stored version is treated as unsupported and fails
/// closed rather than being read forward.
pub(crate) const SCHEMA_VERSION: i32 = 1;

/// Render the v1 DDL for the given (already-validated) schema name.
///
/// `schema` must have already passed
/// [`crate::config::PostgresStoreConfig::validate`] (or the private
/// identifier grammar it enforces): Postgres has no way to bind an
/// identifier as a query parameter, so this text is interpolated directly.
/// Callers must never pass an unvalidated schema name here.
fn v1_ddl(schema: &str) -> String {
    format!(
        "
CREATE TABLE {schema}.sessions (
  session_id BYTEA PRIMARY KEY,
  current_sequence BIGINT NOT NULL,
  head_checksum BYTEA,
  chain_anchor_sequence BIGINT NOT NULL DEFAULT 1,
  chain_anchor_checksum BYTEA,
  snapshot_sequence BIGINT,
  metadata BYTEA NOT NULL,
  batch_count BIGINT NOT NULL DEFAULT 0,
  record_count BIGINT NOT NULL DEFAULT 0
);
CREATE TABLE {schema}.batches (
  batch_id BYTEA PRIMARY KEY,
  session_id BYTEA NOT NULL REFERENCES {schema}.sessions(session_id),
  first_sequence BIGINT NOT NULL,
  last_sequence BIGINT NOT NULL,
  expected_sequence BIGINT NOT NULL,
  request_cbor BYTEA NOT NULL,
  UNIQUE (session_id, first_sequence)
);
CREATE TABLE {schema}.records (
  session_id BYTEA NOT NULL,
  sequence BIGINT NOT NULL,
  record_id BYTEA NOT NULL UNIQUE,
  batch_id BYTEA NOT NULL REFERENCES {schema}.batches(batch_id),
  lane_id BYTEA NOT NULL,
  run_id BYTEA,
  kind TEXT NOT NULL,
  format_version INTEGER NOT NULL,
  kind_version INTEGER NOT NULL,
  payload_cbor BYTEA NOT NULL,
  timestamp BIGINT NOT NULL,
  committed_at BIGINT,
  payload_digest BYTEA NOT NULL,
  previous_checksum BYTEA,
  envelope_checksum BYTEA NOT NULL,
  derived_event_ids BYTEA NOT NULL,
  PRIMARY KEY (session_id, sequence)
);
CREATE TABLE {schema}.snapshots (
  session_id BYTEA PRIMARY KEY,
  sequence BIGINT NOT NULL,
  payload_cbor BYTEA NOT NULL,
  digest BYTEA NOT NULL,
  timestamp BIGINT NOT NULL
);
CREATE INDEX records_batch_id ON {schema}.records(batch_id);
CREATE INDEX batches_session_first ON {schema}.batches(session_id, first_sequence);
CREATE TABLE {schema}.store_totals (
  id BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (id),
  session_count BIGINT NOT NULL
);
INSERT INTO {schema}.store_totals (id, session_count) VALUES (TRUE, 0);
CREATE TABLE {schema}.fa_schema_version (
  version INTEGER NOT NULL
);
"
    )
}

/// Ensure the store's schema exists and is migrated to [`SCHEMA_VERSION`].
///
/// Takes `pg_advisory_xact_lock(hashtext(schema))` inside the migration
/// transaction so concurrent processes never race the DDL. When the
/// `fa_schema_version` table is absent:
///
/// - [`SchemaPolicy::Manage`] applies the v1 DDL and writes version 1.
/// - [`SchemaPolicy::Require`] fails with
///   `StoreError::Unavailable{reason_code: "postgres_schema_missing"}`
///   without issuing any DDL.
///
/// Any stored version other than [`SCHEMA_VERSION`] fails closed with
/// `StoreError::Integrity{reason_code: "postgres_schema_unsupported"}`
/// regardless of policy — old builds must never read a newer schema
/// forward.
///
/// `client` must not be shared with another concurrent caller: the
/// transaction is driven with raw `BEGIN`/`COMMIT`/`ROLLBACK` over `&Client`
/// (rather than `tokio_postgres::Client::transaction()`, which requires
/// `&mut Client` and so cannot be handed to two concurrent callers at all)
/// so a caller can migrate over one checked-out pooled connection while
/// other connections migrate the same schema in parallel — the advisory
/// lock, not Rust's borrow checker, is what serializes those. Two
/// `ensure_schema` calls racing on the *same* `Client` would interleave
/// their `BEGIN`/`COMMIT` statements on one physical session and corrupt
/// each other's transaction; give each concurrent caller its own
/// connection, as the connection pool (a later task) does.
///
/// # Errors
///
/// Returns a mapped [`StoreError`] for any connection/protocol failure, or
/// the fail-closed errors described above.
pub(crate) async fn ensure_schema(
    client: &tokio_postgres::Client,
    schema: &str,
    policy: SchemaPolicy,
) -> Result<(), StoreError> {
    // `tokio_postgres::Client::transaction()` requires `&mut self`, which
    // would rule out serving several concurrent callers over one shared
    // `&Client` — exactly the case the concurrent-`ensure_schema` scenario
    // (spec D8) needs to exercise. Manage the transaction with raw
    // `BEGIN`/`COMMIT`/`ROLLBACK` instead; every statement below runs on
    // the same server-side session because `simple_query`/`execute`/
    // `query*` on a single `Client` all share its one underlying
    // connection.
    match ensure_schema_in_transaction(client, schema, policy).await {
        Ok(()) => {
            client
                .batch_execute("COMMIT")
                .await
                .map_err(|e| map_postgres_error(&e))?;
            Ok(())
        }
        Err(error) => {
            // Best-effort rollback: if the connection already died, the
            // server discarded the transaction anyway, so a rollback
            // failure here must not shadow the original error.
            let _ = client.batch_execute("ROLLBACK").await;
            Err(error)
        }
    }
}

/// The body of [`ensure_schema`], run inside a `BEGIN`/`COMMIT` pair that
/// the caller manages so every early-return path still rolls back cleanly.
async fn ensure_schema_in_transaction(
    client: &tokio_postgres::Client,
    schema: &str,
    policy: SchemaPolicy,
) -> Result<(), StoreError> {
    client
        .batch_execute("BEGIN")
        .await
        .map_err(|e| map_postgres_error(&e))?;

    client
        .execute(
            "SELECT pg_advisory_xact_lock(hashtext($1)::bigint)",
            &[&schema],
        )
        .await
        .map_err(|e| map_postgres_error(&e))?;

    let version_table_exists: bool = client
        .query_one(
            "SELECT EXISTS (SELECT 1 FROM information_schema.tables \
             WHERE table_schema = $1 AND table_name = 'fa_schema_version')",
            &[&schema],
        )
        .await
        .map_err(|e| map_postgres_error(&e))?
        .get(0);

    if !version_table_exists {
        return match policy {
            SchemaPolicy::Require => Err(StoreError::Unavailable {
                reason_code: "postgres_schema_missing",
            }),
            SchemaPolicy::Manage => {
                client
                    .execute(&format!("CREATE SCHEMA IF NOT EXISTS {schema}"), &[])
                    .await
                    .map_err(|e| map_postgres_error(&e))?;
                client
                    .batch_execute(&v1_ddl(schema))
                    .await
                    .map_err(|e| map_postgres_error(&e))?;
                client
                    .execute(
                        &format!("INSERT INTO {schema}.fa_schema_version (version) VALUES ($1)"),
                        &[&SCHEMA_VERSION],
                    )
                    .await
                    .map_err(|e| map_postgres_error(&e))?;
                Ok(())
            }
        };
    }

    let row = client
        .query_one(
            &format!("SELECT version FROM {schema}.fa_schema_version"),
            &[],
        )
        .await
        .map_err(|e| map_postgres_error(&e))?;
    let version: i32 = row.get(0);

    if version != SCHEMA_VERSION {
        return Err(StoreError::Integrity {
            reason_code: "postgres_schema_unsupported",
        });
    }

    Ok(())
}

/// Server-gated unit tests for `ensure_schema` (spec D3/D8). Skips with a
/// notice (rather than failing) when `FINSTACK_PG_TEST_URL` is unset,
/// matching the crate's other env-gated suites in `tests/`.
#[cfg(test)]
mod tests {
    use std::env;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    /// Read the server-gated test URL. `None` when unset.
    fn pg_test_url() -> Option<String> {
        env::var("FINSTACK_PG_TEST_URL").ok()
    }

    /// Connect to `url` and spawn its connection-driving task.
    ///
    /// # Panics
    ///
    /// Panics (via `expect`) if the connection cannot be established.
    /// Test-only code — the crate's `unwrap`/`expect` deny attributes are
    /// inner attributes scoped to non-test code (see `lib.rs`).
    async fn connect(url: &str) -> tokio_postgres::Client {
        let (client, connection) = tokio_postgres::connect(url, tokio_postgres::NoTls)
            .await
            .expect("connect to FINSTACK_PG_TEST_URL");
        tokio::spawn(async move {
            let _ = connection.await;
        });
        client
    }

    /// Generate a disposable schema name, unique within this process.
    fn fresh_schema_name() -> String {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or_default();
        format!("fa_test_{pid:x}_{nanos:x}_{counter:x}")
    }

    /// Best-effort disposable-schema cleanup. Unlike
    /// `tests/helpers::SchemaGuard`, this has no `Drop` warning — these are
    /// unit tests, not a shared, reusable test-support crate, so a leaked
    /// schema on assertion failure is an acceptable, low-ceremony trade-off
    /// local to this module.
    async fn drop_schema(client: &tokio_postgres::Client, schema: &str) {
        let _ = client
            .batch_execute(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"))
            .await;
    }

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

        drop_schema(&client, &schema).await;
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

        drop_schema(&client, &schema).await;
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

        drop_schema(&client, &schema).await;
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

        drop_schema(&client, &schema).await;
    }

    /// Two concurrent `ensure_schema` calls against the same schema, over two
    /// independent connections, both succeed: the advisory
    /// `pg_advisory_xact_lock` serializes the DDL rather than racing it.
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

        drop_schema(&cleanup_client, &schema).await;
    }
}
