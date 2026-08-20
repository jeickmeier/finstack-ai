# Memory extension design: `finstack-ai-memory`

- **Date:** 2026-08-19
- **Status:** Approved for planning
- **Source of truth for the shape:** `docs/planning/05-finstack-ai-future-capabilities-design-validation.md`, section 7 ("Memory system"), especially 7.2 (reference composition).

## 1. Overview

Finish memory as a **composition**, not another recall crate. Today `extensions/context/finstack-ai-context-memory` implements only the recall half of the section-7.2 composition: a keyword/exact-id `ContextProvider` over an in-process index, with a programmatic `stage()` write. This design replaces it with one cohesive extension crate that implements the full composition:

```text
finstack-ai-memory (extensions/context/finstack-ai-memory)
  MemoryStore (trait) ── InProcessMemoryStore, SqliteMemoryStore (feature "sqlite")
  MemoryContextProvider  — automatic budgeted recall (ContextProvider port)
  MemoryToolset          — remember, search_memory, forget, correct, inspect (Toolset port)
  MemoryObserver         — eventual asynchronous capture (Observer port)
  MemoryExtractor (trait)— shared candidate-memory extraction
  [deferred] MemoryCaptureMiddleware — blocked on runtime committed middleware effects
```

`finstack-ai-context-memory` is **removed outright** (pre-alpha; no deprecation shim). Its provider logic and `InProcessArtifactStore` move into the new crate; the Python bindings' import is updated.

## 2. Goals

1. A production-shaped `MemoryStore` trait with the full section-7.5 metadata model (scopes, provenance, confidence, supersession, tombstones, retention, sensitivity).
2. Two store implementations: in-process (reference/tests, wasm32-compatible) and SQLite with FTS5/BM25 full-text retrieval (feature-gated).
3. Recall (`MemoryContextProvider`) with the section-7.6 cache-stability rules.
4. Model-driven memory management via `MemoryToolset`, capability-gated per section 7.2, idempotent via tool effect IDs.
5. Durable-eventual capture via `MemoryObserver` with a pluggable `MemoryExtractor`.
6. Python exposure as a native linked extension (same pattern as `DocumentToolset`).
7. WASM exposure via a host-backed store (`HostMemoryStore`) so JS can supply IndexedDB or a remote service.

## 3. Non-goals

- **Embedding/vector retrieval.** Explicit future work. The `MemoryQuery` type and `MemoryStore` trait are shaped so an `Embedding` variant and a vector-store implementation can be added without breaking changes, but no embedding model, ANN index, or network fetch ships in this design.
- **`MemoryCaptureMiddleware`.** The runtime `Middleware` port states middleware is never a committed effect and must be pure with respect to external state (recovery re-runs the chain). Until the runtime grows committed middleware effects, the durable-write paths are the `remember` tool (already a committed, idempotent tool effect) and the observer. The middleware is documented as a deferred thin adapter over `MemoryExtractor` + idempotent `MemoryStore::put`.
- **Model-driven extraction.** The default `MemoryExtractor` is conservative and rule-based. LLM-based extraction (a model effect) is future work behind the same trait.
- **Kernel/runtime changes.** None. The composition uses only existing ports: `ContextProvider`, `Toolset`, `Observer`, `ArtifactStore`.
- **Consolidation tooling** (`memory.consolidate` flows such as merge/summarize jobs). The capability name is reserved in `MemoryPolicy`, but no consolidation implementation ships.

## 4. Crate layout and packaging

Memory stays grouped with the other context-providing batteries under `extensions/context/`.

```text
extensions/context/finstack-ai-memory/
  Cargo.toml            # features: default = [], "sqlite"
  README.md
  src/
    lib.rs              # public exports, crate lints (match existing extension lint set)
    record.rs           # MemoryRecord, MemoryScope, MemoryProvenance, retention, tombstones
    store/
      mod.rs            # MemoryStore trait, MemoryQuery, MemoryStoreError
      in_process.rs     # InProcessMemoryStore (+ InProcessArtifactStore moved from old crate)
      sqlite.rs         # SqliteMemoryStore (cfg(feature = "sqlite"))
    provider.rs         # MemoryContextProvider
    toolset.rs          # MemoryToolset + MemoryPolicy
    observer.rs         # MemoryObserver
    extract.rs          # MemoryExtractor trait + RuleBasedExtractor
    tests/              # unit + integration tests
```

One crate, one version, one review surface — matching the doc's `MemoryExtension` framing. The crate follows the existing extension conventions: `#![forbid(unsafe_code)]`, deny `unwrap`/`expect`/`panic`, `missing_docs` warnings, `thiserror` error enums with stable non-secret reason strings.

### Removal of `finstack-ai-context-memory`

- Delete `extensions/context/finstack-ai-context-memory/`.
- Move `InProcessArtifactStore` into `finstack-ai-memory::store::in_process` and re-export at crate root (the Python binding crate links it directly today at `bindings/finstack-ai-python/src/agent.rs:14`).
- Update workspace `Cargo.toml` members and every dependent (`bindings/finstack-ai-python`, any examples/tests) to the new crate.
- The provider's `ComponentId` stays `finstack.context.memory` (recall behavior is compatible); the crate `Version` bumps minor.

## 5. Record model (`record.rs`)

```rust
pub struct MemoryScope {
    pub tenant: Arc<str>,            // required, validated non-empty / no NUL
    pub user: Option<Arc<str>>,
    pub agent: Option<Arc<str>>,
    pub workspace: Option<Arc<str>>,
}

pub struct MemoryProvenance {
    pub source_session: Option<SessionId>,
    pub source_run: Option<RunId>,
    pub source_ref: Option<Arc<str>>,     // message/event reference
    pub extraction: ExtractionMethod,     // Explicit | ToolWrite | ObserverCapture | Imported
    pub confidence: Confidence,           // bounded 0..=100 integer, no floats
}

pub struct MemoryRecord {
    pub id: MemoryId,                     // stable, caller-supplied or derived
    pub scope: MemoryScope,
    pub keywords: Arc<[Arc<str>]>,
    pub body: MemoryBody,                 // Inline(Arc<str>) | Blob(ArtifactRef)
    pub preview: Arc<str>,                // bounded (256 chars), always present
    pub sensitivity: Sensitivity,
    pub provenance: MemoryProvenance,
    pub created_at: Timestamp,
    pub last_confirmed_at: Timestamp,
    pub supersedes: Option<MemoryId>,
    pub superseded_by: Option<MemoryId>,  // set by correct()
    pub retention: RetentionPolicy,       // KeepUntilDeleted | ExpireAfter(Duration)
    pub tombstoned: bool,
}
```

Rules:

- **Bodies:** inline up to a configured threshold (default 4 KiB); larger bodies are staged through the injected `ArtifactStore` and referenced by `ArtifactRef` (`BlobRef` inside), reusing `stage_required_artifact`. The store never persists large payloads itself.
- **Deletion is a tombstone.** `forget` marks `tombstoned = true`; tombstoned records are excluded from every recall and search path but remain inspectable via `inspect` under `memory.manage`. Physical purging is a store-maintenance concern out of scope here.
- **Correction is supersession.** `correct` writes a new record with `supersedes = old_id` and sets `superseded_by` on the old record. Superseded records are excluded from recall/search like tombstones.
- **Timestamps** come from the injected clock/context (never ambient `now()` inside deterministic paths), matching runtime conventions.

## 6. `MemoryStore` trait (`store/mod.rs`)

Object-safe, `Send + Sync`, `PortFuture`-returning — the same style as runtime ports so implementations are binding-friendly.

```rust
pub trait MemoryStore: Send + Sync {
    fn put(&self, idempotency_key: Arc<str>, record: MemoryRecord)
        -> PortFuture<Result<PutOutcome, MemoryStoreError>>;   // Inserted | AlreadyApplied
    fn get(&self, scope: &MemoryScope, id: &MemoryId)
        -> PortFuture<Result<Option<MemoryRecord>, MemoryStoreError>>;
    fn search(&self, scope: &MemoryScope, query: MemoryQuery, limit: usize)
        -> PortFuture<Result<Vec<MemoryHit>, MemoryStoreError>>;
    fn forget(&self, idempotency_key: Arc<str>, scope: &MemoryScope, id: &MemoryId)
        -> PortFuture<Result<(), MemoryStoreError>>;
    fn correct(&self, idempotency_key: Arc<str>, scope: &MemoryScope,
               old: &MemoryId, replacement: MemoryRecord)
        -> PortFuture<Result<(), MemoryStoreError>>;
    fn list(&self, scope: &MemoryScope, page: MemoryPage)
        -> PortFuture<Result<MemoryListing, MemoryStoreError>>;
}

#[non_exhaustive]
pub enum MemoryQuery {
    ExactId(MemoryId),
    Keywords(Arc<[Arc<str>]>),
    FullText(Arc<str>),
    // future: Embedding(EmbeddingQuery) — added without breaking the trait
}
```

- **Idempotency is a store contract**, not a caller courtesy: every mutating operation takes an idempotency key. Replaying the same key returns `AlreadyApplied` without duplicating. This is what makes tool-effect replay, observer redelivery, and (later) middleware re-runs safe.
- `MemoryHit` = record + rank score + which query mode matched (retrieval evidence, §7.5 "explainable retrieval").
- **Scope enforcement:** every read/write takes a `MemoryScope`; a store must return only records whose scope is compatible (tenant must match exactly; user/agent/workspace filter when present). The model can never widen scope — scope comes from the call context, not tool arguments.

### 6.1 `InProcessMemoryStore`

Mutex-guarded map, preserving current matching semantics (exact id, case-insensitive keyword), plus a naive substring scorer for `FullText`. Compiles on wasm32. Used by tests, examples, and as the default when no persistence is configured. Idempotency keys held in a bounded set.

### 6.2 `SqliteMemoryStore` (feature `sqlite`)

Follows the patterns in `extensions/stores/finstack-ai-store-sqlite` (rusqlite, worker/connection handling, schema module, explicit migrations table).

Schema sketch:

```sql
memory_records(id, tenant, user, agent, workspace, body_inline, blob_ref_json,
               preview, sensitivity, keywords_json, provenance_json,
               created_at, last_confirmed_at, supersedes, superseded_by,
               retention_json, tombstoned)
memory_fts    -- FTS5 external-content table over (preview, body_inline, keywords)
memory_idempotency(key PRIMARY KEY, applied_at)
```

- `FullText` queries use FTS5 with BM25 ranking; `Keywords` uses FTS5 term queries; `ExactId` is a primary-key lookup.
- Tombstoned/superseded rows are filtered in SQL, and the FTS index is updated on tombstone/supersede so they cannot rank.
- Mutations run in a transaction that inserts the idempotency key first; a conflict short-circuits to `AlreadyApplied`.

## 7. `MemoryContextProvider` (`provider.rs`)

The existing provider re-seated on `Arc<dyn MemoryStore>`; construction takes store, tenant scope (extended to full `MemoryScope`), and a `RecallConfig`.

Recall path per §7.3: derive query text from `ContextRequest.user_input` → `search` with `Keywords`/`FullText` → rank → dedupe (drop superseded/tombstoned; collapse duplicates by id) → emit `ContextItem`s with provenance (`source_id = "finstack.context.memory"`, `source_ref = memory id`, `external = true`, `ContextAuthority::Untrusted`) and token estimates → apply the committed budget exactly as today (`Reject` vs `TruncateWithDiagnostic`).

Cache stability per §7.6:

- **Tiered ranking, stable order:** hits are bucketed into rank tiers; within a tier, order is by memory id (bytewise), so equal inputs always produce identical contributions.
- **Bounded stable prefix:** `RecallConfig.stable_prefix` pins up to N top-tier memories that, once recalled in a session, keep their order across turns.
- **`cache_key`:** the contribution's cache key is a digest over the ordered (memory id, last_confirmed_at) pairs, so unchanged recall reuses provider prompt cache.

The provider never mutates conversation history and never writes to the store. The old public `stage()` method is removed; writes go through the store, toolset, or observer.

## 8. `MemoryToolset` (`toolset.rs`)

Implements the runtime `Toolset` port. Construction: `MemoryToolset::try_new(store, artifact_store, scope, policy)`.

### 8.1 Capability gating

```rust
pub struct MemoryPolicy {
    pub read: bool,         // memory.read     → search_memory, inspect
    pub write: bool,        // memory.write    → remember
    pub manage: bool,       // memory.manage   → forget, correct
    pub consolidate: bool,  // memory.consolidate — reserved, no tools in this design
    pub profile: bool,      // memory.profile    — reserved, no tools in this design
}
```

`tools()` returns only the specs the policy enables, so applications can expose recall without autonomous writes (provider only, or `read`-only toolset), exactly as §7.2 requires.

### 8.2 Tools

| Tool | Capability | Behavior |
|---|---|---|
| `remember` | write | Validate args (id optional — derived from content digest when absent; keywords; body; sensitivity), stage large bodies via `ArtifactStore`, `put` with the **tool effect ID as idempotency key**. Returns the stored id. |
| `search_memory` | read | `MemoryQuery` from args (`id`, `keywords`, or `text`), bounded `limit`, returns hits with previews, ids, scores, and match evidence. Never returns tombstoned/superseded records. |
| `inspect` | read | Fetch one record's full metadata (provenance, timestamps, supersession links). Body returned inline only if small; otherwise the `ArtifactRef` is referenced. |
| `forget` | manage | Tombstone by id, idempotent via tool effect ID. |
| `correct` | manage | Supersede: new record linked to old, idempotent via tool effect ID. |

- Scope always comes from the toolset's configured `MemoryScope` + the call context's tenant, never from model-supplied arguments.
- Argument schemas are data-only `ToolSpec`s validated by the runtime before `call` (schemas already checked per the port contract).
- All tools emit a single result event via `ToolEventStream`; no streaming needed.
- `reconcile` for `remember`/`forget`/`correct`: re-issue the store call with the same idempotency key and map `AlreadyApplied` to completed.

## 9. `MemoryObserver` and `MemoryExtractor` (`observer.rs`, `extract.rs`)

Eventual asynchronous capture per §7.4: appropriate when memory loss does not invalidate the run.

```rust
pub trait MemoryExtractor: Send + Sync {
    fn extract(&self, events: &[RunEvent]) -> Vec<CandidateMemory>;
}
```

- `MemoryObserver` implements the runtime `Observer` port. It consumes committed `RunEvent` batches, feeds terminal-run events through the extractor, and `put`s each candidate with idempotency key `capture:{run_id}:{event_id}:{candidate_index}` and `ExtractionMethod::ObserverCapture`.
- Redelivered batches are safe (idempotent keys). Observer failure never changes run results (port contract) — errors are returned to the runtime's observer isolation, not propagated.
- `RuleBasedExtractor` (default): conservative, opt-in patterns only (e.g., explicitly tagged remember-this markers in application output). Shipping a low-recall default is deliberate; aggressive extraction is an application choice via a custom extractor. LLM-based extraction is future work.
- The extractor is **shared infrastructure**: the deferred `MemoryCaptureMiddleware` will wrap the same `MemoryExtractor` + idempotent `put` when the runtime supports committed middleware effects. This design keeps that adapter thin.

## 10. Python exposure (`bindings/finstack-ai-python`)

Pattern: native linked extension, like `DocumentToolset`.

- New pyclass `MemoryExtension` constructed with `MemoryConfig`: `in_process()` or `sqlite(path)`, plus scope fields and `MemoryPolicy` flags.
- The extension exposes handles usable in the existing agent factory params: its provider via `context_providers=`, toolset via `toolsets=`, observer via `observers=`. Internally these wrap the native components; no Python callbacks in the hot path.
- Update the existing import of `finstack_ai_context_memory::InProcessArtifactStore` to the new crate.
- Add a notebook example under `examples/python-notebooks/` demonstrating: remember via tool, recall on next run, forget, and SQLite persistence across sessions.
- Python-implemented custom stores are out of scope (the coarse `collect` callback path already exists for fully custom providers per §7.7).

## 11. WASM exposure (`bindings/finstack-ai-wasm`)

Pattern: host-backed port, like `host_store.rs` / `host_context.rs`.

- New `HostMemoryStore`: implements `MemoryStore` by delegating each operation to JS host callbacks (`memory_put`, `memory_get`, `memory_search`, `memory_forget`, `memory_correct`, `memory_list`) with JSON-normalized records, using the existing `HostFailure`/`parse_host_json` machinery. JS owns persistence — IndexedDB or a remote service — per §7.7.
- The native `MemoryContextProvider`, `MemoryToolset`, and `MemoryObserver` run inside the wasm module over `HostMemoryStore` (or over `InProcessMemoryStore`, which compiles to wasm32, when no host store is wired).
- Large memory attachments continue through `BlobRef`/`ArtifactStore` (already host-backed in wasm).
- `SqliteMemoryStore` is native-only (`cfg` excluded from wasm32).

## 12. Security and scoping summary

- Tenant scope is validated at construction and re-checked against the committed call context on every operation (as the current provider does).
- The model cannot name or widen scopes: scope is configuration + call context only.
- Recall items carry `ContextAuthority::Untrusted` and per-record `Sensitivity`; the runtime's budget and sensitivity handling apply unchanged.
- Stable, non-secret error reasons throughout (`memory_configuration_invalid`, `memory_store_unavailable`, `memory_scope_mismatch`, `memory_not_found`, …).

## 13. Testing

- **Record/store unit tests:** scope filtering, tombstone/supersession exclusion, idempotent `put`/`forget`/`correct` (`AlreadyApplied`), inline-vs-blob body threshold.
- **SQLite integration tests:** schema migration, FTS5/BM25 ranking order, FTS updates on tombstone/supersede, idempotency-key conflict handling, persistence across reopen.
- **Provider tests:** deterministic ordering (tiers, id tiebreak), stable prefix behavior across repeated collects, `cache_key` stability/instability, budget `Reject` and `Truncate` paths, tenant mismatch rejection (port existing tests forward).
- **Toolset tests:** policy gating of `tools()`, per-tool argument validation, effect-ID idempotency across replay, `reconcile` behavior.
- **Observer tests:** idempotent redelivery, extractor isolation, failure containment.
- **Binding tests:** Python smoke test through the agent factory (remember → recall); wasm test of `HostMemoryStore` round-trip via scripted host callbacks (existing fixture patterns).

## 14. Future work (explicitly deferred)

1. **Vector/embedding retrieval:** `MemoryQuery::Embedding` variant + a vector store implementation; embedding generation as a committed model effect. Desired; the trait is shaped for it.
2. **`MemoryCaptureMiddleware`:** thin adapter over `MemoryExtractor` + idempotent `put`, blocked on runtime committed middleware effects.
3. **LLM-driven extraction:** a `MemoryExtractor` implementation backed by a model effect.
4. **Consolidation and profile tooling** (`memory.consolidate`, `memory.profile`): merge/summarize/decay jobs and profile-memory management tools.
5. **Physical purge/retention enforcement jobs** for tombstoned and expired records.
