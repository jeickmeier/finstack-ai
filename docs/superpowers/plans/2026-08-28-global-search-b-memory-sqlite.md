# Plan B: memory sqlite semantic index (schema v3)

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development or superpowers:executing-plans, task-by-task. Steps use checkbox syntax.

**Goal:** `SqliteMemoryStore` implements the embedding index end to end:
schema v3 with migrations, transactional eviction beside the FTS
maintenance, the three index operations, and `MemoryQuery::Embedding`
search whose ranking is byte-identical to the in-process store's.

**Architecture:** Vectors are little-endian f32 BLOBs in a
`memory_embeddings` table keyed `(scope_digest, id, embedder_id)`. Pending
is a SQL anti-join; ranking is brute-force dot product over unit-normalized
vectors computed in Rust on the existing dedicated worker thread (≤ 4,096
rows — milliseconds; **no `sqlite-vec`, no new dependency**). All index
writes ride the same transactions as record mutations. Spec §6.1.

**Tech Stack:** Rust; existing `rusqlite`/`tokio` behind the `sqlite`
feature; `finstack-ai-embeddings` (already a dependency after Plan A).

**Spec:** `docs/superpowers/specs/2026-08-28-global-search-design.md`
**Depends on:** Plan A complete.

## Global Constraints

- Verify per task:
  `cargo nextest run -p finstack-ai-memory --features sqlite --locked` and
  `cargo clippy -p finstack-ai-memory --all-targets --features sqlite --locked -- -D warnings`;
  also build once without the feature. Never run workspace-wide tests.
- Schema changes only via the file-owned versioning in
  `src/store/sqlite/schema.rs`; `PRAGMA quick_check` after any migration.
- New stable error reasons regenerate
  `fixtures/compatibility/error-codes/v1/codes.json` via
  `mise run write-public-api` in the same task.
- One commit per task; short imperative subject; do not push.

---

### Task B1: schema v3 + migrations

**Files:**
- Modify: `src/store/sqlite/schema.rs`
- Test: `src/tests/sqlite.rs` (extend)

**Interfaces:** `SCHEMA_USER_VERSION: 2 → 3`. New DDL (in `V3_DDL` and in
the v2→v3 migration):

```sql
CREATE TABLE memory_embeddings (
  scope_digest TEXT NOT NULL, id TEXT NOT NULL, embedder_id TEXT NOT NULL,
  dimensions INTEGER NOT NULL, source_digest TEXT NOT NULL,
  vector BLOB NOT NULL, embedded_at INTEGER NOT NULL,
  PRIMARY KEY (scope_digest, id, embedder_id)
);
CREATE INDEX memory_embeddings_space ON memory_embeddings(embedder_id, scope_digest);
```

Version dispatch: `0 => create_v3`, `1 => migrate_v1 then migrate_v2_to_v3`,
`2 => migrate_v2_to_v3` (additive: create table + index, bump version),
`3 => Ok(())`, else unsupported.

- [ ] **Step 1:** Failing tests: a store created by the current code (v2)
  reopens under the new code with all records/FTS/keywords intact, an empty
  `memory_embeddings` table, `user_version = 3`, `quick_check = ok`; a
  fresh open is v3 directly; a future version still errors
  `memory_store_sqlite_schema_unsupported`.
- [ ] **Step 2:** Implement.
- [ ] **Step 3:** Green; clippy.
- [ ] **Step 4:** Commit `Add memory sqlite embeddings schema v3`

### Task B2: lifecycle eviction

**Files:**
- Modify: `src/store/sqlite/queries.rs` — `write_record` deletes the
  record's `memory_embeddings` rows in-transaction (beside its existing
  `memory_fts`/`memory_keywords` resets); add `delete_embedding_rows`
  sibling to `delete_fts_row` and call it at every `delete_fts_row` site
  (forget, correct/supersede); mirror the deletion in the expiry sweep.
- Test: `src/tests/sqlite.rs`

- [ ] **Step 1:** Failing tests (seed rows via direct `store_embedding`
  once B3 lands — for B2 seed via SQL through a test helper): put-over-
  tombstone, forget, correct, and expiry sweep each remove exactly the
  affected record's rows across all spaces and leave other records' rows
  intact.
- [ ] **Step 2:** Implement.
- [ ] **Step 3:** Green; clippy.
- [ ] **Step 4:** Commit `Evict sqlite memory embeddings with record lifecycle`

### Task B3: index operations (pending / store / drop space)

**Files:**
- Modify: `src/store/sqlite/worker.rs` (three new `Command` variants +
  `MemoryStore` method overrides), `src/store/sqlite/queries.rs`
- Test: `src/tests/sqlite.rs`

**Interfaces:**
- `pending_embedding_sources`: anti-join — live, unexpired,
  non-superseded records in any scope with no `memory_embeddings` row for
  `embedder_id`; text/digest built with the shared
  `embedding_source_text`/`embedding_source_digest`; stable order
  `(scope_digest, id)`; `limit` capped by `limits.max_search_results`.
- `store_embedding`: recompute the current row's source digest — mismatch
  is a silent `Ok` no-op; enforce per-space dims (first write fixes them)
  and `max_embedding_spaces` (`CapacityExceeded { resource: "embedding_spaces" }`);
  reject a vector for a missing/ineligible record (`NotFound`); write
  unit-normalized BLOB via `INSERT OR REPLACE`.
- `forget_embedding_space`: `DELETE ... WHERE embedder_id = ?1`.

- [ ] **Step 1:** Failing tests: anti-join correctness (embedded records
  drop out; a re-written record re-appears); digest guard no-ops after an
  interleaved correct; dims fixed per space; space cap; rows persist across
  close/reopen; `forget_embedding_space` empties only that space;
  `reconcile_memory_embeddings` with `HashEmbedder` drains a seeded store
  and is idempotent.
- [ ] **Step 2:** Implement.
- [ ] **Step 3:** Green; clippy; `mise run write-public-api` if any new
  stable reason string was introduced.
- [ ] **Step 4:** Commit `Implement sqlite memory embedding index operations`

### Task B4: semantic search + cross-store agreement

**Files:**
- Modify: `src/store/sqlite/queries.rs` — replace the Plan A stub: fetch
  live rows for `(scope_digest, embedder_id)` joined to `memory_records`
  (tombstone/supersession/expiry predicates), decode BLOBs (defensively
  skip a dims-mismatched row), rank by `similarity_score(dot)` descending
  with id tie-break, truncate to `limit`, evidence
  `MatchEvidence::Semantic`.
- Test: `src/tests/sqlite.rs`

- [ ] **Step 1:** Failing tests: ranked results with deterministic
  tie-break; scope and space isolation; unknown space returns empty (not an
  error); limit honored; tombstoned/superseded/expired excluded; **cross-
  store agreement** — identical corpus indexed via `HashEmbedder` in
  `InProcessMemoryStore` and `SqliteMemoryStore` returns identical ordered
  id lists for identical queries (mirrors
  `sqlite_and_in_process_full_text_normalization_match`).
- [ ] **Step 2:** Implement.
- [ ] **Step 3:** Green; clippy; no-feature build still clean.
- [ ] **Step 4:** Commit `Rank sqlite memory search by embedding similarity`
