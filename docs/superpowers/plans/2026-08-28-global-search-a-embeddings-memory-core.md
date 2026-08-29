# Plan A: embeddings crate and memory semantic core (in-process)

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development or superpowers:executing-plans, task-by-task. Steps use checkbox syntax.

**Goal:** `extensions/embeddings/finstack-ai-embeddings` exists (vector type,
`TextEmbedder` contract, deterministic `HashEmbedder`); `finstack-ai-memory`
gains `MemoryQuery::Embedding`, `MatchEvidence::Semantic`, the extended
store trait with its in-process implementation, and
`reconcile_memory_embeddings`. Sqlite compiles behind an honest stub
(implemented in Plan B); provider/toolset/observer get only compiler-forced
arm updates (functional work is Plan C).

**Architecture:** Layer 0 is shared machinery implementing no ports,
dependency-light and wasm-buildable. Memory embeddings are **derived index
data**: pending is an anti-join (live records with no row for the space),
`store_embedding` carries a source-digest staleness guard, and maintenance
is best-effort — it never fails a write. Spec §4–§6.1.

**Tech Stack:** Rust; `finstack-ai-runtime` (`PortObject`/`PortFuture`),
`thiserror`; existing `finstack-ai-memory` deps plus the new
`finstack-ai-embeddings` workspace dependency.

**Spec:** `docs/superpowers/specs/2026-08-28-global-search-design.md`

## Global Constraints

- Standard extension lint header (`#![forbid(unsafe_code)]`, deny
  `unwrap`/`expect`/`panic`/`unreachable`, `#![warn(missing_docs)]`);
  `[lints] workspace = true`; `publish = false`.
- `finstack-ai-embeddings` takes **no native or optional dependencies** and
  must build for `wasm32-unknown-unknown` (deny.toml scans that graph with
  all features on).
- Verify per task: `cargo nextest run -p <crate> --locked` and
  `cargo clippy -p <crate> --all-targets --locked -- -D warnings`; for
  `finstack-ai-memory` run both with and without `--features sqlite`.
  Never run workspace-wide tests.
- Any task changing a public surface or a stable error reason runs
  `mise run write-public-api` (regenerates cargo-public-api dumps **and**
  `fixtures/compatibility/error-codes/v1/codes.json`) and commits the
  updated baselines with the task.
- One commit per task; short imperative subject; do not push.

---

### Task A1: embeddings family + crate scaffold

**Files:**
- Modify: `Cargo.toml` (root: add `extensions/embeddings/finstack-ai-embeddings`
  to `members`; add crate to `[workspace.dependencies]` mirroring existing
  extension entries)
- Create: `extensions/embeddings/README.md` (family charter: embedding
  primitives and embedder implementations; trusted native, same class as
  the rest of `extensions/`; the shared crate stays dependency-free and
  wasm-buildable, network impls live in sibling crates)
- Create: `extensions/embeddings/finstack-ai-embeddings/{Cargo.toml,README.md,src/lib.rs}`
  (lint header; empty module stubs `vector`, `embedder`)

- [ ] **Step 1:** Failing check — `cargo check -p finstack-ai-embeddings --locked` fails (crate absent).
- [ ] **Step 2:** Create the scaffold and registrations.
- [ ] **Step 3:** `cargo check -p finstack-ai-embeddings --locked` and
  `cargo check -p finstack-ai-embeddings --locked --target wasm32-unknown-unknown`
  pass; clippy clean; `mise run check-layering` passes with the new family.
- [ ] **Step 4:** Commit `Add embeddings extension family scaffold`

### Task A2: `EmbeddingVector` and vector math

**Files:**
- Create: `extensions/embeddings/finstack-ai-embeddings/src/vector.rs`
- Modify: `src/lib.rs`
- Test: `src/tests.rs` (`#[cfg(test)] mod tests;`)

**Interfaces:**

```rust
pub const EMBEDDING_MAX_DIMENSIONS: usize = 4096;
pub struct EmbeddingVector(/* Arc<[f32]> */);
impl EmbeddingVector {
    pub fn try_new(components: Vec<f32>) -> Result<Self, EmbedError>;
        // rejects: empty; > EMBEDDING_MAX_DIMENSIONS; any non-finite; all-zero
        // (norm 0 makes unit normalization undefined)
    pub fn dimensions(&self) -> usize;
    pub fn as_slice(&self) -> &[f32];
    pub fn unit_normalized(&self) -> Self;
    pub fn dot(&self, other: &Self) -> Option<f32>;  // None on dims mismatch
}
// PartialEq/Eq/Hash implemented manually over f32::to_bits (sound once
// non-finite values are banned) — this is what lets MemoryQuery keep Eq.
pub fn truncate_to_bytes(text: &str, max_bytes: usize) -> &str; // char-boundary
```

- [ ] **Step 1:** Failing tests: rejection cases (empty / oversized /
  NaN / inf / all-zero); bit-pattern equality and hash agreement;
  `unit_normalized` yields norm ≈ 1 and is idempotent; `dot` on
  mismatched dims is `None`; truncation never splits a char and is a
  no-op under the limit.
- [ ] **Step 2:** Implement (`clippy::float_cmp`-clean: comparisons via
  bits, math via accumulation in input order for determinism).
- [ ] **Step 3:** Tests green; clippy (native + wasm32 check).
- [ ] **Step 4:** Commit `Add validated embedding vector type`

### Task A3: `TextEmbedder` contract + `HashEmbedder`

**Files:**
- Create: `src/embedder.rs`
- Modify: `src/lib.rs`, `src/tests.rs`

**Interfaces:**

```rust
pub enum EmbedError {  // thiserror; stable snake_case reasons
    InvalidInput { reason: &'static str },
    Unavailable { message: Arc<str> },
}
pub struct TextEmbedderDescriptor {
    pub embedder_id: Arc<str>,   // stable identity: model + revision + dims
    pub dimensions: usize,
    pub max_input_bytes: usize,
}
pub trait TextEmbedder: PortObject {
    fn descriptor(&self) -> TextEmbedderDescriptor;
    fn embed(&self, texts: Vec<Arc<str>>)
        -> PortFuture<Result<Vec<EmbeddingVector>, EmbedError>>; // batch; output order = input order
}
pub struct HashEmbedder { /* dimensions */ }
impl HashEmbedder {
    pub fn try_new(dimensions: usize) -> Result<Self, EmbedError>;
    // embedder_id: "embed.hash-v1.<dims>"; deterministic token-hash
    // bag-of-words; tokenless input hashes the trimmed text whole; empty
    // input is InvalidInput. Reference/test impl only — never a production default.
}
```

- [ ] **Step 1:** Failing tests: same text → identical vector across calls;
  batch preserves order; distinct texts differ; declared dims honored;
  empty input errors; punctuation-only input embeds (whole-text fallback);
  descriptor id stable and encodes dims.
- [ ] **Step 2:** Implement.
- [ ] **Step 3:** Green; clippy; wasm32 check; `mise run write-public-api`
  (new crate baseline + any new stable codes).
- [ ] **Step 4:** Commit `Add text embedder contract with hash reference`

### Task A4: memory query variant, evidence, limits, helpers

**Files:**
- Modify: `extensions/context/finstack-ai-memory/Cargo.toml`
  (`finstack-ai-embeddings = { workspace = true }`)
- Modify: `src/store/mod.rs` — `MemoryQuery::Embedding { embedder_id: Arc<str>, vector: EmbeddingVector }`;
  `MatchEvidence::Semantic` and mark `MatchEvidence` `#[non_exhaustive]`;
  `MemoryStoreLimits` + defaults `MAX_MEMORY_EMBEDDING_DIMENSIONS: usize = 4096`,
  `MAX_MEMORY_EMBEDDING_SPACES: usize = 4`; helpers
  `embedding_source_text(&MemoryRecord) -> String` (preview + inline body +
  keywords — exactly the FTS fields; blob bodies contribute preview +
  keywords only), `embedding_source_digest(&str) -> Result<Digest, _>`
  (domain `"memory-embed-source"`, version 1),
  `similarity_score(dot: f32) -> u32` (clamp to [−1, 1] → `0..=1_000_000`)
- Modify (compiler-forced arms only): `src/store/in_process.rs`
  (`validate_query`: embedder-id bounds ≤ 256 bytes / non-empty / no NUL,
  dims ≤ limit; `match_record`/search arm: stub
  `Err(InvalidRequest { reason: "memory_embeddings_unsupported" })` until A5),
  `src/store/sqlite/queries.rs` (validate arm; dispatch arm: same stub until
  Plan B), `src/provider.rs` (`tier_of`: `Semantic => 2`),
  `src/toolset.rs` (`matched_str`: `"semantic"`)
- Test: `src/tests/store.rs`, `src/tests/record.rs` (extend)

- [ ] **Step 1:** Failing tests: query validation (bad embedder id, oversize
  dims, valid accepted); `matched_str`/tier mappings for `Semantic`;
  `embedding_source_text` canonical form for inline and blob bodies
  (byte-exact fixtures); `similarity_score` monotonic, clamped, endpoints
  `0`/`500_000`/`1_000_000`.
- [ ] **Step 2:** Implement; keep every in-crate `match` exhaustive (no `_`
  arms — the compiler must flag future variants).
- [ ] **Step 3:** Green + clippy for both feature sets;
  `mise run write-public-api` (memory baseline + new error reasons).
- [ ] **Step 4:** Commit `Add embedding query and semantic evidence to memory model`

### Task A5: store trait extension, in-process index, reconciler

**Files:**
- Modify: `src/store/mod.rs`:

```rust
pub struct EmbeddingSource { pub scope: MemoryScope, pub id: MemoryId,
                             pub text: Arc<str>, pub source_digest: Digest }
// MemoryStore additions, all default-implemented (object safety + host
// adapters preserved):
fn pending_embedding_sources(&self, embedder_id: Arc<str>, limit: usize)
    -> PortFuture<Result<Vec<EmbeddingSource>, MemoryStoreError>>;  // default Ok(vec![])
fn store_embedding(&self, embedder_id: Arc<str>, scope: MemoryScope, id: MemoryId,
    source_digest: Digest, vector: EmbeddingVector)
    -> PortFuture<Result<(), MemoryStoreError>>;
    // default Err(InvalidRequest { reason: "memory_embeddings_unsupported" })
fn forget_embedding_space(&self, embedder_id: Arc<str>)
    -> PortFuture<Result<(), MemoryStoreError>>;                    // default Ok(())
pub async fn reconcile_memory_embeddings(store: &dyn MemoryStore,
    embedder: &dyn TextEmbedder, limit: usize) -> Result<usize, MemoryStoreError>;
    // pending → truncate each text to descriptor.max_input_bytes → batch
    // embed → store with digest guard; embed failure maps to
    // Unavailable("memory_embedder_failed"); returns applied count
```

- Modify: `src/store/in_process.rs` — `MemoryState` gains
  `embeddings: BTreeMap<(MemoryScope, MemoryId, Arc<str>), StoredEmbedding>`
  (`StoredEmbedding { unit_vector, dimensions, source_digest }`); implement
  the three methods (pending = live records lacking a row for the space;
  digest guard silently no-ops on mismatch; first vector fixes a space's
  dims, mismatches are `InvalidRequest`; new space beyond
  `max_embedding_spaces` is `CapacityExceeded { resource: "embedding_spaces" }`);
  replace the A4 search stub with real ranking (dot over unit-normalized
  vectors, `similarity_score`, descending with id tie-break,
  `MatchEvidence::Semantic`); evict rows in `forget`, `correct`, and the
  expiry cleanup path.
- Test: `src/tests/store.rs` (extend)

- [ ] **Step 1:** Failing tests: pending lists only live-unembedded records
  for the named space; digest-guard no-op after an interleaved `correct`;
  per-space dims fixed; space cap enforced; semantic search ranks by
  similarity with deterministic tie-break, honors scope isolation and limit,
  excludes tombstoned/superseded/expired, returns empty for an unknown
  space; eviction on forget/correct/expiry; `reconcile_memory_embeddings`
  with `HashEmbedder` drains, is idempotent on re-run, resumes after a
  partial failure; trait defaults behave (a minimal custom store reports no
  pending and rejects `store_embedding`).
- [ ] **Step 2:** Implement.
- [ ] **Step 3:** Green + clippy both feature sets; wasm32 check of the
  memory crate still passes (in-process path is pure Rust);
  `mise run write-public-api`.
- [ ] **Step 4:** Commit `Extend memory store with eventual embedding index`
