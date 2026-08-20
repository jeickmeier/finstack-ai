# Memory Extension (`finstack-ai-memory`) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the recall-only `finstack-ai-context-memory` crate with one cohesive `finstack-ai-memory` extension implementing the full memory composition: store trait (+ in-process and SQLite/FTS5 impls), recall provider, capability-gated toolset, observer capture, plus Python and WASM exposure.

**Architecture:** One crate at `extensions/memory/finstack-ai-memory` built around an object-safe `MemoryStore` trait whose every mutation takes an idempotency key. Recall (`MemoryContextProvider`), tools (`MemoryToolset`), and capture (`MemoryObserver` + `MemoryExtractor`) are thin components over the store. No kernel/runtime changes. Capture middleware is explicitly deferred (runtime middleware must be pure).

**Tech Stack:** Rust (workspace conventions), rusqlite + FTS5 (feature `sqlite`), pyo3 (existing `bindings/finstack-ai-python`), wasm-bindgen host-callback pattern (existing `bindings/finstack-ai-wasm`).

**Spec:** `docs/superpowers/specs/2026-08-19-memory-extension-design.md`

## Global Constraints

- Crate lint header identical to existing extensions (copy from `extensions/toolsets/finstack-ai-tools-calculator/src/lib.rs:1-22`): `#![warn(missing_docs)]`, `#![forbid(unsafe_code)]`, deny `unwrap_used`/`expect_used`/`panic`/`unreachable`, test-cfg allowances, doc-test `expect` allowance.
- All error enums use `thiserror` with stable, non-secret snake_case reason codes (e.g. `memory_configuration_invalid`).
- No ambient time in library code paths that must be deterministic: `Timestamp` (`finstack_ai_kernel::Timestamp`, epoch-ms, `from_unix_ms`/`as_unix_ms`) is produced by an injected `MemoryClock` closure.
- No floats in the record model (confidence is `u8` 0..=100).
- Workspace: every new crate is added to root `Cargo.toml` `[workspace.members]` AND `[workspace.dependencies]` (path + version `1.0.0`, matching lines like `Cargo.toml:112-113`), `publish = false`, `[lints] workspace = true`.
- Async style: port methods return `PortFuture<Result<...>>` (`Box::pin(async move { ... })`); trait objects are `Send + Sync`.
- Run tests with `cargo test -p <crate>` and check exit codes explicitly; run `cargo clippy -p <crate> --all-targets --all-features` before each commit (workspace denies warnings via lints).
- Commit messages: conventional (`feat:`, `refactor:`, `docs:`, `test:`) ending with `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`.

---

### Task 1: Crate scaffold + record model (`record.rs`)

**Files:**
- Create: `extensions/memory/finstack-ai-memory/Cargo.toml`
- Create: `extensions/memory/finstack-ai-memory/README.md`
- Create: `extensions/memory/finstack-ai-memory/src/lib.rs`
- Create: `extensions/memory/finstack-ai-memory/src/record.rs`
- Create: `extensions/memory/finstack-ai-memory/src/tests/mod.rs`
- Create: `extensions/memory/finstack-ai-memory/src/tests/record.rs`
- Modify: `Cargo.toml` (workspace root — members list near line 30, dependencies table near line 112)

**Interfaces:**
- Produces (used by every later task):
  - `MemoryId(Arc<str>)` — newtype, `MemoryId::parse(&str) -> Result<Self, MemoryError>` rejecting empty/NUL/`> 256` bytes.
  - `MemoryScope { tenant: Arc<str>, user: Option<Arc<str>>, agent: Option<Arc<str>>, workspace: Option<Arc<str>> }` with `MemoryScope::try_new(tenant) -> Result<Self, MemoryError>` (validates tenant non-empty, no NUL) plus builder-style `with_user/with_agent/with_workspace`, and `fn permits(&self, record_scope: &MemoryScope) -> bool` (tenant must match exactly; each optional field filters only when set on `self`).
  - `ExtractionMethod { Explicit, ToolWrite, ObserverCapture, Imported }` (serde snake_case).
  - `MemoryProvenance { source_session: Option<Arc<str>>, source_run: Option<Arc<str>>, source_ref: Option<Arc<str>>, extraction: ExtractionMethod, confidence: u8 }` — `confidence` validated 0..=100 in `MemoryRecord::validate`.
  - `MemoryBody { Inline(Arc<str>), Blob(finstack_ai_kernel::ArtifactRef) }`.
  - `RetentionPolicy { KeepUntilDeleted, ExpireAfterMs(u64) }` (serde snake_case).
  - `MemoryRecord { id: MemoryId, scope: MemoryScope, keywords: Arc<[Arc<str>]>, body: MemoryBody, preview: Arc<str>, sensitivity: finstack_ai_kernel::Sensitivity, provenance: MemoryProvenance, created_at: Timestamp, last_confirmed_at: Timestamp, supersedes: Option<MemoryId>, superseded_by: Option<MemoryId>, retention: RetentionPolicy, tombstoned: bool }` with `fn validate(&self) -> Result<(), MemoryError>` (id valid, preview ≤ 256 chars, keywords each non-empty/≤ 128 bytes/≤ 64 keywords, confidence ≤ 100).
  - `MemoryError` enum: `Configuration { reason: &'static str }`, `InvalidRecord { reason: &'static str }` with codes `memory_configuration_invalid` / `memory_record_invalid`.
  - `MemoryClock = Arc<dyn Fn() -> Timestamp + Send + Sync>` type alias, plus `pub fn system_clock() -> MemoryClock` (native: `std::time::SystemTime` → `Timestamp::from_unix_ms`, clamped to `UNIX_EPOCH` on error; `cfg(not(target_arch = "wasm32"))` — wasm callers must inject their own).

- [ ] **Step 1: Register the crate in the workspace**

In root `Cargo.toml` add to `[workspace] members`:

```toml
    "extensions/memory/finstack-ai-memory",
```

and to `[workspace.dependencies]` (alongside the existing extension entries near line 112):

```toml
finstack-ai-memory = { path = "extensions/memory/finstack-ai-memory", version = "1.0.0" }
```

- [ ] **Step 2: Create crate manifest**

`extensions/memory/finstack-ai-memory/Cargo.toml` (mirror `extensions/stores/finstack-ai-store-sqlite/Cargo.toml` header fields):

```toml
[package]
name = "finstack-ai-memory"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true
homepage.workspace = true
authors.workspace = true
description = "Memory extension composition (store, recall provider, toolset, observer) for finstack-ai"
publish = false

[features]
default = []
sqlite = ["dep:rusqlite"]

[dependencies]
finstack-ai-kernel = { workspace = true }
finstack-ai-runtime = { workspace = true }
futures-util = { workspace = true }
rusqlite = { workspace = true, optional = true }
serde = { workspace = true }
serde_json = { workspace = true }
thiserror = { workspace = true }

[dev-dependencies]
tempfile = { workspace = true }
tokio = { workspace = true }

[lints]
workspace = true
```

(If any of these dep names are missing from `[workspace.dependencies]`, check how sibling extensions reference them and match exactly.)

- [ ] **Step 3: Write failing record tests**

`src/tests/mod.rs`:

```rust
mod record;
```

`src/tests/record.rs`:

```rust
use crate::record::*;
use finstack_ai_kernel::{Sensitivity, Timestamp};
use std::sync::Arc;

fn sample_record(id: &str, tenant: &str) -> MemoryRecord {
    MemoryRecord {
        id: MemoryId::parse(id).unwrap(),
        scope: MemoryScope::try_new(tenant).unwrap(),
        keywords: Arc::from([Arc::<str>::from("alpha")]),
        body: MemoryBody::Inline(Arc::from("body text")),
        preview: Arc::from("body text"),
        sensitivity: Sensitivity::default(),
        provenance: MemoryProvenance {
            source_session: None,
            source_run: None,
            source_ref: None,
            extraction: ExtractionMethod::Explicit,
            confidence: 80,
        },
        created_at: Timestamp::UNIX_EPOCH,
        last_confirmed_at: Timestamp::UNIX_EPOCH,
        supersedes: None,
        superseded_by: None,
        retention: RetentionPolicy::KeepUntilDeleted,
        tombstoned: false,
    }
}

#[test]
fn memory_id_rejects_empty_and_nul() {
    assert!(MemoryId::parse("").is_err());
    assert!(MemoryId::parse("a\0b").is_err());
    assert!(MemoryId::parse("ok-id").is_ok());
}

#[test]
fn scope_permits_filters_by_optional_fields() {
    let record_scope = MemoryScope::try_new("t1").unwrap().with_user("u1");
    let tenant_only = MemoryScope::try_new("t1").unwrap();
    let wrong_tenant = MemoryScope::try_new("t2").unwrap();
    let matching_user = MemoryScope::try_new("t1").unwrap().with_user("u1");
    let other_user = MemoryScope::try_new("t1").unwrap().with_user("u2");
    assert!(tenant_only.permits(&record_scope));
    assert!(matching_user.permits(&record_scope));
    assert!(!other_user.permits(&record_scope));
    assert!(!wrong_tenant.permits(&record_scope));
}

#[test]
fn record_validation_bounds_confidence_and_preview() {
    let mut record = sample_record("m1", "t1");
    record.provenance.confidence = 101;
    assert!(record.validate().is_err());
    let mut record = sample_record("m1", "t1");
    record.preview = Arc::from("x".repeat(300));
    assert!(record.validate().is_err());
    assert!(sample_record("m1", "t1").validate().is_ok());
}
```

Note: `Sensitivity::default()` — check `finstack_ai_kernel::Sensitivity` for its constructor (the old crate at `extensions/context/finstack-ai-context-memory/src/tests.rs` shows how tests build one; copy that idiom if `default()` does not exist).

- [ ] **Step 4: Run tests to verify they fail to compile**

Run: `cargo test -p finstack-ai-memory` — expected: compile error (module `record` missing).

- [ ] **Step 5: Implement `record.rs` and `lib.rs`**

`src/lib.rs` (crate doc + full lint header copied from calculator crate, then):

```rust
pub mod record;

pub use record::{
    ExtractionMethod, MemoryBody, MemoryClock, MemoryError, MemoryId, MemoryProvenance,
    MemoryRecord, MemoryScope, RetentionPolicy, system_clock,
};

#[cfg(test)]
mod tests;
```

`src/record.rs` implements the **Produces** interface exactly as listed above. Key implementation notes:

```rust
/// Bounded, validated memory identity.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MemoryId(Arc<str>);

impl MemoryId {
    pub fn parse(value: &str) -> Result<Self, MemoryError> {
        if value.is_empty() || value.len() > 256 || value.as_bytes().contains(&0) {
            return Err(MemoryError::InvalidRecord { reason: "invalid_memory_id" });
        }
        Ok(Self(Arc::from(value)))
    }
    #[must_use]
    pub fn as_str(&self) -> &str { &self.0 }
}
```

`permits` semantics (asymmetric — `self` is the caller's scope filter, argument is the record's scope):

```rust
#[must_use]
pub fn permits(&self, record_scope: &MemoryScope) -> bool {
    if self.tenant.as_ref() != record_scope.tenant.as_ref() {
        return false;
    }
    fn field_ok(filter: &Option<Arc<str>>, value: &Option<Arc<str>>) -> bool {
        match filter {
            None => true,
            Some(want) => value.as_deref() == Some(want.as_ref()),
        }
    }
    field_ok(&self.user, &record_scope.user)
        && field_ok(&self.agent, &record_scope.agent)
        && field_ok(&self.workspace, &record_scope.workspace)
}
```

All types derive `Debug, Clone, PartialEq, Eq, Serialize, Deserialize` (serde `deny_unknown_fields` on structs, `rename_all = "snake_case"` on enums). Every public item gets a doc comment (crate warns `missing_docs`).

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p finstack-ai-memory` — expected: all record tests PASS. Then `cargo clippy -p finstack-ai-memory --all-targets --all-features` — expected: clean.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml Cargo.lock extensions/memory/
git commit -m "feat(memory): scaffold finstack-ai-memory crate with record model"
```

---

### Task 2: `MemoryStore` trait + `InProcessMemoryStore`

**Files:**
- Create: `extensions/memory/finstack-ai-memory/src/store/mod.rs`
- Create: `extensions/memory/finstack-ai-memory/src/store/in_process.rs`
- Create: `extensions/memory/finstack-ai-memory/src/tests/store.rs`
- Modify: `extensions/memory/finstack-ai-memory/src/lib.rs` (add `pub mod store;` + re-exports)
- Modify: `extensions/memory/finstack-ai-memory/src/tests/mod.rs` (add `mod store;`)

**Interfaces:**
- Consumes: everything from Task 1.
- Produces:
  - `MemoryQuery` — `#[non_exhaustive] pub enum MemoryQuery { ExactId(MemoryId), Keywords(Arc<[Arc<str>]>), FullText(Arc<str>) }`.
  - `MemoryHit { record: MemoryRecord, score: u32, matched: MatchEvidence }` where `MatchEvidence { ExactId, Keyword(Arc<str>), FullText }`.
  - `PutOutcome { Inserted, AlreadyApplied }`.
  - `MemoryPage { offset: usize, limit: usize }` and `MemoryListing { records: Vec<MemoryRecord>, total: usize }`.
  - `MemoryStoreError` enum with variants/codes: `Unavailable { message: Arc<str> }` (`memory_store_unavailable`), `NotFound` (`memory_not_found`), `ScopeMismatch` (`memory_scope_mismatch`), `InvalidRecord { reason: &'static str }` (`memory_record_invalid`).
  - Trait (exact signatures — later tasks call these):

```rust
pub trait MemoryStore: Send + Sync {
    fn put(&self, idempotency_key: Arc<str>, record: MemoryRecord)
        -> PortFuture<Result<PutOutcome, MemoryStoreError>>;
    fn get(&self, scope: MemoryScope, id: MemoryId)
        -> PortFuture<Result<Option<MemoryRecord>, MemoryStoreError>>;
    fn search(&self, scope: MemoryScope, query: MemoryQuery, limit: usize)
        -> PortFuture<Result<Vec<MemoryHit>, MemoryStoreError>>;
    fn forget(&self, idempotency_key: Arc<str>, scope: MemoryScope, id: MemoryId)
        -> PortFuture<Result<(), MemoryStoreError>>;
    fn correct(&self, idempotency_key: Arc<str>, scope: MemoryScope,
               old: MemoryId, replacement: MemoryRecord)
        -> PortFuture<Result<(), MemoryStoreError>>;
    fn list(&self, scope: MemoryScope, page: MemoryPage)
        -> PortFuture<Result<MemoryListing, MemoryStoreError>>;
}
```

  - `InProcessMemoryStore::new() -> Self` (Default too) — `Mutex<BTreeMap<MemoryId, MemoryRecord>>` plus `Mutex<BTreeSet<Arc<str>>>` of applied idempotency keys.
  - Moved from old crate: `InProcessArtifactStore` (copy `extensions/context/finstack-ai-context-memory/src/lib.rs:65-139` verbatim into `store/in_process.rs`, re-export from crate root — the Python bindings link it).

- [ ] **Step 1: Write failing store tests**

`src/tests/store.rs` (uses `tokio::test` for async; check how the old crate's `src/tests.rs` drives `PortFuture`s and copy that idiom — likely `futures` block_on or tokio):

```rust
use crate::record::*;
use crate::store::*;
use std::sync::Arc;

// reuse sample_record from tests/record.rs — move it to tests/mod.rs as
// pub(crate) fn sample_record(...) so both modules share it.

#[tokio::test]
async fn put_is_idempotent_by_key() {
    let store = InProcessMemoryStore::new();
    let record = crate::tests::sample_record("m1", "t1");
    let first = store.put(Arc::from("k1"), record.clone()).await.unwrap();
    let second = store.put(Arc::from("k1"), record).await.unwrap();
    assert_eq!(first, PutOutcome::Inserted);
    assert_eq!(second, PutOutcome::AlreadyApplied);
}

#[tokio::test]
async fn search_excludes_tombstoned_and_superseded() {
    let store = InProcessMemoryStore::new();
    let scope = MemoryScope::try_new("t1").unwrap();
    store.put(Arc::from("k1"), crate::tests::sample_record("m1", "t1")).await.unwrap();
    store.forget(Arc::from("k2"), scope.clone(), MemoryId::parse("m1").unwrap()).await.unwrap();
    let hits = store.search(scope, MemoryQuery::Keywords(Arc::from([Arc::<str>::from("alpha")])), 10).await.unwrap();
    assert!(hits.is_empty());
}

#[tokio::test]
async fn correct_links_supersession_and_hides_old() {
    let store = InProcessMemoryStore::new();
    let scope = MemoryScope::try_new("t1").unwrap();
    store.put(Arc::from("k1"), crate::tests::sample_record("m1", "t1")).await.unwrap();
    let replacement = crate::tests::sample_record("m2", "t1");
    store.correct(Arc::from("k2"), scope.clone(), MemoryId::parse("m1").unwrap(), replacement).await.unwrap();
    let old = store.get(scope.clone(), MemoryId::parse("m1").unwrap()).await.unwrap().unwrap();
    assert_eq!(old.superseded_by, Some(MemoryId::parse("m2").unwrap()));
    let new = store.get(scope.clone(), MemoryId::parse("m2").unwrap()).await.unwrap().unwrap();
    assert_eq!(new.supersedes, Some(MemoryId::parse("m1").unwrap()));
    let hits = store.search(scope, MemoryQuery::ExactId(MemoryId::parse("m1").unwrap()), 10).await.unwrap();
    assert!(hits.is_empty());
}

#[tokio::test]
async fn scope_filters_reads_and_search() {
    let store = InProcessMemoryStore::new();
    store.put(Arc::from("k1"), crate::tests::sample_record("m1", "t1")).await.unwrap();
    let other = MemoryScope::try_new("t2").unwrap();
    assert!(store.get(other.clone(), MemoryId::parse("m1").unwrap()).await.unwrap().is_none());
    let hits = store.search(other, MemoryQuery::FullText(Arc::from("body")), 10).await.unwrap();
    assert!(hits.is_empty());
}

#[tokio::test]
async fn full_text_matches_preview_substring() {
    let store = InProcessMemoryStore::new();
    let scope = MemoryScope::try_new("t1").unwrap();
    store.put(Arc::from("k1"), crate::tests::sample_record("m1", "t1")).await.unwrap();
    let hits = store.search(scope, MemoryQuery::FullText(Arc::from("body text")), 10).await.unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].matched, MatchEvidence::FullText);
}
```

If `tokio::test` is unavailable as a dev-dependency feature, use the same async test harness the old crate's tests use.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p finstack-ai-memory` — expected: compile failure (`store` module missing).

- [ ] **Step 3: Implement `store/mod.rs` + `store/in_process.rs`**

Implementation notes for `InProcessMemoryStore`:

- `put`: validate record (`record.validate()` → `InvalidRecord`), insert idempotency key first (`BTreeSet::insert` returning false ⇒ `AlreadyApplied` without touching records).
- `forget`: idempotency-checked; set `tombstoned = true` on the record if present and scope-permitted; missing record ⇒ `NotFound`.
- `correct`: idempotency-checked; validate replacement; set `replacement.supersedes = Some(old)`; write replacement; set old record's `superseded_by`.
- `search`: filter `scope.permits(&record.scope)`, drop `tombstoned || superseded_by.is_some()`, then match:
  - `ExactId` — key lookup, score 100, `MatchEvidence::ExactId`;
  - `Keywords` — case-insensitive keyword equality, score = number of matched keywords, evidence carries first matched keyword;
  - `FullText` — case-insensitive substring over preview + inline body, score 10.
  Sort hits by `(score desc, id asc)`, truncate to `limit`.
- `list`: scope-filtered, includes tombstoned/superseded (for `inspect`/management), ordered by id, offset/limit paged, `total` = pre-page count.
- Locks: `Mutex` poisoning maps to `Unavailable { message: "memory store lock failed".into() }` — copy the pattern from the old crate (`lib.rs:83-93`).
- Copy `InProcessArtifactStore` verbatim from the old crate into this file; `pub use` it in `store/mod.rs` and crate root.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p finstack-ai-memory` and `cargo clippy -p finstack-ai-memory --all-targets` — expected: PASS, clean.

- [ ] **Step 5: Commit**

```bash
git add extensions/memory/
git commit -m "feat(memory): MemoryStore trait and in-process implementation"
```

---

### Task 3: `SqliteMemoryStore` (feature `sqlite`)

**Files:**
- Create: `extensions/memory/finstack-ai-memory/src/store/sqlite.rs`
- Create: `extensions/memory/finstack-ai-memory/src/tests/sqlite.rs`
- Modify: `extensions/memory/finstack-ai-memory/src/store/mod.rs` (`#[cfg(feature = "sqlite")] pub mod sqlite;` + re-export)
- Modify: `extensions/memory/finstack-ai-memory/src/tests/mod.rs` (`#[cfg(feature = "sqlite")] mod sqlite;`)

**Interfaces:**
- Consumes: `MemoryStore` trait + all Task 1/2 types.
- Produces: `SqliteMemoryStore::open(path: &std::path::Path) -> Result<Self, MemoryStoreError>` and `SqliteMemoryStore::open_in_memory() -> Result<Self, MemoryStoreError>`; implements `MemoryStore`. Native-only: entire module `#[cfg(all(feature = "sqlite", not(target_arch = "wasm32")))]`.

- [ ] **Step 1: Study the existing SQLite store**

Read `extensions/stores/finstack-ai-store-sqlite/src/schema.rs` and `store.rs` for: connection handling (`Mutex<rusqlite::Connection>` vs worker thread — mirror whichever it uses for simple cases), pragma setup (WAL etc.), and migration idiom. Follow those patterns; do NOT invent new ones. Also verify the workspace `rusqlite` has the `bundled` or FTS5-capable feature — run `grep -n "rusqlite" Cargo.toml` at the root; if FTS5 is not enabled, add the `"bundled"` feature to the workspace dep (bundled SQLite includes FTS5).

- [ ] **Step 2: Write failing tests**

`src/tests/sqlite.rs` — same behavioral suite as Task 2 (idempotency, tombstone, supersession, scope filtering) parameterized over `SqliteMemoryStore::open_in_memory()`, plus SQLite-specific tests:

```rust
#[tokio::test]
async fn full_text_ranks_with_bm25() {
    let store = SqliteMemoryStore::open_in_memory().unwrap();
    let scope = MemoryScope::try_new("t1").unwrap();
    let mut a = crate::tests::sample_record("m-a", "t1");
    a.body = MemoryBody::Inline(Arc::from("rust memory extension design"));
    a.preview = Arc::from("rust memory extension design");
    let mut b = crate::tests::sample_record("m-b", "t1");
    b.body = MemoryBody::Inline(Arc::from("memory"));
    b.preview = Arc::from("memory");
    store.put(Arc::from("k1"), a).await.unwrap();
    store.put(Arc::from("k2"), b).await.unwrap();
    let hits = store.search(scope, MemoryQuery::FullText(Arc::from("memory extension")), 10).await.unwrap();
    assert_eq!(hits[0].record.id.as_str(), "m-a"); // both terms match ⇒ ranks first
}

#[tokio::test]
async fn persists_across_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mem.sqlite");
    {
        let store = SqliteMemoryStore::open(&path).unwrap();
        store.put(Arc::from("k1"), crate::tests::sample_record("m1", "t1")).await.unwrap();
    }
    let store = SqliteMemoryStore::open(&path).unwrap();
    let scope = MemoryScope::try_new("t1").unwrap();
    assert!(store.get(scope, MemoryId::parse("m1").unwrap()).await.unwrap().is_some());
}

#[tokio::test]
async fn tombstone_removes_from_fts() {
    let store = SqliteMemoryStore::open_in_memory().unwrap();
    let scope = MemoryScope::try_new("t1").unwrap();
    store.put(Arc::from("k1"), crate::tests::sample_record("m1", "t1")).await.unwrap();
    store.forget(Arc::from("k2"), scope.clone(), MemoryId::parse("m1").unwrap()).await.unwrap();
    let hits = store.search(scope, MemoryQuery::FullText(Arc::from("body")), 10).await.unwrap();
    assert!(hits.is_empty());
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p finstack-ai-memory --features sqlite` — expected: compile failure.

- [ ] **Step 4: Implement schema + store**

Schema (executed on open, with a `memory_schema_version` pragma/user_version guard):

```sql
CREATE TABLE IF NOT EXISTS memory_records (
  id TEXT PRIMARY KEY,
  tenant TEXT NOT NULL,
  user TEXT, agent TEXT, workspace TEXT,
  body_inline TEXT,
  blob_ref_json TEXT,
  preview TEXT NOT NULL,
  sensitivity TEXT NOT NULL,
  keywords_json TEXT NOT NULL,
  provenance_json TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  last_confirmed_at INTEGER NOT NULL,
  supersedes TEXT, superseded_by TEXT,
  retention_json TEXT NOT NULL,
  tombstoned INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS memory_records_tenant ON memory_records(tenant);
CREATE VIRTUAL TABLE IF NOT EXISTS memory_fts USING fts5(
  id UNINDEXED, preview, body, keywords
);
CREATE TABLE IF NOT EXISTS memory_idempotency (
  key TEXT PRIMARY KEY,
  applied_at INTEGER NOT NULL
);
```

Implementation notes:

- Records serialize the JSON-ish columns with `serde_json`; timestamps as `as_unix_ms()`; `sensitivity` via its serde representation.
- Every mutation runs in one transaction: `INSERT INTO memory_idempotency` first — `SQLITE_CONSTRAINT` on the primary key short-circuits to `AlreadyApplied`/no-op success.
- `put` also inserts the FTS row (`preview`, inline body or empty, keywords joined by space). `forget`/`correct` delete the FTS row for tombstoned/superseded ids.
- `FullText` search: `SELECT r.* FROM memory_fts f JOIN memory_records r ON r.id = f.id WHERE memory_fts MATCH ?1 AND r.tombstoned = 0 AND r.superseded_by IS NULL AND r.tenant = ?2 ORDER BY bm25(memory_fts) LIMIT ?3`, then apply the optional scope fields with `scope.permits` in Rust (simpler than dynamic SQL, record counts are bounded by LIMIT — over-fetch by 4x before Rust filtering, then truncate). Score: map rank order to descending `u32` (bm25 returns floats; do NOT store floats — use row order).
- Sanitize the FTS MATCH input: wrap each whitespace token in double quotes to prevent FTS query-syntax injection.
- `Keywords` search via FTS on the keywords column; `ExactId` via primary key.
- Connection: `Mutex<rusqlite::Connection>` with WAL + `busy_timeout`, matching `finstack-ai-store-sqlite`'s simpler paths. All rusqlite errors map to `Unavailable { message }` with non-secret text.
- `PortFuture`: operations are synchronous under the mutex; wrap results in `Box::pin(async move { ... })` after doing the blocking work before the future (same style as the old crate's lock-then-async pattern at `extensions/context/finstack-ai-context-memory/src/lib.rs:83-93`).

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p finstack-ai-memory --features sqlite` and `cargo clippy -p finstack-ai-memory --all-targets --features sqlite` — expected: PASS, clean.

- [ ] **Step 6: Commit**

```bash
git add extensions/memory/ Cargo.toml Cargo.lock
git commit -m "feat(memory): SQLite FTS5 MemoryStore implementation behind sqlite feature"
```

---

### Task 4: `MemoryContextProvider` (recall)

**Files:**
- Create: `extensions/memory/finstack-ai-memory/src/provider.rs`
- Create: `extensions/memory/finstack-ai-memory/src/tests/provider.rs`
- Modify: `extensions/memory/finstack-ai-memory/src/lib.rs`, `src/tests/mod.rs`

**Interfaces:**
- Consumes: `MemoryStore` (Task 2), record types (Task 1).
- Produces:
  - `RecallConfig { max_hits: usize, stable_prefix: usize }` with `Default { max_hits: 8, stable_prefix: 4 }`.
  - `MemoryContextProvider::try_new(store: Arc<dyn MemoryStore>, scope: MemoryScope, config: RecallConfig) -> Result<Self, MemoryError>`; implements `finstack_ai_runtime::ContextProvider`.
  - Component id stays `finstack.context.memory`; bump `Version` minor from the old crate's `0.0.4` to `0.1.0`.

- [ ] **Step 1: Port the old provider's structure**

Start from `extensions/context/finstack-ai-context-memory/src/lib.rs:141-391` — keep: descriptor construction, tenant re-check against `ctx.run.locator.tenant_scope`, `query_text`, `apply_budget`, `estimate_tokens`, `contribution_invalid`. Replace: the private index with `self.store.search(...)`.

- [ ] **Step 2: Write failing tests**

Port the old crate's provider tests from `extensions/context/finstack-ai-context-memory/src/tests.rs` (they exercise budget reject/truncate, tenant mismatch, keyword match — adapt construction to `InProcessMemoryStore` + `put`). Add new cache-stability tests:

```rust
#[tokio::test]
async fn recall_order_is_deterministic_by_tier_then_id() {
    // put three records with equal keyword match ("alpha"): ids m-c, m-a, m-b
    // collect() twice with the same request
    // assert both contributions list items in id order: m-a, m-b, m-c
    // and the two contributions are equal
}

#[tokio::test]
async fn cache_key_stable_when_recall_unchanged() {
    // collect() twice; assert contribution cache keys are equal.
    // put a new matching record; collect(); assert cache key differs.
}

#[tokio::test]
async fn recall_skips_tombstoned_records() {
    // put + forget a record; collect(); assert no items.
}
```

Fill these in with the same `ContextCallContext`/`ContextRequest` fixture idioms the old tests use (copy their helper constructors verbatim first, then adapt). Check `ContextContribution` for its cache-key field name (`grep -n "cache_key" crates/finstack-ai-runtime/src/ports/context/ -r`) — the spec relies on `ContextContribution.cache_key`; wire it via whatever constructor the runtime exposes (`ContextContribution::try_new` currently takes `(items, Some("finstack.context.memory"))` — inspect its signature for the cache-key parameter or builder method and use it).

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p finstack-ai-memory` — expected: compile failure (no `provider` module).

- [ ] **Step 4: Implement recall**

`collect` flow:

1. Tenant check (as old code).
2. `query_text(&request)`; empty query ⇒ empty contribution.
3. Two searches: `MemoryQuery::Keywords(tokens)` and `MemoryQuery::FullText(query)`, each `limit = config.max_hits * 2`.
4. Merge, dedupe by id (keep highest score), drop anything tombstoned/superseded (store already filters; assert-by-filter anyway).
5. Tier = score bucket (`ExactId`/keyword-count ⇒ tier 0, full-text ⇒ tier 1). Sort `(tier asc, id asc)`. Truncate to `config.max_hits`.
6. Build `ContextItem`s exactly as the old code (`ContextItemKind::Reference`, preview text + artifact name, provenance `source_id = "finstack.context.memory"`, `source_ref = memory id`, `external = true`, `ContextAuthority::Untrusted`, per-record sensitivity, `estimate_tokens(preview)`).
7. `apply_budget` (ported unchanged).
8. Cache key: `Digest::raw_json` over the concatenated `(id, last_confirmed_at.as_unix_ms())` pairs in final order; attach to the contribution.

`stable_prefix` semantics: the provider keeps a `Mutex<Vec<MemoryId>>` of the ids of the first `config.stable_prefix` items from the last successful collect **in this provider instance**; on the next collect, hits whose ids are in that list sort before all others (in their remembered order) regardless of tier. Update the list after each collect. Document that the prefix is per-instance (per-agent) state, intentionally not persisted.

- [ ] **Step 5: Run tests, clippy, commit**

Run: `cargo test -p finstack-ai-memory` + clippy. Expected: PASS.

```bash
git add extensions/memory/
git commit -m "feat(memory): store-backed recall provider with cache-stable ordering"
```

---

### Task 5: `MemoryToolset` + `MemoryPolicy`

**Files:**
- Create: `extensions/memory/finstack-ai-memory/src/toolset.rs`
- Create: `extensions/memory/finstack-ai-memory/src/tests/toolset.rs`
- Modify: `extensions/memory/finstack-ai-memory/src/lib.rs`, `src/tests/mod.rs`

**Interfaces:**
- Consumes: `MemoryStore`, record types, `MemoryClock`.
- Produces:
  - `MemoryPolicy { read: bool, write: bool, manage: bool, consolidate: bool, profile: bool }` (`Default`: read+write true, rest false). `consolidate`/`profile` are reserved: accepted, currently gate nothing.
  - `MemoryToolset::try_new(store: Arc<dyn MemoryStore>, artifact_store: Arc<dyn finstack_ai_runtime::ArtifactStore>, scope: MemoryScope, policy: MemoryPolicy, clock: MemoryClock) -> Result<Self, MemoryError>`; implements `finstack_ai_runtime::Toolset`.
  - Tool ids: `finstack.tools.memory.remember|search|inspect|forget|correct`; model names: `remember`, `search_memory`, `inspect_memory`, `forget_memory`, `correct_memory`.
  - Inline body threshold: `pub const INLINE_BODY_MAX_BYTES: usize = 4096;`

- [ ] **Step 1: Study the Toolset pattern**

Template: `extensions/toolsets/finstack-ai-tools-calculator/src/lib.rs` — `ToolSpec` construction with `RawJson::parse` schemas + `spec.validate()`, `ToolsetDescriptor { name, metadata }`, `call` dispatch, `ToolResult`/`ToolStreamItem::Completed` via `futures_util::stream::once`, and its `validate_call_context` helper (later in the same file — copy it, it checks tool id/name against the validated call). Also read `ValidatedToolCall` (`finstack_ai_kernel`) for how to get the tool name and `call.call.arguments()`.

- [ ] **Step 2: Write failing tests**

`src/tests/toolset.rs`:

```rust
#[test]
fn policy_gates_registered_tools() {
    // read-only policy ⇒ tools() names == {search_memory, inspect_memory}
    // write-only ⇒ {remember}
    // manage ⇒ {forget_memory, correct_memory}
    // default ⇒ {remember, search_memory, inspect_memory}
    // all-true ⇒ all five
}

#[tokio::test]
async fn remember_is_idempotent_across_replay() {
    // Build toolset over InProcessMemoryStore + InProcessArtifactStore.
    // Construct a ToolCallContext fixture (copy the fixture idiom from
    // extensions/toolsets/finstack-ai-tools-calculator/src/ tests — same
    // RunCallContext/ToolBatchId/ToolCallId setup) with a fixed effect_id.
    // call remember twice with identical ctx + args.
    // Assert both succeed and store.list(scope, ...) has exactly 1 record.
}

#[tokio::test]
async fn remember_stages_large_bodies_as_blobs() {
    // body > INLINE_BODY_MAX_BYTES ⇒ stored record has MemoryBody::Blob(_)
    // and preview is 256 chars.
}

#[tokio::test]
async fn search_memory_returns_hits_and_never_tombstoned() { /* put, forget via store, call search_memory, assert empty */ }

#[tokio::test]
async fn forget_and_correct_require_ids_and_are_idempotent() { /* call forget twice same ctx; second succeeds; record tombstoned */ }

#[tokio::test]
async fn scope_comes_from_configuration_not_arguments() {
    // args containing a "tenant" field are rejected by deny_unknown_fields /
    // schema (additionalProperties: false): calling remember with an extra
    // "tenant" key yields a validation ToolError.
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p finstack-ai-memory` — expected: compile failure.

- [ ] **Step 4: Implement toolset**

Argument structs (serde, `deny_unknown_fields`) and JSON schemas (`additionalProperties: false`, mirror the calculator's inline `RawJson::parse` style):

- `remember`: `{ id?: string, keywords: string[], body: string, sensitivity?: string }` → derive id when absent as `mem-{hex}` from `Digest::blob_content(body)` first 16 hex chars. Result `{ "id": string, "outcome": "inserted" | "already_applied" }`.
- `search_memory`: `{ id?: string, keywords?: string[], text?: string, limit?: integer (1..=25, default 8) }` — exactly one of id/keywords/text required (validate in code, reason `memory_query_invalid`). Result `{ "hits": [{ "id", "preview", "score", "matched" }] }`.
- `inspect_memory`: `{ id: string }` → full record metadata JSON (serialize the `MemoryRecord`, body replaced by `{"inline": string}` or `{"blob_name": string}`). `NotFound` ⇒ `is_error: true` result with code in payload, not a `ToolError`.
- `forget_memory`: `{ id: string }` → `{ "id", "tombstoned": true }`.
- `correct_memory`: `{ old_id: string, keywords: string[], body: string, sensitivity?: string }` → builds the replacement record (id derived like remember) and calls `store.correct`. Result `{ "old_id", "new_id" }`.

Spec metadata per tool: reads are `SideEffectClass::ReadOnly` + `RetrySafety::SafeToRetry` + `ApprovalRequirement::NotRequired`; mutations (`remember`, `forget_memory`, `correct_memory`) — check `SideEffectClass` variants (`grep -n "pub enum SideEffectClass" crates/ -r`) and use the write/idempotent variant, `RetrySafety::SafeToRetry` (they are idempotent), approval `NotRequired`, `ToolExecutionMode::Parallel`, `max_result_bytes: 16_384`, `deferral: ToolDeferralSupport::Never`.

Idempotency key for every mutation: `Arc::from(format!("tool:{}", ctx.run.effect_id))` — check `EffectId`'s Display/to-hex accessor and use the canonical text form.

Timestamps: `created_at = last_confirmed_at = (self.clock)()`.

Large bodies: when `body.len() > INLINE_BODY_MAX_BYTES`, stage through `stage_required_artifact` with `ArtifactScope { tenant_scope: scope.tenant, session_id: ctx.run.locator.session_id, run_id: Some(ctx.run.locator.run_id), sensitivity }` and `ArtifactMetadata { kind: "memory-record", media_type: "text/plain", name: Some(id), attributes: Metadata::empty() }` (exact same call as the old crate's `stage()` at `lib.rs:226-243`); store `MemoryBody::Blob(artifact)`.

`reconcile`: for `PendingToolEffect` on a mutating tool, re-run the same store call with the same derived idempotency key and map success (`AlreadyApplied` included) to `ToolReconcileResult::Completed(...)` with the same result JSON shape; unknown tool ⇒ `Unknown`.

Errors: `MEMORY_TOOL_INVALID_ARGUMENTS = "memory_tool_invalid_arguments"` (Validation), `MEMORY_TOOL_UNAVAILABLE = "memory_tool_unavailable"` (map `MemoryStoreError::Unavailable`), reuse `memory_not_found` inside `is_error` payloads.

- [ ] **Step 5: Run tests, clippy, commit**

```bash
git add extensions/memory/
git commit -m "feat(memory): capability-gated MemoryToolset with idempotent mutations"
```

---

### Task 6: `MemoryExtractor` + `MemoryObserver`

**Files:**
- Create: `extensions/memory/finstack-ai-memory/src/extract.rs`
- Create: `extensions/memory/finstack-ai-memory/src/observer.rs`
- Create: `extensions/memory/finstack-ai-memory/src/tests/observer.rs`
- Modify: `extensions/memory/finstack-ai-memory/src/lib.rs`, `src/tests/mod.rs`

**Interfaces:**
- Consumes: `MemoryStore`, record types, `MemoryClock`.
- Produces:
  - `CandidateMemory { id: Option<MemoryId>, keywords: Vec<Arc<str>>, body: Arc<str>, sensitivity: Sensitivity, confidence: u8, source_run: Option<Arc<str>>, source_ref: Option<Arc<str>> }`.
  - `pub trait MemoryExtractor: Send + Sync { fn extract(&self, events: &[finstack_ai_kernel::RunEvent]) -> Vec<CandidateMemory>; }`
  - `RuleBasedExtractor::new(marker: Arc<str>) -> Self` — default marker `"[[remember]]"`; scans terminal-event text bodies for lines beginning with the marker; each such line yields one candidate (`body` = line minus marker, keywords = first 5 whitespace tokens lowercased, confidence 60).
  - `MemoryObserver::try_new(store: Arc<dyn MemoryStore>, scope: MemoryScope, extractor: Arc<dyn MemoryExtractor>, clock: MemoryClock) -> Result<Self, MemoryError>`; implements `finstack_ai_runtime::Observer`.

- [ ] **Step 1: Study the Observer pattern**

Read `crates/finstack-ai-runtime/src/ports/observer/mod.rs:100-160` (trait + `NoopObserver`) and `extensions/observers/finstack-ai-observer-log/src/lib.rs` (descriptor construction with `ComponentRef`/`ComponentId`/`Version`, `ObserverPayloadMode`). Read `finstack_ai_kernel::RunEvent` accessors (`kind()`, `event_id()`, `run_id()`, `body()`) and its `RunEventKind` variants to find the terminal-completion kind (grep `RunEventKind` in `crates/finstack-ai-kernel`).

- [ ] **Step 2: Write failing tests**

`src/tests/observer.rs`:

```rust
#[test]
fn rule_based_extractor_finds_marked_lines() {
    // Build a RunEvent fixture whose body text contains:
    //   "[[remember]] the user prefers dark mode\nother line"
    // (copy the RunEvent construction idiom from
    //  extensions/observers/finstack-ai-observer-log/src/ tests)
    // assert one candidate with body "the user prefers dark mode".
}

#[tokio::test]
async fn observer_capture_is_idempotent_on_redelivery() {
    // observe(batch) twice with the same events;
    // store.list(scope, ..) has exactly one record;
    // record.provenance.extraction == ExtractionMethod::ObserverCapture.
}

#[tokio::test]
async fn observer_swallows_store_failures() {
    // A failing MemoryStore stub (Unavailable on put) — observe() returns Ok(())
    // (isolation: capture loss must not error the pipeline; the runtime
    //  isolates ObserverError anyway, but we choose Ok + no panic).
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p finstack-ai-memory` — expected: compile failure.

- [ ] **Step 4: Implement**

`observe` flow: filter the batch to terminal/completion events (per the `RunEventKind` found in Step 1) → `extractor.extract(&events)` → for each candidate `i` on event `e`: build a `MemoryRecord` (id = candidate id or digest-derived; provenance `ObserverCapture`, `source_run` = event run id text, `source_ref` = event id text) → `store.put(Arc::from(format!("capture:{run_id}:{event_id}:{i}")), record)`. Any `Err` from `put` is logged into the observer result as `Ok(())` (swallowed) — never propagate a failure that would suggest run impact; genuinely misconfigured construction fails in `try_new` instead.

Descriptor: `ComponentId::parse("finstack.observer.memory")`, `Version { major: 0, minor: 1, patch: 0 }`, payload mode: whichever variant includes bodies (extraction needs `body()`; pick the include-body mode from `ObserverPayloadMode`).

- [ ] **Step 5: Run tests, clippy, commit**

```bash
git add extensions/memory/
git commit -m "feat(memory): observer capture with rule-based extractor"
```

---

### Task 7: Remove `finstack-ai-context-memory`, migrate dependents

**Files:**
- Delete: `extensions/context/finstack-ai-context-memory/` (entire directory)
- Modify: `Cargo.toml` (root: remove member + workspace-dependency lines for the old crate)
- Modify: `bindings/finstack-ai-python/Cargo.toml` + `bindings/finstack-ai-python/src/agent.rs:14` (and any other importer)
- Create: `extensions/memory/finstack-ai-memory/README.md` content (finalize)

**Interfaces:**
- Consumes: crate-root re-exports `InProcessArtifactStore`, `MemoryContextProvider` from `finstack-ai-memory`.
- Produces: a workspace with zero references to `finstack_ai_context_memory`.

- [ ] **Step 1: Find every dependent**

Run: `grep -rln "finstack_ai_context_memory\|finstack-ai-context-memory" --include="*.rs" --include="*.toml" . | grep -v target | grep -v .claude` — expected at minimum: root `Cargo.toml`, `bindings/finstack-ai-python`, the old crate itself. Fix each.

- [ ] **Step 2: Migrate the Python binding import**

In `bindings/finstack-ai-python/Cargo.toml` replace the `finstack-ai-context-memory` dependency with `finstack-ai-memory = { workspace = true }`. In `agent.rs:14`: `use finstack_ai_memory::InProcessArtifactStore;`. If the binding constructs `MemoryContextProvider` anywhere, update the constructor call: it now takes `(Arc<dyn MemoryStore>, MemoryScope, RecallConfig)` — use `InProcessMemoryStore` + `MemoryScope::try_new(tenant)` + `RecallConfig::default()`.

- [ ] **Step 3: Delete the old crate and workspace entries**

```bash
git rm -r extensions/context/finstack-ai-context-memory
```

Remove the member line and the `[workspace.dependencies]` line for it in root `Cargo.toml`.

- [ ] **Step 4: Write the README**

`extensions/memory/finstack-ai-memory/README.md`: what the crate is (the §7.2 composition), the five components, feature flags (`sqlite`), a short construction example (in-process store + provider + toolset), the §7.4 note that capture middleware is deferred, and the vector-retrieval future-work note. Keep the tone/length of the old crate's README.

- [ ] **Step 5: Full workspace verification**

Run: `cargo build --workspace --all-features` then `cargo test --workspace` and `cargo clippy --workspace --all-targets --all-features`. Expected: green with zero references to the old crate. Check exit codes explicitly.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "refactor(memory): remove finstack-ai-context-memory in favor of finstack-ai-memory"
```

---

### Task 8: Python exposure (`MemoryExtension`)

**Files:**
- Create: `bindings/finstack-ai-python/src/memory.rs`
- Modify: `bindings/finstack-ai-python/src/lib.rs` (module registration near line 109: `module.add_class::<PyMemoryExtension>()?;`)
- Modify: `bindings/finstack-ai-python/src/agent.rs` (accept native memory components)
- Create: `examples/python-notebooks/` memory example notebook (name it consistently with siblings — `ls examples/python-notebooks/` first)
- Test: Python-side test in whatever harness the binding uses (`ls bindings/finstack-ai-python/tests/ python/ 2>/dev/null; grep -rn "pytest" bindings/finstack-ai-python examples/ | head`)

**Interfaces:**
- Consumes: all `finstack-ai-memory` public API.
- Produces (Python):

```python
mem = MemoryExtension.in_process(tenant="t1", user=None, agent=None, workspace=None,
                                 read=True, write=True, manage=False)
mem = MemoryExtension.sqlite(path="mem.db", tenant="t1", ...)
# handles usable in existing factory params:
mem.context_provider()  # -> object accepted by context_providers=[...]
mem.toolset()           # -> object accepted by toolsets=[...]
mem.observer()          # -> object accepted by observers=[...]
```

- [ ] **Step 1: Study how the factories accept native vs Python components**

`bindings/finstack-ai-python/src/agent.rs` factory params take `Vec<Py<PyPythonToolset>>` etc. — those are the Python-callback wrappers. Find how the **native** `DocumentToolset` is registered (grep `DocumentToolset` in `agent.rs` and follow to where toolsets are assembled into the runtime registration). The memory components must join at that same native assembly point. Two workable shapes — pick whichever matches the existing code with the least new surface:
  1. If the assembly point takes `Arc<dyn Toolset>` lists internally, add an optional `memory: Option<Py<PyMemoryExtension>>` keyword to each factory and append the native components there (mirroring how `artifact_store`/`attachment_index` thread through).
  2. If `PyPythonToolset` has a native-wrapping constructor, wrap.
Document the choice in the module docs.

- [ ] **Step 2: Write the failing Python test**

Follow the binding's existing test idiom (found in Step 1's survey). Test body:

```python
def test_memory_extension_remember_and_recall():
    mem = MemoryExtension.in_process(tenant="t1")
    # direct smoke of handles:
    assert mem.toolset() is not None
    assert mem.context_provider() is not None
    assert mem.observer() is not None
```

Plus, if the binding has a scripted/fixture agent (there is a `scripted.rs` in the wasm side; check `bindings/finstack-ai-python/src/callback_fixture.rs` and existing tests for an offline agent fixture), add an end-to-end: run an agent whose scripted model calls `remember`, then a second run asserting the provider recalls it. If no offline fixture exists for Python agents, the handle-smoke test plus Rust-side coverage suffices — note that in the test file.

- [ ] **Step 3: Implement `memory.rs`**

`PyMemoryExtension` (frozen pyclass, `name = "MemoryExtension"`): holds `Arc<dyn MemoryStore>`, `MemoryScope`, `MemoryPolicy`, `Arc<InProcessArtifactStore>`, and lazily-constructed `Arc<MemoryContextProvider>` / `Arc<MemoryToolset>` / `Arc<MemoryObserver>`. Static constructors `in_process(...)` and `sqlite(path, ...)` (the latter `#[cfg]`-gated on the binding crate enabling `finstack-ai-memory/sqlite` — add that feature to the binding's Cargo.toml dependency). Map every `MemoryError`/`MemoryStoreError` to the binding's existing error type (see `bindings/finstack-ai-python/src/errors.rs`).

- [ ] **Step 4: Build and run**

Run the binding's build (check for a `justfile`/`Makefile`/maturin config — `ls bindings/finstack-ai-python/` and follow whatever `examples/python-notebooks` docs say about rebuilding the native module). Run the Python test suite. Expected: PASS.

- [ ] **Step 5: Add the notebook example**

Notebook cells: construct `MemoryExtension.sqlite(...)` in a temp dir → agent factory with `toolsets`/`context_providers` wiring per Step 1's chosen shape → prompt that triggers `remember` → second run showing recall → `forget` → note on policy flags. Match the structure and tone of the existing notebooks.

- [ ] **Step 6: Commit**

```bash
git add bindings/finstack-ai-python examples/python-notebooks Cargo.lock
git commit -m "feat(python): MemoryExtension exposing native memory composition"
```

---

### Task 9: WASM exposure (`HostMemoryStore`)

**Files:**
- Create: `bindings/finstack-ai-wasm/src/host_memory.rs`
- Modify: `bindings/finstack-ai-wasm/src/lib.rs` (module + any JS-facing registration, mirroring how `host_store`/`host_context` are exported)
- Modify: `bindings/finstack-ai-wasm/Cargo.toml` (depend on `finstack-ai-memory`, default features only — NOT `sqlite`)
- Test: same-file `#[cfg(test)]` or the crate's fixture-based tests (see `bindings/finstack-ai-wasm/src/fixture.rs`, `scripted.rs` for the harness idiom)

**Interfaces:**
- Consumes: `MemoryStore` trait + record types (serde round-trip).
- Produces: `HostMemoryStore` implementing `MemoryStore` by delegating to host callbacks named `memory_put`, `memory_get`, `memory_search`, `memory_forget`, `memory_correct`, `memory_list`; JSON request/response envelopes defined below. Also `InProcessMemoryStore` re-exported for wasm consumers that skip persistence.

- [ ] **Step 1: Verify the memory crate compiles for wasm32**

Run: `cargo build -p finstack-ai-memory --target wasm32-unknown-unknown` (no features). Fix any native-only leakage (`system_clock` must already be `cfg(not(wasm32))`; `Mutex` from std is fine). Expected: builds clean. If the target isn't installed: `rustup target add wasm32-unknown-unknown`.

- [ ] **Step 2: Study and mirror `host_store.rs`**

`bindings/finstack-ai-wasm/src/host_store.rs` shows the full dual-target pattern: native side uses `HostFailure`/`NativeHostResult` callbacks for tests; wasm side holds `js_sys::Function`s (`JsMethod`), proxies JSON, and maps missing methods to a stable Unavailable error. Copy this structure method-for-method for the six memory operations.

- [ ] **Step 3: Define the JSON envelopes**

Requests serialize the Rust types with serde (they already derive Serialize/Deserialize):

```json
// memory_put:    {"idempotency_key": "...", "record": {MemoryRecord}}
// memory_get:    {"scope": {MemoryScope}, "id": "..."}
// memory_search: {"scope": {MemoryScope}, "query": {"full_text": "..."} , "limit": 8}
// memory_forget: {"idempotency_key": "...", "scope": {...}, "id": "..."}
// memory_correct:{"idempotency_key": "...", "scope": {...}, "old": "...", "replacement": {MemoryRecord}}
// memory_list:   {"scope": {...}, "page": {"offset": 0, "limit": 50}}
```

Responses: `{"ok": <result-json>}` or `{"error": {"code": "...", "message": "..."}}` — map to `MemoryStoreError` by code (`memory_not_found` ⇒ `NotFound`, `memory_scope_mismatch` ⇒ `ScopeMismatch`, else `Unavailable`). `Timestamp` serializes through the record's serde derive — verify `Timestamp` implements Serialize (grep its derives); if it does not, add `#[serde(with = ...)]` epoch-ms adapters on the record fields in Task 1's types (and note the change).

- [ ] **Step 4: Write failing native-callback tests**

Using the native callback side (as `host_store.rs` tests do): a scripted host implementing the six methods over a `HashMap`, then run the Task-2 behavioral suite (idempotent put, tombstone excluded from search, scope filtering) through `HostMemoryStore`.

- [ ] **Step 5: Implement, run tests**

Run: `cargo test -p finstack-ai-wasm` (native tests) and the crate's wasm build check (see how CI/scripts build it: `ls scripts/wasm_package`). Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add bindings/finstack-ai-wasm Cargo.lock
git commit -m "feat(wasm): HostMemoryStore host-backed memory store"
```

---

### Task 10: Final verification + docs cross-link

**Files:**
- Modify: `extensions/README.md` and `extensions/context/README.md` if they enumerate crates (check content first)
- Modify: `docs/planning/05-finstack-ai-future-capabilities-design-validation.md` — NO content edits; instead check whether any repo docs index links the old crate (`grep -rn "context-memory" docs/ README.md`) and update stale references.

- [ ] **Step 1: Stale-reference sweep**

Run: `grep -rn "finstack-ai-context-memory\|finstack_ai_context_memory" --include="*.md" --include="*.rs" --include="*.toml" --include="*.ipynb" . | grep -v target | grep -v .claude | grep -v docs/superpowers` — fix every hit (docs mentioning the old crate should now name `finstack-ai-memory`).

- [ ] **Step 2: Full workspace gate**

Run, checking each exit code:

```bash
cargo build --workspace --all-features
cargo test --workspace --all-features
cargo clippy --workspace --all-targets --all-features
cargo build -p finstack-ai-memory --target wasm32-unknown-unknown
```

Expected: all green.

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "docs: point memory references at finstack-ai-memory; final verification"
```

---

## Deferred (do NOT implement — documented for context)

- `MemoryCaptureMiddleware`: blocked on runtime committed middleware effects (`crates/finstack-ai-runtime/src/ports/middleware/port.rs:12-14` forbids externally-effectful middleware). The `MemoryExtractor` + idempotent `MemoryStore::put` from Task 6 are the reuse surface.
- `MemoryQuery::Embedding` + vector store impl; LLM-driven `MemoryExtractor`.
- `memory.consolidate` / `memory.profile` tooling; physical purge/retention jobs.
