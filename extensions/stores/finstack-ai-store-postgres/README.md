# finstack-ai-store-postgres

Durable, multi-writer `PostgreSQL` [`JournalStore`](../../../crates/finstack-ai-runtime/src/ports/journal.rs)
implementation for finstack-ai.

Unlike sqlite's single ordered worker, operations run natively async on
tokio through a small hand-rolled connection pool; per-session write
serialization is delegated to Postgres row locks rather than client-side
ordering. Applications inject this crate explicitly — it is not the Agent,
Python, or WASM default (native-tokio only; no WASM support).

## Config

Construct a [`PostgresStoreConfig`](src/config.rs) with
`PostgresStoreConfig::new(url, limits)`, then adjust any of its fields before
calling `PostgresJournalStore::try_open`. Defaults:

| Field | Default | Notes |
| --- | --- | --- |
| `url` | (required) | `postgres://` connection string |
| `schema` | `"finstack_ai"` (`DEFAULT_SCHEMA`) | must match `[a-z_][a-z0-9_]{0,62}`; interpolated into DDL/DML, never parameterized |
| `durability` | `PostgresDurability::Durable` | see below |
| `limits` | (required) | `finstack_ai_runtime::StoreLimits`; every field must be non-zero |
| `pool_size` | `8` (`DEFAULT_POOL_SIZE`) | pooled connections; must be non-zero |
| `schema_policy` | `SchemaPolicy::Manage` | see below |
| `connect_timeout` | `5s` (`DEFAULT_CONNECT_TIMEOUT`) | applied to the initial connection and every pooled reconnect |

`PostgresStoreConfig::validate` rejects a zero `pool_size`
(`StoreError::InvalidRequest{reason_code: "zero_pool_size"}`), a zero
`StoreLimits` field (`reason_code: "zero_store_limit"`), or an invalid
`schema` (`reason_code: "invalid_schema_name"`).

## Durability modes

`PostgresDurability` governs the `SET synchronous_commit` applied to every
pooled connection, and what `health()` reports:

| Mode | `synchronous_commit` | `health().durable` | `health().detail` |
| --- | --- | --- | --- |
| `Durable` | `on` | `true` | `"postgres synchronous_commit=on"` |
| `Relaxed` | `off` | `false` | `"postgres synchronous_commit=off"` |

`Durable` is the only mode that may report `durable = true`; it still
depends on the server and underlying storage honoring
`synchronous_commit`. `health()` also does a `SELECT 1` round trip on a
pooled connection to set `ready`; a probe failure discards that connection
(spec D2) and reports `ready: false` rather than propagating an error —
`health()` is a status report, not a fallible operation.

## Schema policy

`SchemaPolicy` governs whether this store instance may issue schema DDL.
Migrations run inside `pg_advisory_xact_lock(hashtext(schema))` so
concurrent processes never race the DDL.

- **`Manage`** (default): when `<schema>.fa_schema_version` is absent,
  creates the schema and applies the v1 DDL (`sessions`, `batches`,
  `records`, `snapshots`, `store_totals`, `fa_schema_version`, plus
  indexes), then writes schema version 1.
- **`Require`**: never issues DDL. Fails closed with
  `StoreError::Unavailable{reason_code: "postgres_schema_missing"}` if the
  schema hasn't already been migrated by a `Manage` instance (or an
  operator running the DDL by hand). Use this for the least-privilege role
  a `Require` connection needs: grant `SELECT`, `INSERT`, `UPDATE`, `DELETE`
  on every table in the schema, and nothing else — no `CREATE`, no DDL
  privileges, no ownership of the schema.

Any stored version other than the current one fails closed regardless of
policy — `StoreError::Integrity{reason_code: "postgres_schema_unsupported"}`
— so an old build never reads a newer schema forward.

## TLS

This crate's v1 wiring is plaintext-only (`tokio_postgres::NoTls`). A
connection URL whose `sslmode` demands TLS (`require`, `verify-ca`,
`verify-full`) is rejected up front in `PostgresJournalStore::try_open` with
`StoreError::InvalidRequest{reason_code: "postgres_tls_unsupported"}` rather
than silently connecting in plaintext. `sslmode=disable`, `allow`,
`prefer`, or an absent `sslmode` all connect normally. Until
`tokio-postgres-rustls` wiring lands, put TLS at the network layer (a
private network, an SSH tunnel, or a TLS-terminating proxy/sidecar) in
front of a plaintext connection.

## Testing

The crate's server-gated unit and integration tests read the `PostgreSQL`
connection URL from the `FINSTACK_PG_TEST_URL` environment variable and
skip (print `"skipped: FINSTACK_PG_TEST_URL unset"`, don't fail) when it is
unset. Start a disposable local server with `mise run pg-test-db`, export
the URL it prints, then run the suite:

```bash
mise run pg-test-db
export FINSTACK_PG_TEST_URL=postgres://postgres:postgres@localhost:5432/postgres
cargo test -p finstack-ai-store-postgres
```

CI runs the same battery against a pinned `postgres:16` service container
in the `ci-rust` job.

Each test that needs schema state generates its own disposable schema name
(`fa_test_<pid>_<nanos>_<counter>`) rather than sharing one fixture schema,
so concurrent test runs (and concurrent `cargo nextest` workers) never
collide; tests best-effort `DROP SCHEMA ... CASCADE` their schema when
they're done.
