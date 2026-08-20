//! Schema v1 DDL and advisory-locked, fail-closed migrations.
//!
//! Mirrors `extensions/stores/finstack-ai-store-sqlite/src/schema.rs` v1
//! column-for-column (`BLOB`→`BYTEA`, `INTEGER`→`BIGINT` except
//! `format_version`/`kind_version`, same PK/unique/index set) plus the
//! multi-writer bookkeeping columns/tables documented in spec D3/D8.
//!
//! Spec D3/D8 describe [`ensure_schema`] and [`SCHEMA_VERSION`] as
//! `pub(crate)`: production code only ever reaches them through the
//! connection pool / `JournalStore` impl added in a later task. That impl
//! does not exist yet, so this module is temporarily `pub` (see
//! `#[doc(hidden)] pub mod schema;` in `lib.rs`) purely so the integration
//! tests in `tests/schema.rs` — which must run as a separate crate — can
//! drive migrations directly against a real server. A later task should
//! narrow this back to `pub(crate)` once the pool wires it in and the
//! integration tests move to exercising it through the public store API.

use crate::config::SchemaPolicy;
use crate::error::map_postgres_error;
use finstack_ai_runtime::StoreError;

/// Schema version written to `fa_schema_version` once v1 DDL has been
/// applied. Any other stored version is treated as unsupported and fails
/// closed rather than being read forward.
pub const SCHEMA_VERSION: i32 = 1;

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
pub async fn ensure_schema(
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
