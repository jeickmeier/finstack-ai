# Spec: PostgreSQL Journal Store (`finstack-ai-store-postgres`)

Status: draft for approval — there is no previously approved journal spec for
this backend. UC-06 (docs/planning/01-finstack-ai-product-requirements.md:194)
names "PostgreSQL or another store" for a server hosting thousands of idle
sessions and hundreds of active runs. Proposal 0001 explicitly deferred this
leaf ("Explicitly not proposed", docs/proposals/0001-ecosystem-and-capability-gaps.md:690);
this spec is the opt-in that un-defers it.

## Problem

`finstack-ai-store-memory` and `finstack-ai-store-sqlite` are the only
first-party `JournalStore` leaves. Both are single-process: memory is not
durable, and sqlite serializes all work through one worker thread on one local
file. Neither can host UC-06 — thousands of idle sessions, hundreds of active
runs, **multiple host processes writing to the same store**. A durable,
network-attached, multi-writer journal is required, with the same append,
batch-id idempotency, checksum-chain, and snapshot contract the existing
stores implement.

## Goal

A new extension crate `extensions/stores/finstack-ai-store-postgres`
implementing the full `JournalStore` port
(crates/finstack-ai-runtime/src/ports/journal.rs:55) — `append`, `load`,
`load_from`, `write_snapshot`, `write_state_snapshot`, `write_metadata`,
`scan`, `prune`, `health` — against PostgreSQL ≥ 14, passing the same
conformance battery as sqlite (`check_journal_store_conformance`,
`run_journal_v1_corpus`, crash-prefix restore classes), plus new multi-writer
tests sqlite cannot express.

## Non-goals

- No WASM support (native-tokio only; browser hosts keep IndexedDB).
- No generic SQL abstraction shared with sqlite; only the *semantic* layer is
  shared, via `finstack-ai-store-common`.
- No listen/notify change feeds, no read replicas, no partitioning/sharding.
  One logical database is the unit of deployment.
- No exactly-once claims (consistent with docs/site/durability.md).
- No connection-string secret management beyond "the application supplies it".

## Prerequisite: `finstack-ai-store-common`

The approved spec docs/superpowers/specs/2026-08-19-store-common-semantics.md
extracts append admission, batch commitment, snapshot semantics,
chain-verification, scan validation, and error mapping into
`finstack-ai-store-common` precisely because "a third backend (Turso/Postgres)
would mean a third reimplementation". That crate does not exist yet.
**Store-common lands first; this store consumes it.** This store must not
copy those functions a third time.

## Design decisions

### D1. Client library: `tokio-postgres` + rustls

Use `tokio-postgres` (0.7.x, MIT/Apache-2.0) with `tokio-postgres-rustls`
over the already-pinned workspace `rustls`/`tokio-rustls` (ring). Rejected:
`sqlx` (macro/compile-time machinery, larger tree), hand-rolled wire protocol
(SCRAM + TLS + extended query protocol is far past the sigv4-by-hand
precedent). `deny.toml` and `cargo deny` must stay clean; both crates are
MIT/Apache-2.0.

### D2. Concurrency model: small internal pool, no worker thread

Unlike sqlite's single ordered worker, operations run natively async on
tokio (`finstack-ai-runtime` `native-tokio` feature, same as the S3 store).
A hand-rolled bounded pool (`tokio::sync::Semaphore` + `Mutex<Vec<Client>>`,
lazy connect, discard-on-error, no reuse of a connection that returned a
protocol/IO error) avoids a `deadpool` dependency. Default pool size 8,
configurable. Per-session write serialization is delegated to Postgres row
locks (D4), not client-side ordering.

### D3. Schema v1 (mirrors sqlite v1, plus multi-writer bookkeeping)

Types: `BYTEA` for ids/digests/CBOR, `BIGINT` for sequences/timestamps
(u64→i64 guarded exactly like sqlite's `i64_from_u64`). Tables `sessions`,
`batches`, `records`, `snapshots` carry the same columns and unique
constraints as extensions/stores/finstack-ai-store-sqlite/src/schema.rs
(`records.record_id UNIQUE`, `batches UNIQUE (session_id, first_sequence)`,
`records PRIMARY KEY (session_id, sequence)`). Two deltas:

1. `sessions` gains denormalized `batch_count BIGINT` and `record_count
   BIGINT`, maintained in the append transaction, so `StoreLimits` checks
   never scan under concurrency.
2. A single-row `store_totals(session_count BIGINT)` table backs the
   `limits.sessions` check; it is locked only when creating a new session,
   so an unlocked `COUNT(*)` race cannot over-admit sessions.

All objects live in a configurable schema (namespace), default
`finstack_ai`, so one server hosts many isolated stores and tests get
disposable namespaces.

### D4. Append transaction protocol (multi-writer correctness)

`READ COMMITTED` transaction with explicit locks; admission logic is the
store-common pure functions so reason codes and check ordering
(batch replay → empty batch → record reuse → sequence conflict → limits,
sessions → batches → records) match memory/sqlite byte-for-byte:

1. `SELECT … FROM sessions WHERE session_id=$1 FOR UPDATE` — serializes all
   writers of one session; different sessions never contend.
2. Missing row: lock `store_totals FOR UPDATE`, enforce `limits.sessions`,
   `INSERT` the session row (`ON CONFLICT DO NOTHING` + re-`SELECT … FOR
   UPDATE` closes the create race; losing creator proceeds as step 1).
3. Under the lock: fetch existing batch by `batch_id` (idempotent replay
   compares stored `request_cbor` — same byte-stable `AppendIdentity` CBOR
   as sqlite), fetch record-id reuse, run store-common admission, then
   insert batch + records (`UNNEST` batch insert, one round trip) and
   CAS-update the session head (`WHERE current_sequence = $prev` kept as
   defense-in-depth; failure is `Integrity{sequence_cas_failed}`).
4. `COMMIT`.

### D5. Ambiguous acknowledgement is a first-class outcome

If the connection dies after `COMMIT` is sent but before the reply,
the append maps to `StoreError::AmbiguousAcknowledgement` (the variant
exists in the port for exactly this). The connection is discarded, never
reused. Recovery is the existing idempotency contract: retrying the same
`AppendRequest` either replays the committed batch (equal `request_cbor` →
same `CommittedBatch`) or commits it fresh. Every other transport/server
error maps per a `map_postgres_error` table analogous to sqlite's:
connection/IO/serialization → `Unavailable` (reason codes
`postgres_unavailable`, `postgres_serialization`), disk-full class (53100)
→ `Unavailable{postgres_disk_full}`, integrity-violation surprises →
`Integrity{postgres_constraint}`.

### D6. Durability and `health()`

`PostgresDurability::Durable` (default): every pooled connection runs
`SET synchronous_commit = on`; `health()` reports `durable: true`, detail
`"postgres synchronous_commit=on"`. `PostgresDurability::Relaxed`: `SET
synchronous_commit = off`, `durable: false`, detail says so — mirroring
sqlite's explicit relaxed labeling. `health()` also round-trips `SELECT 1`
for `ready` and reports `ready: false` (not an error) when the pool cannot
connect.

### D7. Crash-prefix

Server crash: Postgres WAL guarantees committed-transaction atomicity, so an
acknowledged batch survives and an unacknowledged one is absent — the batch
is the atomic unit, same as sqlite. Client-process crash mid-append: the
transaction rolls back server-side. The property is *proved*, not assumed,
by tests: (a) a loopback TCP proxy (precedent:
extensions/stores/finstack-ai-store-object-s3/tests/loopback.rs) that severs
the stream at commit time — assert `AmbiguousAcknowledgement`, then assert
retry-with-same-batch-id lands exactly one copy whichever side of the sever
the commit fell; (b) restart-replay through the coordinator recovery path
mirroring finstack-ai-test/tests/crash_prefix/sqlite.rs restore classes.

### D8. Migrations

`fa_schema_version(version INTEGER NOT NULL)` single row inside the store's
schema. On open: take `pg_advisory_xact_lock` (key derived from the schema
name) so concurrent processes never race DDL; empty → apply v1 DDL and write
version 1; version 1 → proceed; anything else → fail closed
`Integrity{postgres_schema_unsupported}` (the sqlite `user_version` rule).
Config `schema_policy: Manage | Require` — `Require` never issues DDL and
fails with `Unavailable{postgres_schema_missing}` for least-privilege roles.
Future versions must be applied by a newer store build under the same
advisory lock; old builds fail closed rather than read forward.

### D9. Checksums and chain verification

Load performs full-chain verification through store-common (the memory
`verify_session` / sqlite `verify_stored_session` semantics): every envelope's
`previous_checksum`/`envelope_checksum` links and the stored head matches.
A process-local `VerifiedHead` cache (per store instance, keyed by session)
lets subsequent loads verify only the suffix — valid under multi-writer
because the chain is append-only. The cache entry is dropped whenever (a) a
load observes a head below the cached sequence (external prune/reset ⇒ full
re-verify), or (b) any `Integrity` error is returned for that session.
`load_from` windows (`FromSequence`, `SnapshotPlusTail`) use the store-common
tail verification with the unified gap/split reason codes.

### D10. Testing without a bundled server

Integration tests require a real server: gated on `FINSTACK_PG_TEST_URL`
(skip-with-notice when unset, like other env-gated suites). Each test creates
a disposable schema `fa_test_<uuid>` and drops it on success. CI: a
`postgres:16` service container on the `ci-rust` job exporting
`FINSTACK_PG_TEST_URL=postgres://postgres:postgres@localhost:5432/postgres`.
Local: `mise run pg-test-db` starts one via docker. Everything not needing a
server (config validation, error mapping, SQL text, identity CBOR) is a plain
unit test.

## Behavioral invariants

- Byte-stable idempotency identity: `AppendIdentity` CBOR identical to
  sqlite's (same struct, same serde derives — it lives in store-common).
- Error reason codes and check ordering identical to memory/sqlite as pinned
  by store-common unit tests and the shared conformance battery.
- `health().durable == true` only under `Durable` with server
  `synchronous_commit` honored (NFR-REL-001 posture).
- No `unsafe`, no `unwrap`/`expect`/`panic` in non-test code (workspace lints).
- Public API baseline recorded at
  `fixtures/compatibility/public-rust-api/cargo-public-api/finstack-ai-store-postgres.txt`.

## Acceptance

- `cargo test -p finstack-ai-store-postgres` green with and without
  `FINSTACK_PG_TEST_URL` (without: server-gated tests skip, unit tests run).
- `check_journal_store_conformance` and the journal-v1 40-body corpus pass
  against the postgres store.
- Multi-writer suite passes: two pools, same session — interleaved appends
  yield one linear chain; concurrent same-batch appends yield one committed
  batch and one idempotent replay or clean `Conflict`; severed-commit test
  proves `AmbiguousAcknowledgement` + idempotent retry.
- `cargo clippy --workspace --all-targets` and `cargo deny check` clean;
  `mise run check-public-api` green with the new baseline.
- docs/site/durability.md store table lists the leaf; CHANGELOG entry added.
