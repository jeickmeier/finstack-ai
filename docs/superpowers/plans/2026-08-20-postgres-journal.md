# PostgreSQL Journal Store Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship `finstack-ai-store-postgres`, a durable multi-writer `JournalStore` leaf for UC-06-scale hosting, passing the same conformance battery as sqlite plus new multi-writer/ambiguity tests.

**Architecture:** A new extension crate mirrors the sqlite store's shape (config / schema / error / append / load / store modules) but runs natively async over a small hand-rolled `tokio-postgres` connection pool instead of a worker thread. All append-admission, batch-commitment, snapshot, chain-verification, and scan semantics come from `finstack-ai-store-common` (prerequisite crate; spec already approved). Multi-writer correctness comes from per-session `FOR UPDATE` row locks plus a defense-in-depth head CAS; a severed `COMMIT` maps to `StoreError::AmbiguousAcknowledgement` and is healed by the batch-id idempotency contract. Schema v1 is applied/verified under a Postgres advisory lock with fail-closed versioning.

**Tech Stack:** Rust workspace; `tokio-postgres` 0.7 + `tokio-postgres-rustls` (new workspace deps); `finstack-ai-runtime` with `native-tokio`; `finstack-ai-store-common`; `finstack-ai-test` conformance helpers; Postgres ≥ 14 (CI: `postgres:16` service).

**Spec:** docs/superpowers/specs/2026-08-20-postgres-journal-design.md (decisions D1–D10). Prerequisite spec: docs/superpowers/specs/2026-08-19-store-common-semantics.md. Reference implementation for every semantic detail: `extensions/stores/finstack-ai-store-sqlite/` (verified 2026-08-20 on `main` @ 306b776 — re-verify cited lines before editing; they drift).

## Global Constraints

- **Prerequisite:** `finstack-ai-store-common` must exist and both existing stores must be rewired through it (its own spec/plan) before Task 3 of this plan. Tasks 1–2 may proceed in parallel with that work.
- Workspace lints (Cargo.toml:199-205 + crate headers): no `unsafe`, no `unwrap`/`expect`/`panic` in non-test code, `missing_docs` warn ⇒ document every public item. Fallible constructors.
- Error reason codes are stable strings; check ordering is observable behavior. Never invent a code where sqlite/store-common already defines one; new codes introduced by this plan: `postgres_unavailable`, `postgres_serialization`, `postgres_disk_full`, `postgres_constraint`, `postgres_schema_unsupported`, `postgres_schema_missing`, `zero_pool_size`, `sequence_cas_failed` (reused), `postgres_i64_overflow`.
- u64→i64 conversions guarded via helpers modeled on `extensions/stores/finstack-ai-store-sqlite/src/error.rs:4-18`.
- Gating commands use workspace binaries with explicit exit codes (`cargo test -p <crate>`), never a summarizing wrapper (RTK guidance).
- Server-gated tests: skip with an explicit `eprintln!("skipped: FINSTACK_PG_TEST_URL unset")` + `return` when the env var is absent; never fail, never fake success.
- New public crate ⇒ new baseline `fixtures/compatibility/public-rust-api/cargo-public-api/finstack-ai-store-postgres.txt` via `scripts/compat/public_items.py`, gated by `mise run check-public-api`.
- Commit after every green task; message prefix `feat(store-postgres):`.
- `publish = false` in the crate manifest (matches sqlite).

## Current-state map (read before starting)

| Fact | Where |
| --- | --- |
| `JournalStore` port: 9 methods, default impls for `scan`/`write_metadata`/`write_state_snapshot`/`prune`/`load_from` | `crates/finstack-ai-runtime/src/ports/journal.rs:55-154` |
| `StoreError` variants incl. `AmbiguousAcknowledgement`; stable `code()` strings | `ports/journal.rs:585-648` |
| `StoreLimits` + zero-limit validation | `ports/journal.rs:19-48` |
| sqlite v1 DDL (column-for-column model for PG v1) | `extensions/stores/finstack-ai-store-sqlite/src/schema.rs:11-56` |
| Append admission ordering + `AppendIdentity` CBOR idempotency | `.../src/append.rs:13-126` |
| Error mapping conventions | `.../src/error.rs` |
| Sqlite trait wiring (method → ctx op) | `.../src/journal_store.rs` |
| Conformance helper for one store: `check_journal_store_conformance` (atomicity, equal-retry, fault injection) | `crates/finstack-ai-test/tests/port_conformance.rs:238-277` |
| Journal-v1 40-body corpus runner | `crates/finstack-ai-test/tests/journal_v1.rs` |
| Crash-prefix restore classes + sqlite battery | `crates/finstack-ai-test/src/crash_prefix.rs`, `tests/crash_prefix/` |
| Loopback fault-injection precedent (hand-rolled TCP) | `extensions/stores/finstack-ai-store-object-s3/tests/loopback.rs` |
| Async-native extension precedent (`native-tokio`, tokio dep) | `extensions/stores/finstack-ai-store-object-s3/Cargo.toml` |
| CI has no service containers today; jobs run `mise run ci-rust` etc. | `.github/workflows/ci.yml` |
| Store table to update | `docs/site/durability.md` |
| Python binding exposes sqlite store (parity target) | `bindings/finstack-ai-python/src/lib.rs:65,123` |

---

### Task 1: Crate scaffold, workspace wiring, config

**Files:**
- Create: `extensions/stores/finstack-ai-store-postgres/Cargo.toml`
- Create: `extensions/stores/finstack-ai-store-postgres/src/lib.rs`
- Create: `extensions/stores/finstack-ai-store-postgres/src/config.rs`
- Create: `extensions/stores/finstack-ai-store-postgres/src/error.rs`
- Modify: `Cargo.toml` (workspace members + `[workspace.dependencies]`: `tokio-postgres = { version = "0.7", default-features = false, features = ["runtime"] }`, `tokio-postgres-rustls = "0.13"`, `finstack-ai-store-postgres = { path = ..., version = "1.0.0" }`)

**Interfaces:**
- Produces: `PostgresStoreConfig { url: Arc<str>, schema: Arc<str>, durability: PostgresDurability, limits: StoreLimits, pool_size: usize, schema_policy: SchemaPolicy, connect_timeout: Duration }` with `PostgresStoreConfig::new(url, limits) -> Self` (defaults: schema `finstack_ai`, `Durable`, pool 8, `Manage`, 5s) and `validate(&self) -> Result<(), StoreError>`; `enum PostgresDurability { Durable, Relaxed }`; `enum SchemaPolicy { Manage, Require }`; `pub(crate) fn i64_from_u64 / u64_from_i64`; `pub(crate) fn map_postgres_error(&tokio_postgres::Error) -> StoreError`.

- [ ] **Step 1: Write failing unit tests** in `config.rs` `#[cfg(test)]`: zero `pool_size` → `InvalidRequest{zero_pool_size}`; zero limit → `InvalidRequest{zero_store_limit}` (delegates to `StoreLimits::validate`); schema name rejected unless `[a-z_][a-z0-9_]{0,62}` → `InvalidRequest{invalid_schema_name}` (schema is interpolated into DDL — it must be validated, never parameterized). In `error.rs` tests: `map_postgres_error` on a synthesized closed-connection error → `Unavailable{postgres_unavailable}`; SQLSTATE `53100` → `postgres_disk_full`; `40001` → `postgres_serialization`; `23xxx` → `Integrity{postgres_constraint}`.
- [ ] **Step 2: Run** `cargo test -p finstack-ai-store-postgres` — FAIL (crate/types missing).
- [ ] **Step 3: Implement** manifest (deps: kernel, protocol, runtime `native-tokio`, store-common, tokio-postgres, tokio-postgres-rustls, rustls, serde, thiserror; dev-deps: finstack-ai-test, tokio macros/rt, serde_json), `lib.rs` module skeleton with crate docs, config + error modules.
- [ ] **Step 4: Run** the tests — PASS. Also `cargo deny check` and `cargo clippy -p finstack-ai-store-postgres --all-targets` — clean.
- [ ] **Step 5: Commit** `feat(store-postgres): scaffold crate with config and error mapping`

### Task 2: Test harness (env-gated server, disposable schemas) + schema v1 + migrations

**Files:**
- Create: `extensions/stores/finstack-ai-store-postgres/src/schema.rs`
- Create: `extensions/stores/finstack-ai-store-postgres/tests/helpers/mod.rs`
- Create: `extensions/stores/finstack-ai-store-postgres/tests/schema.rs`

**Interfaces:**
- Produces: `pub(crate) const SCHEMA_VERSION: i32 = 1`; `pub(crate) async fn ensure_schema(client: &tokio_postgres::Client, schema: &str, policy: SchemaPolicy) -> Result<(), StoreError>`; test helper `pg_test_url() -> Option<String>` and `async fn disposable_store(limits: StoreLimits) -> Option<(PostgresJournalStore, SchemaGuard)>` (creates `fa_test_<uuid>` schema; `SchemaGuard` drops it).
- DDL: sqlite v1 tables column-for-column (`schema.rs:11-56`) with `BLOB`→`BYTEA`, `INTEGER`→`BIGINT` (format/kind versions `INTEGER`), same PK/unique/index set, plus `sessions.batch_count BIGINT NOT NULL DEFAULT 0`, `sessions.record_count BIGINT NOT NULL DEFAULT 0`, `store_totals(id BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (id), session_count BIGINT NOT NULL)` seeded with one row, and `fa_schema_version(version INTEGER NOT NULL)`.

- [ ] **Step 1: Write failing tests** (server-gated): fresh schema → `ensure_schema` creates all tables and writes version 1; second call is a no-op; version row hand-set to 2 → `Integrity{postgres_schema_unsupported}`; `SchemaPolicy::Require` on empty schema → `Unavailable{postgres_schema_missing}`; two concurrent `ensure_schema` calls (two connections, `tokio::join!`) both succeed (advisory `pg_advisory_xact_lock(hashtext($schema)::bigint)` serializes DDL).
- [ ] **Step 2: Run** — FAIL.
- [ ] **Step 3: Implement** `ensure_schema`: begin tx → advisory xact lock → read `fa_schema_version` (missing table + `Manage` ⇒ apply DDL with the validated schema name interpolated, insert version; `Require` ⇒ fail) → match version → commit.
- [ ] **Step 4: Run with a local server** (`mise run pg-test-db` from Task 9 or any Postgres; export `FINSTACK_PG_TEST_URL`) — PASS. Run without the env var — tests skip, exit 0.
- [ ] **Step 5: Commit** `feat(store-postgres): schema v1 with advisory-locked fail-closed migrations`

### Task 3: Pool, store handle, `health()`

**Files:**
- Create: `extensions/stores/finstack-ai-store-postgres/src/pool.rs`
- Create: `extensions/stores/finstack-ai-store-postgres/src/store.rs`
- Create: `extensions/stores/finstack-ai-store-postgres/src/journal_store.rs` (trait impl, grows over Tasks 4–7; unimplemented methods keep port defaults until their task)
- Test: `extensions/stores/finstack-ai-store-postgres/tests/health.rs`

**Interfaces:**
- Consumes: Task 1 config/error, Task 2 `ensure_schema`.
- Produces: `pub struct PostgresJournalStore` with `pub async fn try_open(config: PostgresStoreConfig) -> Result<Self, StoreError>` (validates config, connects one client, runs `ensure_schema`, seeds pool); `pub(crate) struct Pool` with `async fn get(&self) -> Result<PooledClient, StoreError>` (semaphore-bounded, lazy connect, TLS via rustls when the URL demands it) and `PooledClient::discard(self)` for poisoned connections; every checkout has run `SET synchronous_commit = on|off` and `SET search_path = <schema>` at connect time. `impl JournalStore` provides `health()`.

- [ ] **Step 1: Write failing tests**: `try_open` against the disposable schema succeeds; `health()` → `ready: true`, `durable: true`, detail `"postgres synchronous_commit=on"`; `Relaxed` config → `durable: false`; unroutable URL (`postgres://127.0.0.1:1@/x`, short `connect_timeout`) → `try_open` errs `Unavailable{postgres_unavailable}`; unit test (no server): pool size honored — 2 permits, third `get` waits until a client is returned.
- [ ] **Step 2: Run** — FAIL.
- [ ] **Step 3: Implement** pool + store + `health()` (`SELECT 1` probe; probe failure ⇒ `ready: false`, not `Err`).
- [ ] **Step 4: Run** — PASS (gated tests against server; pool unit test always).
- [ ] **Step 5: Commit** `feat(store-postgres): connection pool, open path, health`

### Task 4: Append

**Files:**
- Create: `extensions/stores/finstack-ai-store-postgres/src/append.rs`
- Modify: `src/journal_store.rs` (wire `append`)
- Test: `extensions/stores/finstack-ai-store-postgres/tests/append.rs`

**Interfaces:**
- Consumes: store-common admission (`AppendIdentity`, replay/reuse/conflict/limit classification, `build_committed_batch`), Task 3 pool.
- Produces: `pub(crate) async fn append(client: &mut PooledClient, request: &AppendRequest, limits: &StoreLimits) -> Result<CommittedBatch, StoreError>` implementing spec D4 exactly: tx → `SELECT current_sequence, head_checksum, batch_count, record_count FROM sessions WHERE session_id=$1 FOR UPDATE` → (missing: lock `store_totals FOR UPDATE`, sessions-limit, `INSERT … ON CONFLICT (session_id) DO NOTHING`, re-select `FOR UPDATE`) → batch-replay lookup by `batch_id` comparing `request_cbor` → record-id reuse lookup (`SELECT record_id, batch_id FROM records WHERE record_id = ANY($1)`) → store-common admission → insert batch row, `UNNEST` records insert, `UPDATE sessions SET current_sequence=$new, head_checksum=$h, batch_count=batch_count+1, record_count=record_count+$n WHERE session_id=$s AND current_sequence=$prev` (0 rows ⇒ `Integrity{sequence_cas_failed}`), bump `store_totals` only on create → commit. A commit whose error is connection-loss (not a server-reported failure) ⇒ `AmbiguousAcknowledgement` + `discard`.

- [ ] **Step 1: Write failing tests** (all server-gated, one session unless noted): happy path returns `CommittedBatch` with chained checksums; identical retry returns the byte-equal batch; same `batch_id` different draft → `Corruption{append_batch_id_reuse}`; empty batch → `InvalidRequest{empty_append_batch}`; wrong `expected_sequence` → `Conflict` with correct `actual_next_sequence`; limits: `sessions`, `batches_per_session`, `records_per_session` each trip in order (assert `batch_and_record_idempotency_precede_sequence_checks` ordering exactly as the sqlite test of that name); **multi-writer:** two stores (two pools, same schema), 16 interleaved appends each with per-attempt conflict-retry → final journal is one linear chain of 32 batches, every checksum links; two stores submit the *same* `AppendRequest` concurrently → both get equal `CommittedBatch`.
- [ ] **Step 2: Run** — FAIL.
- [ ] **Step 3: Implement** `append.rs` per the interface block.
- [ ] **Step 4: Run** — PASS.
- [ ] **Step 5: Commit** `feat(store-postgres): multi-writer append with idempotent replay`

### Task 5: Load, chain verification, `load_from`

**Files:**
- Create: `extensions/stores/finstack-ai-store-postgres/src/load.rs`
- Modify: `src/journal_store.rs` (wire `load`, `load_from`), `src/store.rs` (add `verified: Mutex<HashMap<SessionId, VerifiedHead>>`)
- Test: `extensions/stores/finstack-ai-store-postgres/tests/load.rs`

**Interfaces:**
- Consumes: store-common chain/tail verification and `accelerated_from`; Task 4 append (test setup).
- Produces: `load` returning `LoadedSession` with batches rebuilt on stored batch boundaries, snapshot + accelerated restore populated; full-chain verification on first load, suffix-only via the `VerifiedHead` cache thereafter; cache dropped when observed head < cached sequence or on any `Integrity` result (spec D9). `load_from` implements `FromSequence`/`SnapshotPlusTail` natively (SQL from-sequence fetch) with store-common tail verification and the unified gap/split codes.

- [ ] **Step 1: Write failing tests**: load of unknown session → `LoadedSession::empty`; append 3 batches → load returns 3 batches, correct head; corrupt one `envelope_checksum` via raw SQL → load → `Integrity` (store-common chain code) and a subsequent fixed load re-verifies fully (cache dropped); `FromSequence` at a batch boundary with correct prior checksum → suffix only; wrong prior checksum → `Integrity{load_from_prior_checksum_mismatch}`; mid-batch start → `Integrity{load_from_splits_batch}`; missing start → gap code; `SnapshotPlusTail` without snapshot → full load; second-store-instance append then first-store load → new tail verified (cache is suffix-safe under multi-writer).
- [ ] **Step 2: Run** — FAIL.
- [ ] **Step 3: Implement** `load.rs`.
- [ ] **Step 4: Run** — PASS.
- [ ] **Step 5: Commit** `feat(store-postgres): verified load with suffix cache and load windows`

### Task 6: Snapshots, scan, metadata

**Files:**
- Create: `extensions/stores/finstack-ai-store-postgres/src/snapshot.rs`
- Modify: `src/journal_store.rs` (wire `write_snapshot`, `write_state_snapshot`, `scan`, `write_metadata`)
- Test: `extensions/stores/finstack-ai-store-postgres/tests/snapshot_scan_metadata.rs`

**Interfaces:**
- Consumes: store-common `encode_state_request`, snapshot admission (`snapshot_bytes` limit, `snapshot_ahead_of_journal`, `snapshot_sequence_regression`), scan validation (`scan_limit_zero`, `SCAN_PAGE_MAX_RECORDS` clamp, `next_sequence`).
- Produces: snapshot upsert (`INSERT … ON CONFLICT (session_id) DO UPDATE` guarded by regression check inside a tx holding the session row lock); `scan` as a plain indexed range read; `write_metadata` as CAS on `head_checksum` inside the row lock (`Conflict`-style mismatch uses the store-common reason code).

- [ ] **Step 1: Write failing tests**: state snapshot round-trips through `load` as `accelerated` with matching `head_checksum`; oversized snapshot → `LimitExceeded{snapshot_bytes}`; snapshot ahead of head → `snapshot_ahead_of_journal`; sequence regression → `snapshot_sequence_regression`; scan pages of 2 across 5 records walk `next_sequence` to `None`; `limit: 0` → `scan_limit_zero`; metadata CAS succeeds with matching head, fails cleanly with stale head.
- [ ] **Step 2: Run** — FAIL. **Step 3: Implement.** **Step 4: Run** — PASS.
- [ ] **Step 5: Commit** `feat(store-postgres): snapshots, scan, metadata CAS`

### Task 7: Prune

**Files:**
- Create: `extensions/stores/finstack-ai-store-postgres/src/prune.rs`; Modify: `src/journal_store.rs`
- Test: `extensions/stores/finstack-ai-store-postgres/tests/prune.rs`

**Interfaces:**
- Consumes: store-common prune admission (`prune_snapshot_not_aligned`) and receipt counting (`outstanding_count`, `tombstone_count`).
- Produces: in one tx under the session row lock: verify snapshot exists and aligns on a batch boundary, delete records/batches with `last_sequence <= snapshot_sequence`, decrement `sessions.batch_count`/`record_count`, return `PruneReceipt`. The `VerifiedHead` cache entry for the session is retained (prune never touches the tail) but a full reload after prune must verify snapshot-plus-tail.

- [ ] **Step 1: Write failing tests**: prune without snapshot → `InvalidRequest` (store-common code); aligned prune deletes prefix, `load` afterwards returns snapshot + tail and verifies; receipt counts match sqlite's for the same scripted history (steal the scenario from the sqlite prune test in `extensions/stores/finstack-ai-store-sqlite/src/tests.rs`); concurrent prune + append on one session serialize without deadlock (lock ordering: session row first, always).
- [ ] **Step 2: Run** — FAIL. **Step 3: Implement.** **Step 4: Run** — PASS.
- [ ] **Step 5: Commit** `feat(store-postgres): snapshot-aligned prefix prune`

### Task 8: Conformance battery + crash/ambiguity proofs

**Files:**
- Create: `crates/finstack-ai-test/tests/postgres_journal.rs` (env-gated)
- Create: `extensions/stores/finstack-ai-store-postgres/tests/severed_commit.rs`
- Modify: `crates/finstack-ai-test/Cargo.toml` (dev-dep on the new store)

**Interfaces:**
- Consumes: `check_journal_store_conformance` + `JournalStoreConformanceCase` (`port_conformance.rs:238-277` pattern), journal-v1 corpus bodies (`all_activated_record_bodies`, `draft_for_body`), crash-prefix helpers (`accept_run`/`drive_to_model_request`/`recover`/`assert_legal` from `tests/crash_prefix/helpers/`), loopback proxy pattern from the S3 store.
- Produces: (a) port-conformance run over `PostgresJournalStore`; (b) all 40 journal-v1 record bodies appended and loaded back byte-equal; (c) restart-replay: drive a run to a model request through the coordinator, drop it, `recover` → `LegalRestore::Retryable`, then prune → recover again (mirror of `sqlite_v1_opens_prunes_and_process_kill_stays_separate`); (d) severed-commit: a minimal in-test TCP proxy between store and server that kills the stream on the first client packet after the proxy sees the `COMMIT` bytes for the marked transaction — append returns `AmbiguousAcknowledgement`; a fresh store retries the identical request and the final journal holds exactly one copy of the batch, whichever side of the sever the commit landed.

- [ ] **Step 1: Write the four failing tests** (all skip without `FINSTACK_PG_TEST_URL`).
- [ ] **Step 2: Run** — FAIL (wiring absent).
- [ ] **Step 3: Implement** wiring and the proxy helper (proxy lives in the test file; byte-sniffing only, no protocol parsing beyond finding the tagged commit).
- [ ] **Step 4: Run** `cargo test -p finstack-ai-store-postgres -p finstack-ai-test --test postgres_journal` — PASS; run once more with the proxy sever point shifted one packet earlier (config knob in the helper) to cover both "committed" and "rolled back" ambiguity outcomes.
- [ ] **Step 5: Commit** `test(store-postgres): conformance battery, crash-prefix, severed-commit ambiguity`

### Task 9: CI service, mise task, docs, baselines

**Files:**
- Modify: `.github/workflows/ci.yml` (add to `ci-rust`: `services: postgres: image: postgres@<pinned digest for 16> env: POSTGRES_PASSWORD: postgres ports: ["5432:5432"] options: --health-cmd pg_isready …`; export `FINSTACK_PG_TEST_URL` in the Rust CI step)
- Modify: `mise.toml` (task `pg-test-db`: `docker run --rm -d -p 5432:5432 -e POSTGRES_PASSWORD=postgres --name finstack-pg postgres:16` + printed export line)
- Modify: `docs/site/durability.md` (store table row: `finstack-ai-store-postgres` — "Multi-process durable journals (synchronous_commit=on)"; note Relaxed mode), `CHANGELOG.md`
- Create: `extensions/stores/finstack-ai-store-postgres/README.md` (config, durability modes, schema policy, least-privilege `Require` grants, test env var)
- Create: `fixtures/compatibility/public-rust-api/cargo-public-api/finstack-ai-store-postgres.txt` via `scripts/compat/public_items.py`

- [ ] **Step 1:** Make the edits; pin the postgres image by digest (repo convention: all actions are SHA-pinned).
- [ ] **Step 2: Run** `mise run ci-rust` locally with `pg-test-db` up — green; `mise run check-public-api` — green.
- [ ] **Step 3:** Push a draft PR to observe the service container actually serving the gated tests (check the test count is nonzero in CI logs — the skip path must not silently swallow the suite).
- [ ] **Step 4: Commit** `feat(store-postgres): CI postgres service, docs, public-api baseline`

### Task 10 (optional, separate PR): Python binding parity

**Files:**
- Modify: `bindings/finstack-ai-python/Cargo.toml`, `src/lib.rs`, `src/store.rs` (`PyPostgresDurability`, `PyPostgresStoreConfig`, store class mirroring the sqlite exposure at `lib.rs:65,123`), `python/finstack_ai/_finstack_ai.pyi`, `docs/`

- [ ] Mirror the sqlite binding surface; async open bridged via `pyo3-async-runtimes`; server-gated Python test; regenerate any binding baselines. TDD steps as in Tasks 1–7.

## Self-review notes

- Spec coverage: D1→T1, D2→T3, D3→T2, D4→T4, D5→T4+T8, D6→T3, D7→T8, D8→T2, D9→T5, D10→T2/T8/T9. Acceptance bullets map to T8 (battery, multi-writer), T9 (clippy/deny/baseline/docs).
- Deliberate deviations from sqlite: denormalized per-session counters and `store_totals` (concurrency, D3); no worker thread (D2); native `load_from` instead of trim-default (D9). Everything else matches sqlite behavior byte-for-byte through store-common.
- Known risk: `tokio-postgres-rustls` version drift against workspace `rustls 0.23` — resolve at Task 1 with `cargo deny`/`cargo tree`; if incompatible, fall back to `tokio-postgres` TLS feature-off with `NoTls` for v1 and document TLS as the application's tunnel concern (flag this to the user at Task 1, do not silently decide).
