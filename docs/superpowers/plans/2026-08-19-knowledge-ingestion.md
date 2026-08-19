# Knowledge Ingestion & Retrieval Extension Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Embedding/keyword/hybrid search, an entity-relation knowledge graph with community summaries, and a source catalog — fed by an extensible ingest pipeline over the document parser, queried by a toolset and a context provider, with Python and WASM parity.

**Architecture:** Five new runtime port families (`EmbeddingModel`, `ChunkIndex`, `GraphStore`, `SourceCatalog`, `Reranker`, plus the pure `FusionStrategy`) with implementations in `extensions/`: an engine crate (`finstack-ai-ingest`) that runs parse → chunk → [embed ∥ graph-extract] → index and hosts the `Model`-port-backed components (`ModelGraphExtractor`, `ModelReranker`, `CommunityBuilder`), memory and SQLite store backends, a query toolset, a context provider, and a `BeforeModel` feeder middleware.

**Tech Stack:** Rust (edition 2024), existing `finstack-ai-runtime` port machinery (`PortObject`/`PortFuture`/`PortStream`), `finstack-ai-tools-document::parser` (document plan Task 2), `rusqlite` (bundled, FTS5 verified available), PyO3 binding, wasm-bindgen binding.

**Spec:** `docs/superpowers/specs/2026-08-19-knowledge-ingestion-design.md`

## Global Constraints

- **Prerequisite:** document plan Tasks 1–2 must be complete (`fixtures/documents/` corpus and `finstack-ai-tools-document` with its `parser` module). Nothing later in that plan is needed.
- Workspace lints/version/edition inherited: every new crate uses `.workspace = true` keys like `extensions/toolsets/finstack-ai-tools-calculator/Cargo.toml`.
- **Evolution policy (spec decision 7a) applies to every public type in `ports/embedding` and `ports/knowledge`:** `#[non_exhaustive]` on every struct and enum, constructor/builder functions for every struct, wildcard match arms in all consuming code, and any future port-trait method must carry a default implementation returning `Unsupported`.
- Stable identity strings exactly as spec'd: toolset id `finstack.tools.knowledge`; tools `knowledge_search`, `graph_query`, `list_sources`, `knowledge_overview`; middleware component id `finstack.middleware.knowledge-ingest`; error codes `knowledge_invalid_arguments`, `knowledge_store_unavailable`, `knowledge_embedding_failed`, `knowledge_query_failed`, `knowledge_rerank_failed` (lower snake_case values in `pub const` SCREAMING_SNAKE names).
- Default collection id is `"default"`. Collection slugs: 1–64 chars of `[a-z0-9-]`, no leading/trailing `-`.
- Defaults: `top_k` 8 (max 32), `expand` 0 (max 2), graph `depth` 1 (max 3), `max_chunks_per_document` 2048, `max_concurrent_embed_batches` 4, `max_concurrent_extract_calls` 2.
- No additions to `FORBIDDEN_WASM` in `scripts/wasm_package/check.py`; the wasm package check must stay green. The memory backend, engine crate, toolset, and context provider must compile for wasm32; the sqlite backend and provider embedders are native-only.
- Scoring algorithms (BM25, cosine, RRF) are **backend-private**; no port contract may name them.
- Run full verification before claiming any task complete: `cargo test -p <crate>` for the touched crate, and `cargo clippy --workspace --all-targets` at the end of each task.
- Commit after every task with the step's exact `git add` list — the working tree contains unrelated in-flight changes; NEVER `git add -A` or `git add .`.
- Test doubles are copied per crate, never shared across crates (document-plan precedent).

---

### Task 1: `ports/embedding` — EmbeddingModel port

**Files:**
- Create: `crates/finstack-ai-runtime/src/ports/embedding/mod.rs`
- Create: `crates/finstack-ai-runtime/src/ports/embedding/types.rs`
- Create: `crates/finstack-ai-runtime/src/ports/embedding/error.rs`
- Create: `crates/finstack-ai-runtime/src/ports/embedding/tests.rs`
- Modify: `crates/finstack-ai-runtime/src/ports/mod.rs` (add `pub(crate) mod embedding;`)
- Modify: `crates/finstack-ai-runtime/src/lib.rs` (re-export the new public items where the other port items are re-exported — find the `pub use ports::…` block and follow its grouping)

**Interfaces:**
- Produces (used by every later task):
  - `trait EmbeddingModel: PortObject { fn descriptor(&self) -> EmbeddingModelDescriptor; fn embed(&self, ctx: EmbeddingCallContext, batch: EmbeddingBatch) -> PortFuture<Result<EmbeddingOutput, EmbeddingError>>; }`
  - `EmbeddingModelDescriptor::new(model_id: &str, dimensions: u32, max_batch: u32, max_input_tokens: u32) -> Result<Self, EmbeddingError>` (rejects zero dimensions/batch)
  - `EmbeddingInputKind::{Document, Query}`
  - `EmbeddingBatch::new(inputs: Vec<Arc<str>>, kind: EmbeddingInputKind) -> Result<Self, EmbeddingError>` (rejects empty batch), `.with_options(Metadata)`; getters `inputs()`, `kind()`, `options()`
  - `EmbeddingOutput::new(vectors: Vec<Vec<f32>>) -> Self`, `.with_usage(input_tokens: u64)`; getters `vectors()`, `input_tokens()`
  - `EmbeddingCallContext::detached(deadline: Option<Timestamp>) -> Self` (pipeline use, no run) and `EmbeddingCallContext::for_run(run: RunCallContext) -> Self`
  - `EmbeddingError` enum: `InvalidBatch { reason: &'static str }`, `Provider { code: Arc<str>, message: Arc<str> }`, `Deadline`, `Cancelled`, `Unsupported { what: &'static str }` — all `#[non_exhaustive]`, `thiserror::Error`

- [ ] **Step 1: Write failing tests**

`crates/finstack-ai-runtime/src/ports/embedding/tests.rs`:

```rust
use std::sync::Arc;

use finstack_ai_kernel::Metadata;

use super::error::EmbeddingError;
use super::types::{
    EmbeddingBatch, EmbeddingInputKind, EmbeddingModelDescriptor, EmbeddingOutput,
};

#[test]
fn descriptor_rejects_zero_dimensions() {
    assert!(matches!(
        EmbeddingModelDescriptor::new("test-model", 0, 16, 8192),
        Err(EmbeddingError::InvalidBatch { .. })
    ));
    let descriptor =
        EmbeddingModelDescriptor::new("test-model", 384, 16, 8192).expect("valid descriptor");
    assert_eq!(descriptor.dimensions(), 384);
    assert_eq!(descriptor.model_id(), "test-model");
}

#[test]
fn batch_rejects_empty_inputs() {
    assert!(matches!(
        EmbeddingBatch::new(Vec::new(), EmbeddingInputKind::Document),
        Err(EmbeddingError::InvalidBatch { .. })
    ));
}

#[test]
fn batch_carries_inputs_kind_and_options() {
    let batch = EmbeddingBatch::new(
        vec![Arc::from("first"), Arc::from("second")],
        EmbeddingInputKind::Query,
    )
    .expect("valid batch")
    .with_options(Metadata::empty());
    assert_eq!(batch.inputs().len(), 2);
    assert_eq!(batch.kind(), EmbeddingInputKind::Query);
}

#[test]
fn output_round_trips_vectors_and_usage() {
    let output = EmbeddingOutput::new(vec![vec![0.1, 0.2], vec![0.3, 0.4]]).with_usage(7);
    assert_eq!(output.vectors().len(), 2);
    assert_eq!(output.input_tokens(), Some(7));
}

#[test]
fn error_display_is_stable_and_non_secret() {
    let error = EmbeddingError::Provider {
        code: Arc::from("embedding_provider_failed"),
        message: Arc::from("upstream 500"),
    };
    assert!(error.to_string().contains("embedding_provider_failed"));
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p finstack-ai-runtime embedding`
Expected: FAIL — module unresolved.

- [ ] **Step 3: Implement**

`mod.rs` declares `pub(crate) mod error; pub(crate) mod types; mod tests (cfg(test))` plus the trait:

```rust
//! Embedding-model port: batch text-to-vector encoding.

pub(crate) mod error;
pub(crate) mod types;

#[cfg(test)]
mod tests;

use crate::{PortFuture, PortObject};

pub use error::EmbeddingError;
pub use types::{
    EmbeddingBatch, EmbeddingCallContext, EmbeddingInputKind, EmbeddingModelDescriptor,
    EmbeddingOutput,
};

/// Object-safe embedding-model port.
///
/// Implementors encode text batches into fixed-dimension vectors. Native
/// objects are `Send + Sync`; browser-WASM hosts stay local.
pub trait EmbeddingModel: PortObject {
    /// Immutable model identity and ceilings.
    fn descriptor(&self) -> EmbeddingModelDescriptor;

    /// Encode one bounded batch; one vector per input, in order.
    ///
    /// # Arguments
    ///
    /// * `ctx` - Deadline, cancellation, and (optional) run locator.
    /// * `batch` - Non-empty inputs plus input-kind hint and options.
    fn embed(
        &self,
        ctx: EmbeddingCallContext,
        batch: EmbeddingBatch,
    ) -> PortFuture<Result<EmbeddingOutput, EmbeddingError>>;
}
```

`types.rs`: implement the structs from the Interfaces block. Every struct `#[non_exhaustive]` (private fields + getters is equivalent and preferred here — use private fields with accessor methods so `#[non_exhaustive]` is not even needed for field additions; enums still get `#[non_exhaustive]`). `EmbeddingCallContext` holds `deadline: Option<Timestamp>`, `run: Option<RunCallContext>` — look at how `RunCallContext` is exported (grep `RunCallContext` in `crates/finstack-ai-runtime/src/lib.rs`) and reuse it; if it is not constructible outside the runtime, store it as `Option<RunCallContext>` anyway (the pipeline passes `None` via `detached`).

`error.rs`:

```rust
//! Stable, non-secret embedding errors.

use std::sync::Arc;

use thiserror::Error;

/// Embedding failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum EmbeddingError {
    /// Batch construction or validation failed.
    #[error("embedding_invalid_batch: {reason}")]
    InvalidBatch {
        /// Stable non-secret reason.
        reason: &'static str,
    },
    /// Provider-side failure with a stable code.
    #[error("{code}: {message}")]
    Provider {
        /// Stable non-secret code.
        code: Arc<str>,
        /// Bounded non-secret message.
        message: Arc<str>,
    },
    /// The call exceeded its deadline.
    #[error("embedding_deadline_exceeded")]
    Deadline,
    /// The call was cancelled.
    #[error("embedding_cancelled")]
    Cancelled,
    /// The implementation does not support the requested feature.
    #[error("embedding_unsupported: {what}")]
    Unsupported {
        /// Stable non-secret feature name.
        what: &'static str,
    },
}
```

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test -p finstack-ai-runtime embedding`
Expected: PASS.

- [ ] **Step 5: Check the public-API fixture**

Run the workspace's public-API check (grep the repo for `cargo-public-api` or check `docs/implementation/` for the command; the document plan names `fixtures/compatibility/public-rust-api/cargo-public-api/finstack-ai.txt`). If new re-exports from `finstack-ai` are surfaced, update the fixture file as an additive change in this commit. If the runtime crate is not part of that fixture, skip.

- [ ] **Step 6: Commit**

```bash
git add crates/finstack-ai-runtime/src/ports/mod.rs crates/finstack-ai-runtime/src/ports/embedding/ crates/finstack-ai-runtime/src/lib.rs
git commit -m "feat: add EmbeddingModel runtime port"
```

---

### Task 2: `ports/knowledge` — shared types and errors

**Files:**
- Create: `crates/finstack-ai-runtime/src/ports/knowledge/mod.rs` (module decls only for now)
- Create: `crates/finstack-ai-runtime/src/ports/knowledge/types.rs`
- Create: `crates/finstack-ai-runtime/src/ports/knowledge/error.rs`
- Create: `crates/finstack-ai-runtime/src/ports/knowledge/tests.rs`
- Modify: `crates/finstack-ai-runtime/src/ports/mod.rs`, `crates/finstack-ai-runtime/src/lib.rs` (same re-export pattern as Task 1)

**Interfaces:**
- Produces (used by every later task; all structs use private fields + getters + builder-style `with_*`; all enums `#[non_exhaustive]`):
  - `CollectionId::parse(&str) -> Result<Self, KnowledgeStoreError>`; `CollectionId::default_collection() -> Self` (the `"default"` slug); `as_str()`
  - `SourceId::new(Digest) -> Self`, `digest()`
  - `ChunkId::new(source: SourceId, index: u32)`, getters `source()`, `index()`
  - `ChunkRange::new(start: u32, end: u32) -> Result<Self, KnowledgeStoreError>` (start ≤ end), getters
  - `ChunkRecord::new(collection, chunk_id, chunk_digest: Digest, text: Arc<str>) -> Self`, `.with_vector(Arc<[f32]>)`, `.with_metadata(Metadata)`; getters `collection()`, `chunk_id()`, `chunk_digest()`, `vector()`, `text()`, `metadata()`; serde `Serialize`/`Deserialize` (export/import wire format)
  - `SearchMode::{Semantic { vector: Arc<[f32]> }, Keyword { text: Arc<str> }, Hybrid { vector: Arc<[f32]>, text: Arc<str> }}`
  - `ChunkQuery::new(collection, mode: SearchMode, top_k: u32) -> Result<Self, KnowledgeStoreError>` (top_k 1..=1024), `.with_filter(Metadata)`, `.with_options(Metadata)`; getters
  - `ChunkHit::new(record: ChunkRecord, score: f32)`; getters `record()`, `score()`
  - `ChunkIndexCapabilities::new()`, `.with_native_semantic(bool)`, `.with_native_keyword(bool)`, `.with_native_hybrid(bool)`; getters `native_semantic()`, `native_keyword()`, `native_hybrid()`
  - `SourceStatus::{Indexed, MetadataOnly { reason: Arc<str> }, PartialFailure}`
  - `SourceRecord::new(collection, source, format: Arc<str>, ingested_at: Timestamp, status: SourceStatus)`, `.with_name(Arc<str>)`, `.with_page_count(u32)`, `.with_chunks(chunk_count: u32, chunk_digests: Arc<[Digest]>)`, `.with_graph_template(id: Arc<str>, version: u32)`, `.with_metadata(Metadata)`; getters for every field; serde both ways
  - `SourceListQuery::new(collection)`, `.with_name_contains(Arc<str>)`, `.with_format(Arc<str>)`, `.with_status(SourceStatus)`, `.with_limit(u32)` (default 100, max 1024); getters
  - `SourceRecord` additionally: `.with_raw_stored(bool)` (default false) + `raw_stored()` getter — serialized like every other field
  - `StoredSource::new(media_type: Arc<str>, bytes: Arc<[u8]>)`; getters `media_type()`, `bytes()`
  - `Entity::new(name, entity_type, description: Arc<str>)`, `.with_embedding(Arc<[f32]>)`, `.with_source_chunks(Vec<ChunkId>)`, `.with_metadata(Metadata)`; getters
  - `Relation::new(from, to, relation_type, description: Arc<str>)`, `.with_source_chunks(Vec<ChunkId>)`, `.with_metadata(Metadata)`; getters
  - `GraphFragment::new(collection, source)`, `.with_entities(Vec<Entity>)`, `.with_relations(Vec<Relation>)`; getters
  - `EntityLookup::{Name { name: Arc<str>, entity_type: Option<Arc<str>> }, Semantic { vector: Arc<[f32]>, top_k: u32 }}`
  - `GraphQuery::new(collection, lookup: EntityLookup) -> Result<Self, KnowledgeStoreError>`, `.with_depth(u8)` (clamped to 3), `.with_relation_types(Vec<Arc<str>>)`, `.with_max_entities(u32)` (default 64), `.with_max_relations(u32)` (default 256); getters
  - `GraphResult::new(entities: Vec<Entity>, relations: Vec<Relation>)`; getters
  - `Community::new(collection, id: Arc<str>, level: u8, title: Arc<str>, summary: Arc<str>)`, `.with_entity_names(Vec<Arc<str>>)`, `.with_embedding(Arc<[f32]>)`, `.with_metadata(Metadata)`; getters; serde both ways
  - `CommunityLookup::{All, NamePrefix { prefix: Arc<str> }, Semantic { vector: Arc<[f32]>, top_k: u32 }}`
  - `CommunityQuery::new(collection, lookup: CommunityLookup)`, `.with_level(u8)`, `.with_limit(u32)` (default 16, max 128); getters
  - `GraphExportRecord::{Entity { source: SourceId, entity: Entity }, Relation { source: SourceId, relation: Relation }, Community(Community)}` — serde both ways
  - `KnowledgeStoreError` enum: `Unavailable { reason: Arc<str> }`, `InvalidQuery { reason: &'static str }`, `DimensionMismatch { expected: u32, got: u32 }`, `UnknownCollection`, `Unsupported { what: &'static str }`, `Internal { reason: Arc<str> }` — `#[non_exhaustive]`, `thiserror::Error`, stable snake_case display prefixes (`knowledge_store_unavailable`, `knowledge_invalid_query`, `knowledge_dimension_mismatch`, `knowledge_unknown_collection`, `knowledge_unsupported`, `knowledge_internal`)
  - Reserved metadata keys as constants: `pub const META_EMBEDDING_MODEL: &str = "finstack.embedding_model"`, `pub const META_TEMPLATE_ID: &str = "finstack.template_id"`, `pub const META_TEMPLATE_VERSION: &str = "finstack.template_version"`, `pub const META_DOC_NAME: &str = "finstack.doc_name"`, `pub const META_DOC_FORMAT: &str = "finstack.doc_format"`, `pub const META_HEADING_PATH: &str = "finstack.heading_path"`

- [ ] **Step 1: Write failing tests**

`tests.rs` (representative — cover every validation rule):

```rust
use std::sync::Arc;

use super::error::KnowledgeStoreError;
use super::types::{
    ChunkId, ChunkQuery, ChunkRange, ChunkRecord, CollectionId, SearchMode, SourceId,
    SourceListQuery,
};

fn digest_of(bytes: &[u8]) -> finstack_ai_kernel::Digest {
    // Use the same digest constructor the artifact service uses. Grep
    // `Digest` usage in crates/finstack-ai-runtime/src/services/artifact.rs
    // and copy the call (e.g. Digest::compute(bytes) or equivalent). Do not
    // invent a new hashing path.
    unimplemented!("replace with the artifact-service digest call")
}

#[test]
fn collection_id_validates_slug() {
    assert!(CollectionId::parse("deals-2026").is_ok());
    assert_eq!(CollectionId::default_collection().as_str(), "default");
    for bad in ["", "-lead", "trail-", "UPPER", "has space", "a".repeat(65).as_str()] {
        assert!(
            matches!(CollectionId::parse(bad), Err(KnowledgeStoreError::InvalidQuery { .. })),
            "expected rejection for {bad:?}"
        );
    }
}

#[test]
fn chunk_range_rejects_inverted() {
    assert!(ChunkRange::new(3, 2).is_err());
    assert!(ChunkRange::new(2, 2).is_ok());
}

#[test]
fn chunk_query_bounds_top_k() {
    let collection = CollectionId::default_collection();
    let mode = SearchMode::Keyword { text: Arc::from("revenue") };
    assert!(ChunkQuery::new(collection.clone(), mode.clone(), 0).is_err());
    assert!(ChunkQuery::new(collection.clone(), mode.clone(), 1025).is_err());
    assert!(ChunkQuery::new(collection, mode, 8).is_ok());
}

#[test]
fn chunk_record_serde_round_trip() {
    let collection = CollectionId::default_collection();
    let source = SourceId::new(digest_of(b"doc"));
    let record = ChunkRecord::new(
        collection,
        ChunkId::new(source, 0),
        digest_of(b"chunk text"),
        Arc::from("chunk text"),
    )
    .with_vector(Arc::from([0.5_f32, 0.5]));
    let json = serde_json::to_vec(&record).expect("serialize");
    let back: ChunkRecord = serde_json::from_slice(&json).expect("deserialize");
    assert_eq!(back.text(), record.text());
    assert_eq!(back.vector(), record.vector());
}

#[test]
fn source_list_query_defaults() {
    let query = SourceListQuery::new(CollectionId::default_collection());
    assert_eq!(query.limit(), 100);
}
```

Add matching tests for `GraphQuery` depth clamping (`with_depth(9)` → `depth() == 3`), `CommunityQuery` limit cap, `SourceRecord` serde round-trip, and `GraphExportRecord` serde round-trip (one of each variant). Resolve the `digest_of` helper before running: find the digest constructor (`rtk proxy grep -n "Digest::" crates/finstack-ai-runtime/src/services/artifact.rs`) and substitute the real call.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p finstack-ai-runtime knowledge`
Expected: FAIL — modules unresolved.

- [ ] **Step 3: Implement `types.rs` and `error.rs`**

Follow the Interfaces block exactly. Implementation notes:
- Private fields + public getters everywhere (field additions then never break callers); enums get `#[non_exhaustive]`.
- `CollectionId::parse` validation: `(1..=64).contains(&s.len())`, all bytes `matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'-')`, `!s.starts_with('-') && !s.ends_with('-')`; failure is `InvalidQuery { reason: "collection_slug_invalid" }`.
- Serde derives only on the types named as wire formats (`ChunkRecord`, `SourceRecord`, `SourceStatus`, `Entity`, `Relation`, `Community`, `GraphExportRecord`, `CollectionId`, `SourceId`, `ChunkId`) with `#[serde(rename_all = "snake_case")]` on enums; `Digest`/`Timestamp`/`Metadata` already serialize (they are kernel types).
- `error.rs` mirrors Task 1's error style with the display prefixes from the Interfaces block.
- `mod.rs` for now: `pub(crate) mod error; pub(crate) mod types; #[cfg(test)] mod tests;` plus `pub use` of everything; traits arrive in Task 3.

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test -p finstack-ai-runtime knowledge`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/finstack-ai-runtime/src/ports/mod.rs crates/finstack-ai-runtime/src/ports/knowledge/ crates/finstack-ai-runtime/src/lib.rs
git commit -m "feat: add knowledge port shared types and errors"
```

---

### Task 3: `ports/knowledge` — traits and fusion strategies

**Files:**
- Create: `crates/finstack-ai-runtime/src/ports/knowledge/port.rs` (the four async traits)
- Create: `crates/finstack-ai-runtime/src/ports/knowledge/fusion.rs`
- Modify: `crates/finstack-ai-runtime/src/ports/knowledge/mod.rs`, `tests.rs`
- Modify: `crates/finstack-ai-runtime/src/lib.rs` (re-exports)

**Interfaces:**
- Produces (used by all store/engine/query tasks):
  - `trait ChunkIndex: PortObject` — exactly the eight methods from spec decision 2 (signatures verbatim, including `collection: CollectionId` on `get_chunks` and `delete_chunks`)
  - `trait GraphStore: PortObject` — the seven methods from spec decision 2
  - `trait SourceCatalog: PortObject` — `put`, `get`, `list`, `delete` from spec decision 2
  - `trait SourceStore: PortObject` — `put(collection, source, media_type: Arc<str>, bytes: Arc<[u8]>)`, `get(collection, source) -> …Option<StoredSource>…`, `exists(collection, source) -> …bool…`, `delete(collection, source) -> …bool…` (spec decisions 2/2a). Trait doc states the content-addressing contract: `put` MUST verify `bytes` hash to `source`'s digest and reject mismatches with `InvalidQuery { reason: "source_digest_mismatch" }`; duplicate `put` of identical content is a no-op `Ok`.
  - `trait Reranker: PortObject { fn rerank(&self, ctx: RerankCallContext, query: Arc<str>, candidates: Arc<[Arc<str>]>) -> PortFuture<Result<Vec<RerankEntry>, RerankError>>; }` with `RerankEntry::new(index: u32, score: f32)` and `RerankError` (`Provider`, `Deadline`, `Cancelled`, `InvalidOutput { reason: &'static str }`, `Unsupported`, all non_exhaustive); `RerankCallContext::detached(deadline: Option<Timestamp>)`
  - `trait FusionStrategy: PortObject { fn fuse(&self, rankings: &[&[ChunkHit]], top_k: u32) -> Vec<ChunkHit>; }` (synchronous)
  - `RrfFusion::new()` / `RrfFusion::with_k(u32)` (default k=60)
  - `WeightedScoreFusion::new(alpha: f32)` (alpha = weight of the first ranking, clamped to 0.0..=1.0)

- [ ] **Step 1: Write failing fusion tests**

Append to `tests.rs`:

```rust
use super::fusion::{RrfFusion, WeightedScoreFusion};
use super::port::FusionStrategy as _;
use super::types::ChunkHit;

fn hit(collection: &CollectionId, source: SourceId, index: u32, score: f32) -> ChunkHit {
    let record = ChunkRecord::new(
        collection.clone(),
        ChunkId::new(source, index),
        digest_of(format!("chunk-{index}").as_bytes()),
        Arc::from(format!("chunk-{index}").as_str()),
    );
    ChunkHit::new(record, score)
}

#[test]
fn rrf_prefers_items_ranked_high_in_both_lists() {
    let collection = CollectionId::default_collection();
    let source = SourceId::new(digest_of(b"doc"));
    // Semantic ranking: chunks 0, 1, 2. Keyword ranking: chunks 2, 0, 3.
    let semantic = vec![
        hit(&collection, source, 0, 0.9),
        hit(&collection, source, 1, 0.8),
        hit(&collection, source, 2, 0.7),
    ];
    let keyword = vec![
        hit(&collection, source, 2, 12.0),
        hit(&collection, source, 0, 11.0),
        hit(&collection, source, 3, 10.0),
    ];
    let fused = RrfFusion::new().fuse(&[&semantic, &keyword], 4);
    // Chunk 0: 1/(60+1) + 1/(60+2); chunk 2: 1/(60+3) + 1/(60+1) — both beat
    // single-list chunks 1 and 3.
    let order: Vec<u32> = fused.iter().map(|h| h.record().chunk_id().index()).collect();
    assert_eq!(order.len(), 4);
    assert!(order[..2].contains(&0) && order[..2].contains(&2));
}

#[test]
fn rrf_deduplicates_and_respects_top_k() {
    let collection = CollectionId::default_collection();
    let source = SourceId::new(digest_of(b"doc"));
    let a = vec![hit(&collection, source, 0, 1.0)];
    let b = vec![hit(&collection, source, 0, 1.0)];
    let fused = RrfFusion::new().fuse(&[&a, &b], 10);
    assert_eq!(fused.len(), 1);
    let fused = RrfFusion::new().fuse(&[&a, &b], 0);
    assert!(fused.is_empty());
}

#[test]
fn weighted_fusion_alpha_extremes_reproduce_single_lists() {
    let collection = CollectionId::default_collection();
    let source = SourceId::new(digest_of(b"doc"));
    let semantic = vec![hit(&collection, source, 0, 0.9), hit(&collection, source, 1, 0.1)];
    let keyword = vec![hit(&collection, source, 1, 5.0), hit(&collection, source, 0, 1.0)];
    let all_semantic = WeightedScoreFusion::new(1.0).fuse(&[&semantic, &keyword], 2);
    assert_eq!(all_semantic[0].record().chunk_id().index(), 0);
    let all_keyword = WeightedScoreFusion::new(0.0).fuse(&[&semantic, &keyword], 2);
    assert_eq!(all_keyword[0].record().chunk_id().index(), 1);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p finstack-ai-runtime fusion`
Expected: FAIL.

- [ ] **Step 3: Implement `port.rs` and `fusion.rs`**

`port.rs`: the five traits with signatures copied verbatim from spec decision 2 (see Interfaces). Each trait gets a doc comment stating its contract lines: dimension-mismatch rejection (ChunkIndex), per-source attribution semantics (GraphStore — "when several sources attest the same entity or relation, delete_by_source removes only that source's contribution"), catalog-as-commit-point (SourceCatalog). No default methods in v1 — the policy line goes in the module doc: "future methods must ship default implementations returning `Unsupported`".

`fusion.rs`:

```rust
//! Pure ranking-fusion strategies shipped beside the FusionStrategy trait.

use std::collections::HashMap;

use super::types::{ChunkHit, ChunkId};

use super::port::FusionStrategy;

/// Reciprocal-rank fusion: score(item) = Σ over rankings 1/(k + rank).
///
/// Needs no score normalization across heterogeneous backends, which is why
/// it is the default.
#[derive(Debug, Clone)]
pub struct RrfFusion {
    k: u32,
}

impl RrfFusion {
    /// Default constant k = 60.
    #[must_use]
    pub fn new() -> Self {
        Self { k: 60 }
    }

    /// Explicit k.
    #[must_use]
    pub fn with_k(k: u32) -> Self {
        Self { k: k.max(1) }
    }
}

impl Default for RrfFusion {
    fn default() -> Self {
        Self::new()
    }
}

impl FusionStrategy for RrfFusion {
    fn fuse(&self, rankings: &[&[ChunkHit]], top_k: u32) -> Vec<ChunkHit> {
        let mut scores: HashMap<ChunkId, (f32, ChunkHit)> = HashMap::new();
        for ranking in rankings {
            for (rank, hit) in ranking.iter().enumerate() {
                let increment = 1.0 / (self.k as f32 + rank as f32 + 1.0);
                scores
                    .entry(hit.record().chunk_id())
                    .and_modify(|(score, _)| *score += increment)
                    .or_insert_with(|| (increment, hit.clone()));
            }
        }
        rerank_by_score(scores, top_k)
    }
}

/// Min-max-normalized weighted sum: alpha * first ranking + (1-alpha) * rest.
#[derive(Debug, Clone)]
pub struct WeightedScoreFusion {
    alpha: f32,
}

impl WeightedScoreFusion {
    /// `alpha` is the weight of the FIRST ranking, clamped to 0.0..=1.0.
    #[must_use]
    pub fn new(alpha: f32) -> Self {
        Self { alpha: alpha.clamp(0.0, 1.0) }
    }
}

impl FusionStrategy for WeightedScoreFusion {
    fn fuse(&self, rankings: &[&[ChunkHit]], top_k: u32) -> Vec<ChunkHit> {
        let mut scores: HashMap<ChunkId, (f32, ChunkHit)> = HashMap::new();
        for (list_index, ranking) in rankings.iter().enumerate() {
            let weight = if list_index == 0 { self.alpha } else { 1.0 - self.alpha };
            let max = ranking.iter().map(|h| h.score()).fold(f32::MIN, f32::max);
            let min = ranking.iter().map(|h| h.score()).fold(f32::MAX, f32::min);
            let span = (max - min).max(f32::EPSILON);
            for hit in ranking.iter() {
                let normalized = (hit.score() - min) / span;
                let increment = weight * normalized;
                scores
                    .entry(hit.record().chunk_id())
                    .and_modify(|(score, _)| *score += increment)
                    .or_insert_with(|| (increment, hit.clone()));
            }
        }
        rerank_by_score(scores, top_k)
    }
}

fn rerank_by_score(
    scores: HashMap<ChunkId, (f32, ChunkHit)>,
    top_k: u32,
) -> Vec<ChunkHit> {
    let mut fused: Vec<(f32, ChunkHit)> = scores.into_values().collect();
    fused.sort_by(|a, b| b.0.total_cmp(&a.0));
    fused
        .into_iter()
        .take(top_k as usize)
        .map(|(score, hit)| ChunkHit::new(hit.record().clone(), score))
        .collect()
}
```

(`ChunkId` needs `Hash + Eq + Clone + Copy`-compatible derives — add `#[derive(Clone, Copy, PartialEq, Eq, Hash)]` in Task 2's type if not present; `SourceId`/`Digest` must be `Hash`-able — check `Digest`'s derives and wrap accordingly.)

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test -p finstack-ai-runtime knowledge && cargo test -p finstack-ai-runtime fusion`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/finstack-ai-runtime/src/ports/knowledge/ crates/finstack-ai-runtime/src/lib.rs
git commit -m "feat: add knowledge port traits and fusion strategies"
```

---

### Task 4: `finstack-ai-knowledge-memory` — ChunkIndex

**Files:**
- Create: `extensions/stores/finstack-ai-knowledge-memory/Cargo.toml`
- Create: `extensions/stores/finstack-ai-knowledge-memory/src/lib.rs`
- Create: `extensions/stores/finstack-ai-knowledge-memory/src/chunk_index.rs`
- Create: `extensions/stores/finstack-ai-knowledge-memory/src/tests.rs`
- Modify: root `Cargo.toml` (workspace `members` — add after the `finstack-ai-store-sqlite` line)

**Interfaces:**
- Consumes: every port type/trait from Tasks 2–3.
- Produces: `MemoryKnowledgeStore::new() -> Self` — ONE struct that will implement all three store traits (`ChunkIndex` here; `GraphStore` + `SourceCatalog` in Task 5). `Clone` + cheap (interior `Arc<Mutex<…>>` state, one mutex per store facet). Reports `ChunkIndexCapabilities` with all three modes native.

**Crate `Cargo.toml`** (dependency-table style copied from `extensions/stores/finstack-ai-store-memory/Cargo.toml` — read it first and match its exact form):

```toml
[package]
name = "finstack-ai-knowledge-memory"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true
homepage.workspace = true
authors.workspace = true
description = "In-memory ChunkIndex, GraphStore, and SourceCatalog knowledge backends"

[dependencies]
finstack-ai-runtime = { path = "../../../crates/finstack-ai-runtime", default-features = false }
finstack-ai-kernel = { path = "../../../crates/finstack-ai-kernel" }
futures-util = { workspace = true }
serde_json = { workspace = true }
thiserror = { workspace = true }

[lints]
workspace = true
```

- [ ] **Step 1: Write failing tests**

`src/tests.rs` — use a small local `block_on` helper (copy the one used in `crates/finstack-ai-runtime/src/services/artifact.rs` tests) and a `digest_of` helper (same discovery as Task 2). Core cases:

```rust
use std::sync::Arc;

use finstack_ai_runtime::{
    ChunkId, ChunkIndex as _, ChunkQuery, ChunkRange, ChunkRecord, CollectionId,
    KnowledgeStoreError, SearchMode, SourceId,
};

use crate::MemoryKnowledgeStore;

fn record(
    collection: &CollectionId,
    source: SourceId,
    index: u32,
    text: &str,
    vector: Option<&[f32]>,
) -> ChunkRecord {
    let mut record = ChunkRecord::new(
        collection.clone(),
        ChunkId::new(source, index),
        digest_of(text.as_bytes()),
        Arc::from(text),
    );
    if let Some(vector) = vector {
        record = record.with_vector(Arc::from(vector));
    }
    record
}

#[test]
fn semantic_search_ranks_by_cosine() {
    let store = MemoryKnowledgeStore::new();
    let collection = CollectionId::default_collection();
    let source = SourceId::new(digest_of(b"doc"));
    block_on(store.upsert(Arc::from([
        record(&collection, source, 0, "north", Some(&[1.0, 0.0])),
        record(&collection, source, 1, "east", Some(&[0.0, 1.0])),
    ])))
    .expect("upsert");
    let hits = block_on(store.search(
        ChunkQuery::new(
            collection,
            SearchMode::Semantic { vector: Arc::from([0.9_f32, 0.1]) },
            2,
        )
        .expect("query"),
    ))
    .expect("search");
    assert_eq!(hits[0].record().chunk_id().index(), 0);
    assert!(hits[0].score() > hits[1].score());
}

#[test]
fn keyword_search_matches_terms_and_ranks_rarer_higher() {
    let store = MemoryKnowledgeStore::new();
    let collection = CollectionId::default_collection();
    let source = SourceId::new(digest_of(b"doc"));
    block_on(store.upsert(Arc::from([
        record(&collection, source, 0, "quarterly revenue rose sharply", None),
        record(&collection, source, 1, "revenue revenue revenue revenue", None),
        record(&collection, source, 2, "unrelated filler text entirely", None),
    ])))
    .expect("upsert");
    let hits = block_on(store.search(
        ChunkQuery::new(
            collection,
            SearchMode::Keyword { text: Arc::from("quarterly revenue") },
            3,
        )
        .expect("query"),
    ))
    .expect("search");
    // Chunk 2 shares no terms and must not appear; chunk 0 matches both terms.
    assert!(hits.iter().all(|h| h.record().chunk_id().index() != 2));
    assert_eq!(hits[0].record().chunk_id().index(), 0);
}

#[test]
fn hybrid_search_returns_results_from_both_modes() {
    let store = MemoryKnowledgeStore::new();
    let collection = CollectionId::default_collection();
    let source = SourceId::new(digest_of(b"doc"));
    block_on(store.upsert(Arc::from([
        record(&collection, source, 0, "alpha beta", Some(&[1.0, 0.0])),
        record(&collection, source, 1, "gamma delta", Some(&[0.0, 1.0])),
    ])))
    .expect("upsert");
    let hits = block_on(store.search(
        ChunkQuery::new(
            collection,
            SearchMode::Hybrid {
                vector: Arc::from([0.0_f32, 1.0]),
                text: Arc::from("alpha"),
            },
            2,
        )
        .expect("query"),
    ))
    .expect("search");
    assert_eq!(hits.len(), 2);
}

#[test]
fn dimension_mismatch_is_rejected_per_collection() {
    let store = MemoryKnowledgeStore::new();
    let collection = CollectionId::default_collection();
    let source = SourceId::new(digest_of(b"doc"));
    block_on(store.upsert(Arc::from([record(
        &collection, source, 0, "a", Some(&[1.0, 0.0]),
    )])))
    .expect("first dimension wins");
    let error = block_on(store.upsert(Arc::from([record(
        &collection, source, 1, "b", Some(&[1.0, 0.0, 0.0]),
    )])))
    .expect_err("mismatched dimension");
    assert!(matches!(error, KnowledgeStoreError::DimensionMismatch { expected: 2, got: 3 }));
}

#[test]
fn collections_are_isolated() {
    let store = MemoryKnowledgeStore::new();
    let a = CollectionId::parse("coll-a").expect("slug");
    let b = CollectionId::parse("coll-b").expect("slug");
    let source = SourceId::new(digest_of(b"doc"));
    block_on(store.upsert(Arc::from([record(&a, source, 0, "secret alpha", None)])))
        .expect("upsert");
    let hits = block_on(store.search(
        ChunkQuery::new(b, SearchMode::Keyword { text: Arc::from("secret") }, 8)
            .expect("query"),
    ))
    .expect("search");
    assert!(hits.is_empty());
}

#[test]
fn get_chunks_clamps_range_and_delete_by_source_counts() {
    let store = MemoryKnowledgeStore::new();
    let collection = CollectionId::default_collection();
    let source = SourceId::new(digest_of(b"doc"));
    block_on(store.upsert(Arc::from([
        record(&collection, source, 0, "zero", None),
        record(&collection, source, 1, "one", None),
        record(&collection, source, 2, "two", None),
    ])))
    .expect("upsert");
    let chunks = block_on(store.get_chunks(
        collection.clone(),
        source,
        ChunkRange::new(1, 9).expect("range"),
    ))
    .expect("get");
    assert_eq!(chunks.len(), 2); // indices 1 and 2; 9 clamped
    let deleted = block_on(store.delete_by_source(collection, source)).expect("delete");
    assert_eq!(deleted, 3);
}

#[test]
fn upsert_replaces_same_chunk_id() {
    let store = MemoryKnowledgeStore::new();
    let collection = CollectionId::default_collection();
    let source = SourceId::new(digest_of(b"doc"));
    block_on(store.upsert(Arc::from([record(&collection, source, 0, "old", None)])))
        .expect("upsert");
    block_on(store.upsert(Arc::from([record(&collection, source, 0, "new", None)])))
        .expect("upsert again");
    let chunks = block_on(store.get_chunks(
        collection,
        source,
        ChunkRange::new(0, 0).expect("range"),
    ))
    .expect("get");
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].text().as_ref(), "new");
}
```

Also add: `export`/`import` round-trip (export collection → import into a fresh store → same search results), `delete_chunks` removes exactly the named ids, and `capabilities()` reporting all three modes native.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p finstack-ai-knowledge-memory`
Expected: FAIL — crate empty.

- [ ] **Step 3: Implement `chunk_index.rs`**

Data layout: `Mutex<HashMap<CollectionId, CollectionChunks>>` where

```rust
struct CollectionChunks {
    dimensions: Option<u32>,                 // first-inserted wins
    records: BTreeMap<(SourceId, u32), ChunkRecord>, // ordered for get_chunks
}
```

Algorithms (all private to this crate):
- **Cosine**: `dot(a, b) / (norm(a) * norm(b))` with `norm == 0` guarded to score 0. Skip records without vectors.
- **Keyword scoring (BM25)**: tokenize = lowercase, split on non-alphanumeric, drop tokens shorter than 2 chars. Compute per-query-term BM25 with k1 = 1.2, b = 0.75, over the collection's records; `idf = ln(1 + (n - df + 0.5) / (df + 0.5))`. Records with zero matching terms are excluded.
- **Hybrid**: run both, fuse internally with the same RRF formula (k = 60) — private code, do NOT import `RrfFusion` (backends must not depend on the shipped default staying available).
- **Filter**: a record matches when every top-level key in the query filter exists in the record metadata with an equal JSON value. Read `Metadata`'s accessor API in `crates/finstack-ai-kernel/src/primitives/raw_json.rs` (it wraps a JSON object) and compare values via `serde_json::Value` equality.
- `search` on an unknown collection returns `Ok(vec![])` (searching before ingesting is not an error); `get_chunks`/`delete_*` on an unknown collection return `Err(KnowledgeStoreError::UnknownCollection)`.
- `export`: collect the collection's records into a `Vec`, wrap with `futures_util::stream::iter(records.into_iter().map(Ok))`, box as `PortStream`. `import`: drain the stream (`futures_util::StreamExt::next` in a loop), validate each record like `upsert`, count.

`lib.rs`:

```rust
//! In-memory knowledge backends: ChunkIndex, GraphStore, SourceCatalog.

#![warn(missing_docs)]

mod chunk_index;

#[cfg(test)]
mod tests;

pub use chunk_index::MemoryKnowledgeStore;
```

(`MemoryKnowledgeStore` is defined in `chunk_index.rs` for now and gains the graph/catalog facets in Task 5 from their own modules via `impl` blocks — keep the struct's fields split per facet: `chunks: Mutex<…>`, later `graph: Mutex<…>`, `catalog: Mutex<…>`.)

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test -p finstack-ai-knowledge-memory`
Expected: PASS.

- [ ] **Step 5: Verify wasm cleanliness**

Run: `cargo check -p finstack-ai-knowledge-memory --target wasm32-unknown-unknown`
Expected: clean (no tokio, no fs, no threads beyond Mutex).

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock extensions/stores/finstack-ai-knowledge-memory/
git commit -m "feat: add in-memory ChunkIndex backend"
```

---

### Task 5: `finstack-ai-knowledge-memory` — GraphStore and SourceCatalog

**Files:**
- Create: `extensions/stores/finstack-ai-knowledge-memory/src/graph_store.rs`
- Create: `extensions/stores/finstack-ai-knowledge-memory/src/catalog.rs`
- Create: `extensions/stores/finstack-ai-knowledge-memory/src/source_store.rs`
- Modify: `extensions/stores/finstack-ai-knowledge-memory/src/lib.rs`, `src/chunk_index.rs` (add facet fields), `src/tests.rs`

**Interfaces:**
- Consumes: `GraphStore`, `SourceCatalog` traits and graph/community/source types (Tasks 2–3).
- Produces: `MemoryKnowledgeStore` additionally implements `GraphStore`, `SourceCatalog`, and `SourceStore` (`Mutex<HashMap<(CollectionId, SourceId), StoredSource>>`; `put` re-hashes the bytes with the kernel digest and compares to the `SourceId` — mismatch → `InvalidQuery`). Merge key for entities: `(lowercase(name), lowercase(entity_type))`. Per-source attribution: the store keeps each source's fragment verbatim and materializes the merged view on query/export.

- [ ] **Step 1: Write failing tests**

Append to `src/tests.rs`:

```rust
use finstack_ai_runtime::{
    CommunityLookup, CommunityQuery, Entity, EntityLookup, GraphFragment, GraphQuery,
    GraphStore as _, Relation, SourceCatalog as _, SourceListQuery, SourceRecord, SourceStatus,
};

fn fragment(collection: &CollectionId, source: SourceId, entities: &[(&str, &str)], relations: &[(&str, &str, &str)]) -> GraphFragment {
    GraphFragment::new(collection.clone(), source)
        .with_entities(
            entities
                .iter()
                .map(|(name, ty)| Entity::new(Arc::from(*name), Arc::from(*ty), Arc::from("desc")))
                .collect(),
        )
        .with_relations(
            relations
                .iter()
                .map(|(from, to, ty)| {
                    Relation::new(Arc::from(*from), Arc::from(*to), Arc::from(*ty), Arc::from("rel"))
                })
                .collect(),
        )
}

#[test]
fn entities_merge_case_insensitively_across_sources() {
    let store = MemoryKnowledgeStore::new();
    let collection = CollectionId::default_collection();
    let s1 = SourceId::new(digest_of(b"doc-1"));
    let s2 = SourceId::new(digest_of(b"doc-2"));
    block_on(store.upsert(fragment(&collection, s1, &[("ACME Corp", "company")], &[])))
        .expect("upsert 1");
    block_on(store.upsert(fragment(&collection, s2, &[("acme corp", "company")], &[])))
        .expect("upsert 2");
    let result = block_on(store.query(
        GraphQuery::new(
            collection,
            EntityLookup::Name { name: Arc::from("acme corp"), entity_type: None },
        )
        .expect("query"),
    ))
    .expect("result");
    assert_eq!(result.entities().len(), 1);
}

#[test]
fn delete_by_source_keeps_entities_attested_elsewhere() {
    let store = MemoryKnowledgeStore::new();
    let collection = CollectionId::default_collection();
    let s1 = SourceId::new(digest_of(b"doc-1"));
    let s2 = SourceId::new(digest_of(b"doc-2"));
    block_on(store.upsert(fragment(
        &collection, s1,
        &[("Acme", "company"), ("OnlyInOne", "company")],
        &[("Acme", "OnlyInOne", "supplies")],
    )))
    .expect("upsert 1");
    block_on(store.upsert(fragment(&collection, s2, &[("Acme", "company")], &[])))
        .expect("upsert 2");
    block_on(store.delete_by_source(collection.clone(), s1)).expect("delete");
    let acme = block_on(store.query(
        GraphQuery::new(
            collection.clone(),
            EntityLookup::Name { name: Arc::from("acme"), entity_type: None },
        )
        .expect("query"),
    ))
    .expect("acme survives");
    assert_eq!(acme.entities().len(), 1);
    let gone = block_on(store.query(
        GraphQuery::new(
            collection,
            EntityLookup::Name { name: Arc::from("onlyinone"), entity_type: None },
        )
        .expect("query"),
    ))
    .expect("query ok");
    assert!(gone.entities().is_empty());
}

#[test]
fn neighborhood_expansion_respects_depth() {
    let store = MemoryKnowledgeStore::new();
    let collection = CollectionId::default_collection();
    let source = SourceId::new(digest_of(b"doc"));
    // Chain: A -> B -> C
    block_on(store.upsert(fragment(
        &collection, source,
        &[("A", "company"), ("B", "company"), ("C", "company")],
        &[("A", "B", "owns"), ("B", "C", "owns")],
    )))
    .expect("upsert");
    let depth1 = block_on(store.query(
        GraphQuery::new(
            collection.clone(),
            EntityLookup::Name { name: Arc::from("a"), entity_type: None },
        )
        .expect("query")
        .with_depth(1),
    ))
    .expect("depth 1");
    assert_eq!(depth1.entities().len(), 2); // A, B
    let depth2 = block_on(store.query(
        GraphQuery::new(
            collection,
            EntityLookup::Name { name: Arc::from("a"), entity_type: None },
        )
        .expect("query")
        .with_depth(2),
    ))
    .expect("depth 2");
    assert_eq!(depth2.entities().len(), 3); // A, B, C
}

#[test]
fn semantic_entity_lookup_ranks_by_cosine() {
    let store = MemoryKnowledgeStore::new();
    let collection = CollectionId::default_collection();
    let source = SourceId::new(digest_of(b"doc"));
    let fragment = GraphFragment::new(collection.clone(), source).with_entities(vec![
        Entity::new(Arc::from("North"), Arc::from("company"), Arc::from("d"))
            .with_embedding(Arc::from([1.0_f32, 0.0])),
        Entity::new(Arc::from("East"), Arc::from("company"), Arc::from("d"))
            .with_embedding(Arc::from([0.0_f32, 1.0])),
    ]);
    block_on(store.upsert(fragment)).expect("upsert");
    let result = block_on(store.query(
        GraphQuery::new(
            collection,
            EntityLookup::Semantic { vector: Arc::from([0.9_f32, 0.1]), top_k: 1 },
        )
        .expect("query"),
    ))
    .expect("result");
    assert_eq!(result.entities().len(), 1);
    assert_eq!(result.entities()[0].name().as_ref(), "North");
}

#[test]
fn communities_replace_wholesale_and_filter() {
    let store = MemoryKnowledgeStore::new();
    let collection = CollectionId::default_collection();
    let community = |id: &str, title: &str| {
        finstack_ai_runtime::Community::new(
            collection.clone(), Arc::from(id), 0, Arc::from(title), Arc::from("summary"),
        )
    };
    block_on(store.replace_communities(collection.clone(), Arc::from([community("c1", "Energy")])))
        .expect("replace 1");
    block_on(store.replace_communities(collection.clone(), Arc::from([community("c2", "Metals")])))
        .expect("replace 2");
    let all = block_on(store.communities(CommunityQuery::new(collection, CommunityLookup::All)))
        .expect("list");
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].title().as_ref(), "Metals");
}

#[test]
fn catalog_round_trip_list_and_delete() {
    let store = MemoryKnowledgeStore::new();
    let collection = CollectionId::default_collection();
    let source = SourceId::new(digest_of(b"doc"));
    let record = SourceRecord::new(
        collection.clone(), source, Arc::from("csv"), test_timestamp(), SourceStatus::Indexed,
    )
    .with_name(Arc::from("revenue.csv"));
    block_on(store.put(record)).expect("put");
    let fetched = block_on(store.get(collection.clone(), source)).expect("get");
    assert!(fetched.is_some());
    let listed = block_on(store.list(
        SourceListQuery::new(collection.clone()).with_name_contains(Arc::from("revenue")),
    ))
    .expect("list");
    assert_eq!(listed.len(), 1);
    assert!(block_on(store.delete(collection.clone(), source)).expect("delete"));
    assert!(block_on(store.get(collection, source)).expect("get").is_none());
}
```

`test_timestamp()`: construct a fixed `Timestamp` — read `crates/finstack-ai-kernel/src/primitives/time.rs` for the constructor (it wraps `i64`) and use a constant value.

Also add: graph `export`/`import` round-trip preserving per-source attribution (upsert two sources → export → import into fresh store → `delete_by_source` of one still keeps the shared entity); and `SourceStore` tests — `put`+`get` round-trip returns identical bytes and media type, `put` with bytes that do NOT hash to the `SourceId` → `InvalidQuery`, duplicate `put` is `Ok`, `exists`/`delete` behave, collections isolated.

**Method-resolution note:** `ChunkIndex` and `GraphStore` both define `upsert` and `delete_by_source`. Once both traits are in scope for one store value, bare `store.upsert(…)` is ambiguous — write the graph calls as `GraphStore::upsert(&store, …)` / `GraphStore::delete_by_source(&store, …)` and the chunk calls as `ChunkIndex::upsert(&store, …)` etc. throughout these tests (adjust the Task 4 tests when this task lands).

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p finstack-ai-knowledge-memory entities_merge`
Expected: FAIL — no `GraphStore` impl.

- [ ] **Step 3: Implement `graph_store.rs` and `catalog.rs`**

Graph layout — per collection:

```rust
struct CollectionGraph {
    // Verbatim per-source fragments: the attribution ledger.
    fragments: HashMap<SourceId, GraphFragment>,
    // Communities replaced wholesale.
    communities: Vec<Community>,
}
```

Query path: build the merged view on demand — iterate all fragments, merge entities by `(name.to_lowercase(), entity_type.to_lowercase())` (union source_chunks; concatenate distinct descriptions with "; "; keep the first non-`None` embedding), merge relations by `(from_key, to_key, relation_type)`. Then resolve the lookup (name match = case-insensitive equality; semantic = cosine over entity embeddings), breadth-first expand `depth` hops over merged relations (respect `relation_types` filter and `max_entities`/`max_relations` ceilings), and return the induced subgraph. `delete_by_source` removes the fragment; `upsert` replaces the source's fragment (`fragments.insert(fragment.source(), fragment)`). Export emits `GraphExportRecord::Entity`/`Relation` per fragment (verbatim, with `source`) then `Community` rows; import routes rows back into `fragments`/`communities`.

Merged-view rebuild cost is O(total fragment size) per query — acceptable for the test/wasm backend; note it in the module doc.

Catalog: `Mutex<HashMap<(CollectionId, SourceId), SourceRecord>>`; `list` filters by the query's fields (name substring is case-insensitive `contains`), sorts by `ingested_at` descending, truncates to `limit()`.

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test -p finstack-ai-knowledge-memory`
Expected: PASS.

- [ ] **Step 5: Re-verify wasm**

Run: `cargo check -p finstack-ai-knowledge-memory --target wasm32-unknown-unknown`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add extensions/stores/finstack-ai-knowledge-memory/
git commit -m "feat: add in-memory GraphStore and SourceCatalog backends"
```

---

### Task 6: `finstack-ai-ingest` scaffold + `MarkdownChunker`

**Files:**
- Create: `extensions/ingest/finstack-ai-ingest/Cargo.toml`
- Create: `extensions/ingest/finstack-ai-ingest/src/lib.rs`
- Create: `extensions/ingest/finstack-ai-ingest/src/chunker.rs`
- Create: `extensions/ingest/finstack-ai-ingest/src/tests.rs`
- Modify: root `Cargo.toml` (workspace `members` — new `extensions/ingest/` category dir, add after the `extensions/context/…` entries)

**Interfaces:**
- Consumes: `ChunkId`, `SourceId`, `Metadata` from runtime; `parser::ParsedDocument` NOT yet (chunker input is its own type so the chunker is testable without documents).
- Produces (used by Tasks 8–10):
  - `Chunk { — private fields — }` with `Chunk::new(id: ChunkId, text: Arc<str>) -> Self`, `.with_heading_path(Vec<Arc<str>>)`, `.with_metadata(Metadata)`; getters `id()`, `text()`, `heading_path()`, `metadata()`
  - `ChunkInput::new(source: SourceId, markdown: Arc<str>)`; getters
  - `ChunkerConfig { target_bytes: usize (default 1600), overlap_bytes: usize (default 200), max_chunk_bytes: usize (default 8192) }` with `Default`
  - `trait Chunker: PortObject { fn chunk(&self, input: &ChunkInput) -> Result<Vec<Chunk>, ChunkError>; }`
  - `MarkdownChunker::new(config: ChunkerConfig)`, `Default` via default config
  - `ChunkError::{TooManyChunks { max: u32 }, Internal { reason: &'static str }}` (non_exhaustive, thiserror)

**Crate `Cargo.toml`:**

```toml
[package]
name = "finstack-ai-ingest"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true
homepage.workspace = true
authors.workspace = true
description = "Knowledge ingest pipeline: chunking, embedding, graph extraction, indexing"

[dependencies]
finstack-ai-runtime = { path = "../../../crates/finstack-ai-runtime", default-features = false }
finstack-ai-kernel = { path = "../../../crates/finstack-ai-kernel" }
finstack-ai-tools-document = { path = "../../toolsets/finstack-ai-tools-document" }
futures-util = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
thiserror = { workspace = true }

[lints]
workspace = true
```

- [ ] **Step 1: Write failing chunker tests**

`src/tests.rs`:

```rust
use std::sync::Arc;

use finstack_ai_runtime::SourceId;

use crate::chunker::{Chunker as _, ChunkerConfig, ChunkInput, MarkdownChunker};

// digest_of: same discovery as earlier tasks — copy the artifact-service
// digest call.

fn input(markdown: &str) -> ChunkInput {
    ChunkInput::new(SourceId::new(digest_of(markdown.as_bytes())), Arc::from(markdown))
}

#[test]
fn splits_at_headings_and_records_heading_path() {
    let chunks = MarkdownChunker::default()
        .chunk(&input("# Title\n\nIntro text.\n\n## Section A\n\nBody A.\n\n## Section B\n\nBody B.\n"))
        .expect("chunks");
    assert!(chunks.len() >= 3);
    let section_a = chunks
        .iter()
        .find(|c| c.text().contains("Body A"))
        .expect("section A chunk");
    assert_eq!(
        section_a.heading_path().iter().map(|h| h.as_ref()).collect::<Vec<_>>(),
        vec!["Title", "Section A"]
    );
}

#[test]
fn packs_small_sections_and_splits_oversized_ones() {
    let long_paragraph = "word ".repeat(1000); // ~5000 bytes
    let markdown = format!("# T\n\n{long_paragraph}\n");
    let chunks = MarkdownChunker::new(ChunkerConfig {
        target_bytes: 1000,
        overlap_bytes: 100,
        max_chunk_bytes: 1200,
        ..ChunkerConfig::default()
    })
    .chunk(&input(&markdown))
    .expect("chunks");
    assert!(chunks.len() >= 4);
    assert!(chunks.iter().all(|c| c.text().len() <= 1200));
    // Consecutive chunks overlap.
    let first_tail = &chunks[0].text()[chunks[0].text().len().saturating_sub(50)..];
    assert!(chunks[1].text().contains(first_tail.split_whitespace().next().unwrap()));
}

#[test]
fn tables_are_kept_atomic() {
    let table = "| a | b |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |\n";
    let markdown = format!("# T\n\nBefore.\n\n{table}\nAfter.\n");
    let chunks = MarkdownChunker::new(ChunkerConfig {
        target_bytes: 16, // tiny target: without atomicity the table would split
        overlap_bytes: 0,
        max_chunk_bytes: 4096,
        ..ChunkerConfig::default()
    })
    .chunk(&input(&markdown))
    .expect("chunks");
    let with_table = chunks.iter().find(|c| c.text().contains("|---|")).expect("table chunk");
    assert!(with_table.text().contains("| 3 | 4 |"), "table must not split across chunks");
}

#[test]
fn chunk_ids_are_sequential_from_zero() {
    let chunks = MarkdownChunker::default()
        .chunk(&input("# A\n\nx\n\n# B\n\ny\n"))
        .expect("chunks");
    for (expected, chunk) in chunks.iter().enumerate() {
        assert_eq!(chunk.id().index() as usize, expected);
    }
}

#[test]
fn empty_markdown_yields_zero_chunks() {
    assert!(MarkdownChunker::default().chunk(&input("")).expect("ok").is_empty());
    assert!(MarkdownChunker::default().chunk(&input("   \n\n  ")).expect("ok").is_empty());
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p finstack-ai-ingest`
Expected: FAIL — crate empty.

- [ ] **Step 3: Implement `chunker.rs`**

Algorithm (line-based, no markdown parser dependency):
1. Split the markdown into **blocks**: consecutive lines, where a heading line (`^#{1,6} `) always starts a new block, a table run (consecutive lines starting with `|`) is one atomic block, a fenced code block (``` to ```) is one atomic block, and blank lines separate paragraph blocks.
2. Track the heading path: on a heading of level N, truncate the path to N-1 entries and push the heading text.
3. Pack blocks greedily into chunks up to `target_bytes`; a heading block forces a flush first. An atomic block larger than `max_chunk_bytes` becomes its own chunk (oversize allowed only for atomics); non-atomic oversized paragraphs are split at the last whitespace before `max_chunk_bytes`.
4. Overlap: when flushing, carry the trailing `overlap_bytes` (rounded back to a whitespace boundary) of the previous chunk into the next chunk's head — except across heading boundaries (a new section starts clean).
5. Assign `ChunkId::new(input.source(), running_index)`; store the current heading path on each chunk. Trimmed-empty chunks are dropped.

`lib.rs` for now:

```rust
//! Knowledge ingest pipeline and model-backed knowledge components.

#![warn(missing_docs)]

pub mod chunker;

#[cfg(test)]
mod tests;
```

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test -p finstack-ai-ingest`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock extensions/ingest/
git commit -m "feat: add finstack-ai-ingest crate with MarkdownChunker"
```

---

### Task 7: Graph extraction templates

**Files:**
- Create: `extensions/ingest/finstack-ai-ingest/src/template.rs`
- Create: `extensions/ingest/finstack-ai-ingest/templates/finance-core.json`
- Modify: `extensions/ingest/finstack-ai-ingest/src/lib.rs`, `src/tests.rs`

**Interfaces:**
- Produces (used by Tasks 8, 10, Python/WASM bindings):
  - `GraphExtractionTemplate::from_json(bytes: &[u8]) -> Result<Self, TemplateError>`; getters `id()`, `version()`, `name()`, `description()`, `entity_types()`, `relation_types()`, `instructions()`, `examples()`; `to_json(&self) -> Vec<u8>`
  - `TypeDef { name, description }` getters
  - `ExtractionExample { input, entities: Vec<(name, entity_type)>, relations: Vec<(from, to, relation_type)> }` getters
  - `TemplateError::{Parse { reason: String }, Invalid { reason: &'static str }}` (non_exhaustive)
  - `pub fn finance_core_template() -> GraphExtractionTemplate` (parses the checked-in asset; `expect` is fine — a broken asset is a build defect caught by tests)
  - Ceilings: instructions ≤ 16 KiB, ≤ 64 entity types, ≤ 64 relation types, ≤ 16 examples, each example input ≤ 4 KiB

- [ ] **Step 1: Write the built-in template asset**

`templates/finance-core.json`:

```json
{
  "id": "finance-core",
  "version": 1,
  "name": "Finance core",
  "description": "General financial-document entity and relation extraction.",
  "entity_types": [
    {"name": "company", "description": "A company, fund, bank, or other organization."},
    {"name": "person", "description": "A named individual."},
    {"name": "instrument", "description": "A security, loan, bond, derivative, or other financial instrument."},
    {"name": "metric", "description": "A named financial metric or KPI, e.g. revenue, EBITDA, IRR."},
    {"name": "event", "description": "A dated occurrence: acquisition, default, filing, earnings call."},
    {"name": "date", "description": "A calendar date or period."},
    {"name": "jurisdiction", "description": "A country, state, or regulatory jurisdiction."},
    {"name": "sector", "description": "An industry sector or sub-sector."},
    {"name": "currency", "description": "A currency."}
  ],
  "relation_types": [
    {"name": "owns", "description": "Ownership or equity stake."},
    {"name": "supplies", "description": "Supplier or vendor relationship."},
    {"name": "employs", "description": "Employment or officer role."},
    {"name": "issued", "description": "Issuer of an instrument."},
    {"name": "reported", "description": "Entity reported a metric or figure."},
    {"name": "occurred_on", "description": "Event tied to a date."},
    {"name": "operates_in", "description": "Entity active in a jurisdiction or sector."},
    {"name": "denominated_in", "description": "Instrument or metric denominated in a currency."}
  ],
  "instructions": "Extract only entities and relations stated in the text. Prefer full legal names. Normalize metric names to lowercase. Do not infer relations that are not explicit.",
  "examples": []
}
```

- [ ] **Step 2: Write failing tests**

Append to `src/tests.rs`:

```rust
use crate::template::{GraphExtractionTemplate, TemplateError};

#[test]
fn finance_core_asset_loads() {
    let template = crate::template::finance_core_template();
    assert_eq!(template.id().as_ref(), "finance-core");
    assert_eq!(template.version(), 1);
    assert_eq!(template.entity_types().len(), 9);
}

#[test]
fn from_json_round_trips() {
    let template = crate::template::finance_core_template();
    let json = template.to_json();
    let back = GraphExtractionTemplate::from_json(&json).expect("round trip");
    assert_eq!(back.id(), template.id());
}

#[test]
fn rejects_unknown_fields_missing_types_and_duplicates() {
    let unknown = br#"{"id":"x","version":1,"name":"n","description":"d","entity_types":[{"name":"a","description":"d"}],"relation_types":[],"instructions":"i","examples":[],"extra":true}"#;
    assert!(matches!(
        GraphExtractionTemplate::from_json(unknown),
        Err(TemplateError::Parse { .. })
    ));
    let no_entities = br#"{"id":"x","version":1,"name":"n","description":"d","entity_types":[],"relation_types":[],"instructions":"i","examples":[]}"#;
    assert!(matches!(
        GraphExtractionTemplate::from_json(no_entities),
        Err(TemplateError::Invalid { reason: "no_entity_types" })
    ));
    let duplicate = br#"{"id":"x","version":1,"name":"n","description":"d","entity_types":[{"name":"a","description":"d"},{"name":"a","description":"d"}],"relation_types":[],"instructions":"i","examples":[]}"#;
    assert!(matches!(
        GraphExtractionTemplate::from_json(duplicate),
        Err(TemplateError::Invalid { reason: "duplicate_type_name" })
    ));
}

#[test]
fn rejects_oversized_instructions() {
    let big = "x".repeat(17 * 1024);
    let json = format!(
        r#"{{"id":"x","version":1,"name":"n","description":"d","entity_types":[{{"name":"a","description":"d"}}],"relation_types":[],"instructions":"{big}","examples":[]}}"#
    );
    assert!(matches!(
        GraphExtractionTemplate::from_json(json.as_bytes()),
        Err(TemplateError::Invalid { reason: "instructions_too_large" })
    ));
}
```

- [ ] **Step 3: Run to verify failure, then implement `template.rs`**

Run: `cargo test -p finstack-ai-ingest template` → FAIL. Then implement: serde structs with `#[serde(deny_unknown_fields)]`, `from_json` = `serde_json::from_slice` (`Parse`) followed by the validation rules from the Interfaces block (`Invalid` with the exact reason strings the tests assert). `finance_core_template()` uses `include_bytes!("../templates/finance-core.json")`.

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test -p finstack-ai-ingest`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add extensions/ingest/finstack-ai-ingest/
git commit -m "feat: add graph extraction templates with finance-core asset"
```

---

### Task 8: `model_call` adapter + `ModelGraphExtractor`

**Files:**
- Create: `extensions/ingest/finstack-ai-ingest/src/model_call.rs`
- Create: `extensions/ingest/finstack-ai-ingest/src/extractor.rs`
- Modify: `extensions/ingest/finstack-ai-ingest/src/lib.rs`, `src/tests.rs`

**Interfaces:**
- Consumes: `Model` port (`finstack_ai_runtime::Model`), `GraphExtractionTemplate` (Task 7), `Chunk` (Task 6), graph types (Task 2).
- Produces (used by Tasks 9–10):
  - `model_call::complete_text(model: &Arc<dyn Model>, model_name: &ModelName, system: &str, user: &str, max_output_tokens: u32, deadline: Option<Timestamp>) -> PortFuture<Result<String, ModelCallError>>` — the ONLY place in the crate that touches `ModelRequest`/`ModelEventStream`
  - `trait GraphExtractor: PortObject { fn extract(&self, ctx: ExtractContext, chunks: Arc<[Chunk]>) -> PortFuture<Result<GraphFragment, ExtractError>>; }`
  - `ExtractContext::new(collection: CollectionId, source: SourceId, template: Arc<GraphExtractionTemplate>, deadline: Option<Timestamp>)`; getters
  - `ModelGraphExtractor::new(model: Arc<dyn Model>, model_name: ModelName)`, `.with_batch_size(usize)` (default 8 chunks per model call), `.with_max_output_tokens(u32)` (default 4096)
  - `ExtractError::{Model { code: Arc<str>, message: Arc<str> }, OutputInvalid { reason: &'static str }}` (non_exhaustive)
  - merge helper `pub(crate) fn merge_fragments(collection, source, parts: Vec<GraphFragment>) -> GraphFragment` — case-insensitive `(name, entity_type)` entity merge, `(from, to, relation_type)` relation merge, deduped `source_chunks` unions

**`model_call.rs` is an adapter boundary.** The exact construction of a committed `ModelRequest` and consumption of `ModelEventStream` must be read from the codebase, not invented:
- Read `crates/finstack-ai-runtime/src/ports/model/request.rs` (`ModelRequestDraft` and its `canonical_bytes`), `context.rs` (`ModelRequest`), and `stream.rs` (`ModelEventStream` items).
- Read how the test fakes in `crates/finstack-ai/src/agent/tests.rs` implement `Model` and what a minimal valid `ModelRequest` looks like there; mirror that construction for a detached (non-run) call. If `ModelRequest` requires run-scoped identifiers that cannot be fabricated outside the runtime, add a `pub(crate) fn detached_request(…)` helper that fills them with the documented detached values used by those tests.
- Collect the stream's text deltas into one `String`; map stream errors to `ModelCallError::Model`.
- Do NOT redesign the extractor API around upstream difficulties — adapt inward here, exactly like the document plan's anydoc adapters.

- [ ] **Step 1: Write failing extractor tests (scripted Model double)**

Append to `src/tests.rs`. Build a `ScriptedModel` test double implementing `Model` that returns canned text responses in order (copy the fake-Model skeleton from `crates/finstack-ai/src/agent/tests.rs` — copy, don't import). The canned response for a successful extraction call:

```rust
const EXTRACTION_RESPONSE: &str = r#"{"entities":[{"name":"Acme Corp","entity_type":"company","description":"Widget maker"},{"name":"Q1 revenue","entity_type":"metric","description":"Quarterly revenue"}],"relations":[{"from":"Acme Corp","to":"Q1 revenue","relation_type":"reported","description":"Acme reported Q1 revenue"}]}"#;

#[test]
fn extractor_parses_structured_output_into_fragment() {
    let model = Arc::new(ScriptedModel::with_responses(vec![EXTRACTION_RESPONSE.to_owned()]));
    let extractor = ModelGraphExtractor::new(model, test_model_name());
    let chunks: Arc<[Chunk]> = Arc::from([chunk_fixture(0, "Acme Corp reported Q1 revenue of $5m.")]);
    let fragment = block_on(extractor.extract(extract_ctx(), chunks)).expect("fragment");
    assert_eq!(fragment.entities().len(), 2);
    assert_eq!(fragment.relations().len(), 1);
    // Template provenance stamped on every entity/relation metadata.
    let entity_meta = fragment.entities()[0].metadata();
    assert_metadata_has(entity_meta, finstack_ai_runtime::META_TEMPLATE_ID, "finance-core");
}

#[test]
fn unparseable_output_is_output_invalid_not_panic() {
    let model = Arc::new(ScriptedModel::with_responses(vec!["not json at all".to_owned()]));
    let extractor = ModelGraphExtractor::new(model, test_model_name());
    let chunks: Arc<[Chunk]> = Arc::from([chunk_fixture(0, "text")]);
    let error = block_on(extractor.extract(extract_ctx(), chunks)).expect_err("invalid output");
    assert!(matches!(error, ExtractError::OutputInvalid { .. }));
}

#[test]
fn entities_outside_template_vocabulary_are_dropped() {
    let response = r#"{"entities":[{"name":"X","entity_type":"animal","description":"d"}],"relations":[]}"#;
    let model = Arc::new(ScriptedModel::with_responses(vec![response.to_owned()]));
    let extractor = ModelGraphExtractor::new(model, test_model_name());
    let fragment = block_on(extractor.extract(extract_ctx(), Arc::from([chunk_fixture(0, "t")])))
        .expect("fragment");
    assert!(fragment.entities().is_empty());
}

#[test]
fn merge_fragments_dedups_case_insensitively() {
    // Exercise merge_fragments directly with two parts naming "ACME"/"acme".
    // Assert one merged entity whose source_chunks is the union.
}
```

Fill in the helpers (`chunk_fixture`, `extract_ctx` using `finance_core_template()`, `test_model_name` — read `ModelName`'s constructor, `assert_metadata_has` comparing via `serde_json::Value`), and write the `merge_fragments` test body concretely.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p finstack-ai-ingest extractor`
Expected: FAIL.

- [ ] **Step 3: Implement `extractor.rs` (and `model_call.rs` per its adapter notes)**

Prompt assembly (const scaffolding in the crate):

```rust
const EXTRACTION_SYSTEM: &str = "You extract a knowledge graph from document text. \
Respond with ONLY a JSON object of the form \
{\"entities\":[{\"name\":…,\"entity_type\":…,\"description\":…}],\
\"relations\":[{\"from\":…,\"to\":…,\"relation_type\":…,\"description\":…}]}. \
Use only the entity and relation types listed. No prose, no code fences.";
```

User message per batch: template instructions + rendered type lists (`- name: description` lines) + rendered examples (if any) + the batch's chunk texts labeled `[chunk {index}]`. Split `chunks` into batches of `batch_size`; call `model_call::complete_text` per batch (sequential in this task — the pipeline provides concurrency); parse each response (strip a single leading/trailing ``` fence if present, then `serde_json::from_str` into local serde structs); drop entities whose `entity_type` (case-insensitive) is not in the template and relations whose `relation_type` is not in the template or whose endpoints were dropped; attach the batch's chunk ids as `source_chunks`; stamp `META_TEMPLATE_ID`/`META_TEMPLATE_VERSION` metadata; `merge_fragments` at the end.

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test -p finstack-ai-ingest`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add extensions/ingest/finstack-ai-ingest/
git commit -m "feat: add ModelGraphExtractor with model_call adapter"
```

---

### Task 9: `ModelReranker` + `CommunityBuilder`

**Files:**
- Create: `extensions/ingest/finstack-ai-ingest/src/reranker.rs`
- Create: `extensions/ingest/finstack-ai-ingest/src/community.rs`
- Modify: `extensions/ingest/finstack-ai-ingest/src/lib.rs`, `src/tests.rs`

**Interfaces:**
- Consumes: `Reranker` port + `RerankEntry`/`RerankError` (Task 3), `GraphStore` + `Community`/`CommunityQuery` (Tasks 2–3), `EmbeddingModel` (Task 1), `model_call` (Task 8).
- Produces:
  - `ModelReranker::new(model: Arc<dyn Model>, model_name: ModelName)`, `.with_max_candidates(u32)` (default 64) — implements `Reranker`
  - `CommunityBuilder::new(model: Arc<dyn Model>, model_name: ModelName)`, `.with_embedder(Arc<dyn EmbeddingModel>)`, `.with_seed(u64)` (default 0), `.with_max_communities(u32)` (default 64), `.with_max_entities_per_summary(u32)` (default 32)
  - `CommunityBuilder::build(&self, collection: CollectionId, graph: Arc<dyn GraphStore>) -> PortFuture<Result<CommunityReport, CommunityError>>` with `CommunityReport { communities_built: u32, entities_clustered: u32, failures: Vec<String> }` (getters)
  - `pub(crate) fn label_propagation(adjacency: &BTreeMap<String, BTreeSet<String>>, seed: u64, max_rounds: u32) -> BTreeMap<String, u32>` — node key → community label; deterministic

- [ ] **Step 1: Write failing tests**

Append to `src/tests.rs`:

```rust
#[test]
fn reranker_parses_ranked_indices() {
    let model = Arc::new(ScriptedModel::with_responses(vec![
        r#"{"ranking":[{"index":2,"score":0.9},{"index":0,"score":0.4}]}"#.to_owned(),
    ]));
    let reranker = ModelReranker::new(model, test_model_name());
    let entries = block_on(reranker.rerank(
        RerankCallContext::detached(None),
        Arc::from("which supplier?"),
        Arc::from([Arc::from("a"), Arc::from("b"), Arc::from("c")] as [Arc<str>; 3]),
    ))
    .expect("entries");
    assert_eq!(entries[0].index(), 2);
    assert_eq!(entries.len(), 2); // dropped candidate 1 = rejected
}

#[test]
fn reranker_rejects_out_of_range_indices() {
    let model = Arc::new(ScriptedModel::with_responses(vec![
        r#"{"ranking":[{"index":9,"score":0.9}]}"#.to_owned(),
    ]));
    let reranker = ModelReranker::new(model, test_model_name());
    let error = block_on(reranker.rerank(
        RerankCallContext::detached(None),
        Arc::from("q"),
        Arc::from([Arc::from("a")] as [Arc<str>; 1]),
    ))
    .expect_err("invalid");
    assert!(matches!(error, RerankError::InvalidOutput { .. }));
}

#[test]
fn label_propagation_is_deterministic_and_finds_two_clusters() {
    // Two triangles joined by nothing: {a,b,c} and {x,y,z}.
    let mut adjacency: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (n, peers) in [
        ("a", ["b", "c"]), ("b", ["a", "c"]), ("c", ["a", "b"]),
        ("x", ["y", "z"]), ("y", ["x", "z"]), ("z", ["x", "y"]),
    ] {
        adjacency.insert(n.to_owned(), peers.iter().map(|p| (*p).to_owned()).collect());
    }
    let first = label_propagation(&adjacency, 7, 20);
    let second = label_propagation(&adjacency, 7, 20);
    assert_eq!(first, second, "deterministic given a seed");
    assert_eq!(first["a"], first["b"]);
    assert_eq!(first["x"], first["z"]);
    assert_ne!(first["a"], first["x"]);
}

#[test]
fn community_builder_summarizes_and_replaces() {
    let store = MemoryKnowledgeStore::new(); // dev-dep on the memory backend crate
    let collection = CollectionId::default_collection();
    let source = SourceId::new(digest_of(b"doc"));
    // Seed a graph with one connected component.
    block_on(store.upsert(fragment(&collection, source,
        &[("Acme", "company"), ("Widget Ltd", "company")],
        &[("Acme", "Widget Ltd", "owns")],
    ))).expect("seed");
    let model = Arc::new(ScriptedModel::with_responses(vec![
        r#"{"title":"Acme group","summary":"Acme owns Widget Ltd."}"#.to_owned(),
    ]));
    let builder = CommunityBuilder::new(model, test_model_name());
    let report = block_on(builder.build(collection.clone(), Arc::new(store.clone())))
        .expect("report");
    assert_eq!(report.communities_built(), 1);
    let communities = block_on(store.communities(
        CommunityQuery::new(collection, CommunityLookup::All),
    ))
    .expect("list");
    assert_eq!(communities[0].title().as_ref(), "Acme group");
}
```

Add `finstack-ai-knowledge-memory` as a `[dev-dependencies]` path entry in the engine crate's `Cargo.toml` for this test, and copy the `fragment(…)` helper from that crate's tests into this crate's `tests.rs` (helpers are copied per crate, never shared). Use fully-qualified `GraphStore::upsert(&store, …)` here too — see Task 5's method-resolution note.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p finstack-ai-ingest reranker community label_prop`
Expected: FAIL.

- [ ] **Step 3: Implement**

`reranker.rs`: prompt = const system ("Rank the numbered candidates by relevance to the query. Respond ONLY with JSON {\"ranking\":[{\"index\":…,\"score\":…}]}.") + user message listing `[0] candidate…` lines, truncated to `max_candidates`; parse; validate every index < candidates.len() and no duplicates (`InvalidOutput` otherwise); map to `RerankEntry` in response order.

`community.rs`:
1. Fetch the merged graph: `GraphStore::query` cannot enumerate everything, so add the enumeration via `export` — stream the collection's `GraphExportRecord`s, collect entities/relations (merged by the same key as Task 5 — reuse `merge_fragments` on a synthetic single collection pass).
2. Build `adjacency` over merged entity keys from relations (undirected).
3. `label_propagation`: initialize each node's label to its index in BTreeMap order; iterate up to `max_rounds` or convergence; each node (in a seed-shuffled order — xorshift over the seed, no `rand` dependency) adopts the most frequent label among neighbors, ties → smallest label.
4. Group into communities (cap at `max_communities`, largest first), one `model_call::complete_text` summary each (entity names + types + descriptions, capped at `max_entities_per_summary`); parse `{title, summary}`; per-community failures collected into `report.failures`, not fatal.
5. Optional embedder: embed each summary (`Document` kind) into `Community::with_embedding`.
6. `replace_communities` once at the end. Community ids: `"c{index}"` in deterministic order.

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test -p finstack-ai-ingest`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add extensions/ingest/finstack-ai-ingest/
git commit -m "feat: add ModelReranker and CommunityBuilder"
```

---

### Task 10: `IngestPipeline`

**Files:**
- Create: `extensions/ingest/finstack-ai-ingest/src/pipeline.rs`
- Create: `extensions/ingest/finstack-ai-ingest/src/source.rs`
- Modify: `extensions/ingest/finstack-ai-ingest/src/lib.rs`, `src/tests.rs`

**Interfaces:**
- Consumes: everything from Tasks 1–8; `finstack_ai_tools_document::parser::{parse, DocumentLimits}`.
- Produces (used by the feeder, e2e lane, bindings):
  - `IngestPipeline::builder() -> IngestPipelineBuilder` with `.collection(CollectionId)` (default `default_collection()`), `.limits(DocumentLimits)`, `.ingest_limits(IngestLimits)`, `.chunker(Arc<dyn Chunker>)` (default `MarkdownChunker::default()`), `.catalog(Arc<dyn SourceCatalog>)` (required), `.source_store(Arc<dyn SourceStore>)` (optional raw archive), `.embedding(Arc<dyn EmbeddingModel>, Arc<dyn ChunkIndex>)`, `.keyword_only(Arc<dyn ChunkIndex>)`, `.graph(Arc<dyn GraphExtractor>, Arc<dyn GraphStore>, templates: Vec<Arc<GraphExtractionTemplate>>, default_template: &str)`, `.clock(Arc<dyn Fn() -> Timestamp + Send + Sync>)` (default: wall clock via the kernel `Timestamp` now-constructor; tests inject a fixed clock), `.build() -> Result<Arc<IngestPipeline>, IngestError>`
  - `IngestLimits { max_chunks_per_document: u32 (2048), max_concurrent_embed_batches: u32 (4), max_concurrent_extract_calls: u32 (2), per_call_deadline: Option<Timestamp-delta — use whatever duration type EmbeddingCallContext's deadline uses> }` + `Default`
  - `IngestSource::{Bytes { bytes: Vec<u8>, media_type_hint: Option<String>, name: Option<Arc<str>> }, Markdown { text: Arc<str>, source_id: SourceId, name: Option<Arc<str>> }}` — the `Artifact` variant is added in Task 14 with the feeder (it needs `ArtifactStore` resolution; keep this task store-free)
  - `IngestOptions::default()` with `.with_graph_template(&str)`, `.with_force(bool)`, `.with_embed_entities(bool)` (default true), `.with_options(Metadata)`
  - `pipeline.ingest(source) -> PortFuture<Result<IngestReport, IngestError>>` and `pipeline.ingest_with(source, options)`
  - `pipeline.reingest(source_id: SourceId, options: IngestOptions) -> PortFuture<Result<IngestReport, IngestError>>` — fetches archived bytes from the configured `SourceStore` and runs a normal ingest over them; `IngestError::RawUnavailable` when no store is configured or the source is not archived
  - `IngestReport` getters: `source_id()`, `status()` (`SourceStatus`), `skipped_unchanged()`, `chunks_total()`, `chunks_embedded()`, `chunks_reused()`, `chunks_deleted()`, `entities_extracted()`, `relations_extracted()`, `failures()` (`&[IngestFailure]` — `{ stage: Arc<str>, reason: Arc<str> }`), `elapsed_ms()`
  - `IngestError::{Build { reason: &'static str }, Parse { message: String }, UnknownTemplate { id: Arc<str> }, RawUnavailable, Catalog(KnowledgeStoreError)}` (non_exhaustive)

- [ ] **Step 1: Write failing pipeline tests**

Append to `src/tests.rs`. Test doubles needed here (define in `tests.rs`):
- `FakeEmbeddingModel`: `descriptor()` reports 8 dimensions; `embed` hashes each input (`DefaultHasher`), expands the hash to 8 f32s, L2-normalizes. Deterministic: same text → same vector.
- `ScriptedGraphExtractor`: implements `GraphExtractor`, returns a canned `GraphFragment` built from the ctx's collection/source, records call count in an `Arc<AtomicU32>`.

```rust
#[test]
fn ingest_markdown_end_to_end_updates_all_stores_and_catalog() {
    let store = MemoryKnowledgeStore::new();
    let pipeline = IngestPipeline::builder()
        .catalog(Arc::new(store.clone()))
        .embedding(Arc::new(FakeEmbeddingModel::new()), Arc::new(store.clone()))
        .graph(
            Arc::new(ScriptedGraphExtractor::new()),
            Arc::new(store.clone()),
            vec![Arc::new(crate::template::finance_core_template())],
            "finance-core",
        )
        .clock(fixed_clock())
        .build()
        .expect("pipeline");
    let report = block_on(pipeline.ingest(IngestSource::Markdown {
        text: Arc::from("# Report\n\nAcme revenue rose.\n\n## Detail\n\nMore text here.\n"),
        source_id: SourceId::new(digest_of(b"report-v1")),
        name: Some(Arc::from("report.md")),
    }))
    .expect("report");
    assert!(report.chunks_total() >= 2);
    assert_eq!(report.chunks_embedded(), report.chunks_total());
    assert_eq!(report.status(), &SourceStatus::Indexed);
    // Catalog row exists with chunk digests recorded.
    let record = block_on(store.get(CollectionId::default_collection(), report.source_id()))
        .expect("get").expect("some");
    assert_eq!(record.chunk_count(), report.chunks_total());
    // Chunks searchable.
    let hits = block_on(store.search(
        ChunkQuery::new(
            CollectionId::default_collection(),
            SearchMode::Keyword { text: Arc::from("revenue") },
            8,
        ).expect("query"),
    )).expect("hits");
    assert!(!hits.is_empty());
}

#[test]
fn unchanged_source_short_circuits_and_force_overrides() {
    let store = MemoryKnowledgeStore::new();
    let embedder = Arc::new(FakeEmbeddingModel::new()); // exposes call counter
    let pipeline = pipeline_with(&store, embedder.clone());
    let source = IngestSource::Markdown {
        text: Arc::from("# A\n\nsame content\n"),
        source_id: SourceId::new(digest_of(b"same content")),
        name: None,
    };
    block_on(pipeline.ingest(source.clone())).expect("first");
    let calls_after_first = embedder.calls();
    let second = block_on(pipeline.ingest(source.clone())).expect("second");
    assert!(second.skipped_unchanged());
    assert_eq!(embedder.calls(), calls_after_first, "no re-embedding");
    let forced = block_on(pipeline.ingest_with(source, IngestOptions::default().with_force(true)))
        .expect("forced");
    assert!(!forced.skipped_unchanged());
}

#[test]
fn changed_source_diffs_chunks_reusing_unchanged_ones() {
    let store = MemoryKnowledgeStore::new();
    let embedder = Arc::new(FakeEmbeddingModel::new());
    let pipeline = pipeline_with(&store, embedder.clone());
    // v1: sections A and B. v2: A unchanged, B edited, C appended.
    // NOTE: same source identity across versions — model a named document whose
    // content changed. SourceId is the CONTENT digest, so v2 has a new SourceId;
    // the diff keys off the catalog lookup by... 
    // — see Step 3 note: catalog lookup for diffing is BY NAME within the
    // collection when the source id differs, falling back to full ingest when
    // no prior record matches.
}
```

**STOP — design note the implementer must follow (spec decision 14 refinement discovered while planning):** `SourceId` is the content digest, so a *changed* document gets a *new* `SourceId`. The chunk-diff therefore keys off the **previous catalog record found by document name** (`SourceListQuery` name match within the collection): if a prior record with the same `name` and a different `source` exists, diff against its `chunk_digests`, reuse matching chunks' vectors via `get_chunks` on the OLD source (positional: same digest at same index), write new records under the NEW `SourceId`, then `delete_by_source` the old source in both stores and `delete` its catalog row. If no named prior record exists (or the source has no name), every chunk is new. The `unchanged_source_short_circuits` case is the same-SourceId catalog hit. Write `changed_source_diffs_chunks_reusing_unchanged_ones` to assert: embedder call count for v2 counts only changed/new chunks; old source's records are gone; new source's records searchable; report `chunks_reused()` matches.

Also add tests for: raw archiving — with `.source_store(Arc::new(store.clone()))` configured, a successful ingest archives the exact input bytes (`SourceStore::get` returns them; catalog `raw_stored() == true`) and a PARSE FAILURE still archives them (ingest returns `IngestError::Parse` but `exists` is true); `reingest` — after ingesting with store configured, change the pipeline's chunker config (build a second pipeline over the same stores), `reingest(source_id, default)` re-chunks from archived bytes keeping the same `SourceId`, and `reingest` without a source store (or for an unarchived id) → `IngestError::RawUnavailable`; `requires_ocr` markdown-empty path via `IngestSource::Bytes` with the scanned-PDF fixture (`include_bytes!("../../../fixtures/documents/scanned.pdf")` — status `MetadataOnly`, zero chunks); parse failure fatal (`corrupt.bin` → `IngestError::Parse`); unknown template id → `IngestError::UnknownTemplate` before any work (extractor call count stays 0); entity embeddings stamped when both branches configured (entities in store have embeddings; `with_embed_entities(false)` disables); per-chunk embed failure collected not fatal (make `FakeEmbeddingModel::fail_on(text_substring)`); build error when no branch configured; build error when both `.embedding` and `.keyword_only` set.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p finstack-ai-ingest pipeline`
Expected: FAIL.

- [ ] **Step 3: Implement `source.rs` + `pipeline.rs`**

`source.rs`: `IngestSource` enum + `resolve(self, limits: &DocumentLimits) -> Result<ResolvedInput, IngestError>` where `ResolvedInput { markdown: Arc<str>, source_id: SourceId, name: Option<Arc<str>>, format: Arc<str>, page_count: Option<u32>, requires_ocr: bool }`. `Bytes` runs `parser::parse` (map `DocumentParseError` → `IngestError::Parse`) and digests the input bytes with the kernel digest; `Markdown` passes through with format `"markdown"`.

`pipeline.rs` flow (single async fn, stages in order):
1. Resolve template selection (error before any work).
2. Resolve source → `ResolvedInput`. For `Bytes`/`Artifact` sources with a configured `SourceStore`: archive the raw bytes + media type BEFORE calling the parser (digest already computed); for `Markdown`, archive the text as `text/markdown`. Archive errors are collected as failures, not fatal. On parse failure AFTER archiving, still return `IngestError::Parse` (the archive is the point — evidence survives).
3. Catalog lookup: same `SourceId` present and `!force` → return `skipped_unchanged` report. Else find prior record by name (the Step 1 design note).
4. `requires_ocr` or empty markdown → write `MetadataOnly` catalog record, return.
5. Chunk (`ChunkError` → failures + `PartialFailure` status if zero chunks emerge from non-empty markdown; `max_chunks_per_document` truncates with a recorded failure entry).
6. Chunk branch and graph branch run concurrently (`futures_util::future::join`):
   - **Chunk branch**: diff against prior record; batch changed chunks by the embedder's `max_batch`; up to `max_concurrent_embed_batches` in flight via `futures_util::stream::iter(batches).map(embed_one).buffer_unordered(n)`; build `ChunkRecord`s (stamp `META_EMBEDDING_MODEL`, `META_DOC_NAME`, `META_DOC_FORMAT`, `META_HEADING_PATH` metadata); reuse unchanged chunks' records (rewritten under the new SourceId with old vectors); one `upsert`; `delete_chunks`/`delete_by_source` for removals/old source. `keyword_only` skips embedding, records have no vectors.
   - **Graph branch**: split chunks into extractor calls (extractor batches internally; pipeline enforces `max_concurrent_extract_calls` if it fans out — v1 calls `extract` once with all chunks and lets the extractor batch); on success, optionally embed entity descriptions (batched through the same embedder), `delete_by_source`(old + new) then `upsert` the fragment.
7. Write the catalog record LAST (chunk digests, counts, template id/version, `raw_stored` = whether Step 2's archive succeeded, status: `Indexed`, or `PartialFailure` when `failures` is non-empty), delete the superseded named record — and delete the superseded source's archived bytes from the `SourceStore` (it is retired everywhere: index, graph, catalog, archive). `reingest` = `SourceStore::get` → synthesize `IngestSource::Bytes` from the stored media type/bytes (or `Markdown` for `text/markdown`) → run the normal flow with `force` semantics for the same-`SourceId` short-circuit.
8. Fill `IngestReport`. `elapsed_ms` from the injected clock (start/end delta).

`lib.rs` final module list: `chunker`, `template`, `model_call` (pub(crate)), `extractor`, `reranker`, `community`, `pipeline`, `source`, re-exporting the public names from the Interfaces blocks.

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test -p finstack-ai-ingest`
Expected: PASS.

- [ ] **Step 5: Verify wasm**

Run: `cargo check -p finstack-ai-ingest --target wasm32-unknown-unknown`
Expected: clean. (`futures_util` combinators are wasm-fine; no tokio anywhere in this crate.)

- [ ] **Step 6: Commit**

```bash
git add extensions/ingest/finstack-ai-ingest/
git commit -m "feat: add IngestPipeline with incremental re-ingest"
```

---

### Task 11: `finstack-ai-knowledge-sqlite`

**Files:**
- Create: `extensions/stores/finstack-ai-knowledge-sqlite/Cargo.toml`
- Create: `extensions/stores/finstack-ai-knowledge-sqlite/src/lib.rs`
- Create: `extensions/stores/finstack-ai-knowledge-sqlite/src/schema.rs`
- Create: `extensions/stores/finstack-ai-knowledge-sqlite/src/chunk_index.rs`
- Create: `extensions/stores/finstack-ai-knowledge-sqlite/src/graph_store.rs`
- Create: `extensions/stores/finstack-ai-knowledge-sqlite/src/catalog.rs`
- Create: `extensions/stores/finstack-ai-knowledge-sqlite/src/source_store.rs` (SourceStore over `source_blobs`; same digest-verification contract as the memory backend)
- Create: `extensions/stores/finstack-ai-knowledge-sqlite/src/tests.rs`
- Modify: root `Cargo.toml` (workspace member)

**Interfaces:**
- Consumes: all port traits/types; `rusqlite` (workspace dep, bundled — FTS5 verified available in `libsqlite3-sys` build).
- Produces: `SqliteKnowledgeStore::open(path: &Path) -> Result<Self, KnowledgeStoreError>` and `SqliteKnowledgeStore::open_in_memory()`; implements all three traits. **Follow the worker pattern of `extensions/stores/finstack-ai-store-sqlite/src/{worker.rs,store.rs}`** — read those files first and copy the structure: a dedicated thread owning the `Connection`, commands over a channel, `PortFuture` resolved via oneshot. Copy, don't invent; rusqlite `Connection` is `!Sync` and must not be wrapped in a Mutex-across-await.

**Schema (`schema.rs`, executed on open):**

```sql
CREATE TABLE IF NOT EXISTS chunks (
  collection TEXT NOT NULL,
  source_id BLOB NOT NULL,
  chunk_index INTEGER NOT NULL,
  chunk_digest BLOB NOT NULL,
  vector BLOB,                      -- little-endian f32s; NULL for keyword-only
  text TEXT NOT NULL,
  metadata TEXT NOT NULL,           -- canonical Metadata JSON
  PRIMARY KEY (collection, source_id, chunk_index)
);
CREATE VIRTUAL TABLE IF NOT EXISTS chunks_fts USING fts5(
  text, content='chunks', content_rowid='rowid'
);
-- Keep FTS in sync with triggers (copy the canonical fts5 external-content
-- trigger trio: AFTER INSERT / AFTER DELETE / AFTER UPDATE on chunks).
CREATE TABLE IF NOT EXISTS collections (
  collection TEXT PRIMARY KEY,
  dimensions INTEGER              -- NULL until first vector insert
);
CREATE TABLE IF NOT EXISTS graph_entities (
  collection TEXT NOT NULL, source_id BLOB NOT NULL,
  name TEXT NOT NULL, name_key TEXT NOT NULL, entity_type TEXT NOT NULL,
  description TEXT NOT NULL, embedding BLOB, source_chunks TEXT NOT NULL,
  metadata TEXT NOT NULL,
  PRIMARY KEY (collection, source_id, name_key, entity_type)
);
CREATE INDEX IF NOT EXISTS idx_entities_lookup ON graph_entities (collection, name_key);
CREATE TABLE IF NOT EXISTS graph_relations (
  collection TEXT NOT NULL, source_id BLOB NOT NULL,
  from_key TEXT NOT NULL, to_key TEXT NOT NULL, relation_type TEXT NOT NULL,
  description TEXT NOT NULL, source_chunks TEXT NOT NULL, metadata TEXT NOT NULL,
  PRIMARY KEY (collection, source_id, from_key, to_key, relation_type)
);
CREATE TABLE IF NOT EXISTS graph_communities (
  collection TEXT NOT NULL, id TEXT NOT NULL, level INTEGER NOT NULL,
  title TEXT NOT NULL, summary TEXT NOT NULL, entity_names TEXT NOT NULL,
  embedding BLOB, metadata TEXT NOT NULL,
  PRIMARY KEY (collection, id)
);
CREATE TABLE IF NOT EXISTS source_blobs (
  collection TEXT NOT NULL, source_id BLOB NOT NULL,
  media_type TEXT NOT NULL, bytes BLOB NOT NULL,
  PRIMARY KEY (collection, source_id)
);
CREATE TABLE IF NOT EXISTS sources (
  collection TEXT NOT NULL, source_id BLOB NOT NULL,
  name TEXT, format TEXT NOT NULL, page_count INTEGER,
  ingested_at INTEGER NOT NULL, chunk_count INTEGER NOT NULL,
  chunk_digests BLOB NOT NULL,     -- concatenated digests
  template_id TEXT, template_version INTEGER,
  raw_stored INTEGER NOT NULL DEFAULT 0,
  status TEXT NOT NULL,            -- serde_json of SourceStatus
  metadata TEXT NOT NULL,
  PRIMARY KEY (collection, source_id)
);
```

Per-source graph rows ARE the attribution ledger (same design as the memory backend); the merged view is materialized per query in Rust by reusing the same merge logic — extract the memory backend's merge into a shared shape by **copying** the merge function into this crate (crates don't share test/impl helpers; keep both annotated with a comment naming the other copy).

- [ ] **Step 1: Port the memory backend's test suite**

Copy `extensions/stores/finstack-ai-knowledge-memory/src/tests.rs` wholesale, replace `MemoryKnowledgeStore::new()` with `SqliteKnowledgeStore::open_in_memory()`, and add two sqlite-specific cases: (a) `open` on a temp file path persists across re-open (ingest, drop, re-open, search still hits); (b) memory↔sqlite export/import round-trip — export from a populated `MemoryKnowledgeStore` (dev-dependency), import into sqlite, assert identical search results for one semantic and one keyword query, then the reverse direction.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p finstack-ai-knowledge-sqlite`
Expected: FAIL — crate empty.

- [ ] **Step 3: Implement**

Semantic search: `SELECT` the collection's rows with non-NULL vectors, decode BLOB → `Vec<f32>`, cosine in Rust, rank (brute force per spec). Keyword: `SELECT rowid … FROM chunks_fts WHERE chunks_fts MATCH ?` — build the MATCH string by quoting each whitespace token (`"token"`) joined with `OR` to avoid FTS syntax injection from query text; rank with `bm25(chunks_fts)` (ascending = better; negate for the descending `ChunkHit` score). Hybrid: both queries + private RRF (k=60). Dimension check via the `collections` table. Export/import: page through rows with `LIMIT/OFFSET` inside the worker, emit as one collected `Vec` wrapped in `stream::iter` (bounded memory is a follow-up; note it in the module doc).

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test -p finstack-ai-knowledge-sqlite`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock extensions/stores/finstack-ai-knowledge-sqlite/
git commit -m "feat: add SQLite knowledge backend with FTS5 keyword search"
```

---

### Task 12: `finstack-ai-tools-knowledge` toolset

**Files:**
- Create: `extensions/toolsets/finstack-ai-tools-knowledge/Cargo.toml` (same template as the document toolset: runtime + kernel + futures-util + serde + serde_json + thiserror; dev-dep `finstack-ai-knowledge-memory`)
- Create: `extensions/toolsets/finstack-ai-tools-knowledge/src/lib.rs`
- Create: `extensions/toolsets/finstack-ai-tools-knowledge/src/toolset.rs`
- Create: `extensions/toolsets/finstack-ai-tools-knowledge/src/search.rs` (knowledge_search execution)
- Create: `extensions/toolsets/finstack-ai-tools-knowledge/src/tests.rs`
- Modify: root `Cargo.toml` (workspace member)

**Interfaces:**
- Consumes: all knowledge/embedding ports; `RrfFusion` (default fusion).
- Produces:
  - `KnowledgeToolset::try_new(collection: CollectionId, catalog: Arc<dyn SourceCatalog>) -> Result<Self, KnowledgeToolsetError>`
  - `.with_search(embedder: Option<Arc<dyn EmbeddingModel>>, index: Arc<dyn ChunkIndex>)` (embedder `None` = keyword-only search)
  - `.with_graph(graph: Arc<dyn GraphStore>)`, `.with_reranker(Arc<dyn Reranker>)`, `.with_fusion(Arc<dyn FusionStrategy>)`
  - Error-code consts: `KNOWLEDGE_INVALID_ARGUMENTS = "knowledge_invalid_arguments"`, `KNOWLEDGE_STORE_UNAVAILABLE = "knowledge_store_unavailable"`, `KNOWLEDGE_EMBEDDING_FAILED = "knowledge_embedding_failed"`, `KNOWLEDGE_QUERY_FAILED = "knowledge_query_failed"`, `KNOWLEDGE_RERANK_FAILED = "knowledge_rerank_failed"`
  - `tools()` contents depend on construction: `list_sources` always; `knowledge_search` with `.with_search`; `graph_query` + `knowledge_overview` with `.with_graph` (`graph_query`'s `semantic_query` arg and `knowledge_overview`'s `query` arg additionally require the embedder — validated at call time with `KNOWLEDGE_INVALID_ARGUMENTS` naming the missing dependency)

**Follow the document toolset pattern for ALL harness glue** — `ToolsetDescriptor` (name `finstack-knowledge`), eager `spec(…)` construction with `ToolSpec::validate`, `deny_unknown_fields` argument structs, single-item `Completed` streams, and test scaffolding (`call_context()` / `validated_call` / `call_tool` / `call_tool_err` helpers copied from the calculator toolset's tests, per the document plan's Task 4 instructions).

**Input schema constants** (hand-written JSON, exactly like the document toolset):
- `knowledge_search`: `{query: string (required), top_k?: integer 1..=32, mode?: enum ["semantic","keyword","hybrid"], expand?: integer 0..=2, filter?: object}`
- `graph_query`: `{entity?: string, entity_type?: string, semantic_query?: string, depth?: integer 1..=3, relation_types?: [string]}`
- `list_sources`: `{name_contains?: string, format?: string, status?: enum ["indexed","metadata_only","partial_failure"]}`
- `knowledge_overview`: `{query?: string, top_k?: integer 1..=16}`

- [ ] **Step 1: Write failing tests**

Cases (against `MemoryKnowledgeStore` seeded with a handful of chunks/entities/communities and a `FakeEmbeddingModel` copied from the ingest crate's tests):
1. `tools()` lists exactly `[knowledge_search, graph_query, list_sources, knowledge_overview]` when fully constructed; only `list_sources` with a bare `try_new`; every spec validates and carries id `finstack.tools.knowledge`.
2. `knowledge_search` default call returns hits with `text`, `score`, `source_name`, `heading_path`, `chunk_index` fields; default mode is hybrid (memory backend reports it native).
3. `mode: "semantic"` with a keyword-only construction (no embedder) → `KNOWLEDGE_INVALID_ARGUMENTS`; message names supported modes.
4. Capability fallback: a local `KeywordOnlyIndex` test double wrapping the memory store but reporting `native_keyword` only → default mode resolves to keyword; explicit `mode: "hybrid"` with an embedder present runs the composed path (two searches + fusion — assert both underlying modes were called via the double's counters).
5. `expand: 1` merges neighbor chunks (seed 3 consecutive chunks; assert the hit's returned text includes the neighbor).
6. Rerank: with a `ScriptedReranker` (canned reordering) the top hit changes accordingly; with a failing reranker the call still succeeds and the payload notes `"rerank_failed": true`.
7. `graph_query` by entity name returns the subgraph; `semantic_query` without an embedder → `KNOWLEDGE_INVALID_ARGUMENTS`; both args at once → `KNOWLEDGE_INVALID_ARGUMENTS`.
8. `list_sources` returns catalog rows with name/format/status/chunk_count.
9. `knowledge_overview` without `query` lists communities; with `query` ranks semantically (seed two communities with orthogonal fake embeddings).
10. Unknown-argument and store-error mapping: a `FailingIndex` double returning `Unavailable` → `KNOWLEDGE_STORE_UNAVAILABLE`.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p finstack-ai-tools-knowledge`
Expected: FAIL.

- [ ] **Step 3: Implement**

`search.rs` owns the knowledge_search flow: resolve mode from capabilities (requested → validate supported or composable; default → richest of hybrid/semantic/keyword given capabilities AND embedder presence); embed query when needed (`EmbeddingInputKind::Query`; embedding failure → `KNOWLEDGE_EMBEDDING_FAILED`); native or composed search (composed = two `ChunkQuery` calls with 4×top_k each, fused via the configured strategy); optional rerank over 4×top_k candidates (candidate text = chunk text; map rerank output back by index; failure → fail-soft flag); optional expand via `get_chunks` (range = hit index ± expand, clamped), merged per source with dedup; serialize hits to the output JSON. `toolset.rs` dispatches the four tools; graph/overview/list are straightforward port calls with argument validation and error mapping (`KnowledgeStoreError::Unavailable` → `KNOWLEDGE_STORE_UNAVAILABLE`, everything else → `KNOWLEDGE_QUERY_FAILED`).

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test -p finstack-ai-tools-knowledge`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock extensions/toolsets/finstack-ai-tools-knowledge/
git commit -m "feat: add knowledge query toolset"
```

---

### Task 13: `finstack-ai-context-knowledge` context provider

**Files:**
- Create: `extensions/context/finstack-ai-context-knowledge/Cargo.toml`
- Create: `extensions/context/finstack-ai-context-knowledge/src/lib.rs`
- Create: `extensions/context/finstack-ai-context-knowledge/src/tests.rs`
- Modify: root `Cargo.toml` (workspace member)

**Interfaces:**
- Consumes: `ContextProvider` port (`crates/finstack-ai-runtime/src/ports/context/`), knowledge/embedding ports, `RrfFusion`.
- Produces: `KnowledgeContextProvider::try_new(collection: CollectionId, embedder: Arc<dyn EmbeddingModel>, index: Arc<dyn ChunkIndex>) -> Result<Self, KnowledgeContextError>`, `.with_reranker(…)`, `.with_fusion(…)`, `.with_top_k(u32)` (default 8). Descriptor: `trusted_application_instructions: false`, component id `finstack.context.knowledge`.

**Read first:** `extensions/context/finstack-ai-context-repository/src/lib.rs` — it is the closest existing `ContextProvider` implementation. Copy its descriptor construction, `ComponentInvocation` wiring, `collect` structure, `ContextItem`/`ContextContribution` assembly, and its tests' context/request builders. The knowledge provider differs only in where items come from.

- [ ] **Step 1: Write failing tests**

Cases (helpers copied from the repository context provider's tests):
1. `collect` with a seeded memory index returns a contribution whose items are cited excerpts: item text starts with `[source: {name} › {heading path}]` followed by chunk text; item count ≤ `top_k` and ≤ `request.budget.max_items`; total bytes ≤ `budget.max_bytes` (drop whole items to fit, highest score first).
2. Query text = concatenated `Text` blocks of `request.user_input`; empty user text → empty contribution.
3. Embedding failure (failing fake) → `Ok` with empty contribution, never `Err`.
4. Search failure → same fail-soft empty contribution.
5. Descriptor declares untrusted contributions (`trusted_application_instructions == false`).

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p finstack-ai-context-knowledge`
Expected: FAIL.

- [ ] **Step 3: Implement**

Reuse the Task 12 mode-resolution logic in miniature: hybrid when the index supports it (native or composed via fusion), else semantic. Rerank optional. Item assembly: score-ordered, byte-budgeted, one `ContextItem` per chunk with provenance fields set the way the repository provider sets them (kind/provenance enums — copy its choices for retrieved data).

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test -p finstack-ai-context-knowledge`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock extensions/context/finstack-ai-context-knowledge/
git commit -m "feat: add knowledge retrieval context provider"
```

---

### Task 14: Feeder middleware + `IngestSource::Artifact`

**Files:**
- Create: `extensions/middleware/finstack-ai-middleware-knowledge-ingest/Cargo.toml`
- Create: `extensions/middleware/finstack-ai-middleware-knowledge-ingest/src/lib.rs`
- Create: `extensions/middleware/finstack-ai-middleware-knowledge-ingest/src/tests.rs`
- Modify: `extensions/ingest/finstack-ai-ingest/src/source.rs` (+ its tests) — add the `Artifact` variant
- Modify: root `Cargo.toml` (workspace member)

**Interfaces:**
- Consumes: `Middleware` port, `ArtifactStore`, `IngestPipeline` (Task 10), `parser::DocumentFormat::is_supported_media_type` (document toolset).
- Produces:
  - `IngestSource::Artifact { store: Arc<dyn ArtifactStore>, scope: ArtifactScope, artifact: ArtifactRef }` — resolves bytes, verifies length+digest against the ref (copy the verification from the document-ingest middleware per its plan Task 5), then behaves like `Bytes`; `SourceId` = the artifact digest verbatim
  - `KnowledgeIngestMiddleware::try_new(pipeline: Arc<IngestPipeline>, store: Arc<dyn ArtifactStore>) -> Result<Self, KnowledgeIngestError>` — component id `finstack.middleware.knowledge-ingest`, stage `BeforeModel`, tier `ContextMutation`
  - `.with_spawner(Arc<dyn Fn(PortFuture<()>) + Send + Sync>)` — how detached ingestion actually runs. Default: native `std::thread` + `block_on`-style executor is NOT available portably, so the default spawner is **inline-await-with-timeout? No —** the default is target-conditional: `tokio::spawn` under the runtime's `native-tokio` feature, `wasm_bindgen_futures::spawn_local` under wasm. **Read how the runtime spawns detached work** (grep `spawn` in `crates/finstack-ai-runtime/src`) and reuse its abstraction if one exists; only add the feature-gated fallback if none does. Tests inject a collecting spawner, so tests do not depend on the default.

**Descriptor/stage glue:** copy the `MiddlewareDescriptor`, `ComponentInvocation`, `StageMask`, `OrderTier::ContextMutation` construction from `extensions/middleware/finstack-ai-middleware-compaction/src/lib.rs`, and the `BeforeModel` input/context test builders from its tests (document plan Task 5 gives the same instruction — if that task has landed, copy from `finstack-ai-middleware-document-ingest` instead, it is closer).

- [ ] **Step 1: Write failing tests**

1. Descriptor: stage `BeforeModel`, tier `ContextMutation`, id `finstack.middleware.knowledge-ingest`.
2. Invoke with a user message carrying a supported `File` block (staged csv fixture) → outcome is ALWAYS `StageOutcome::Continue`; the injected collecting spawner received exactly one future; driving that future ingests into the memory-store-backed pipeline (catalog row appears).
3. Unsupported media type (`image/png`) → `Continue`, zero spawned futures.
4. Same attachment on a second invoke → zero new spawned futures (dedup set).
5. Dangling artifact (never staged) → `Continue`; driving the spawned future completes without panic (fail-soft; ingest reports the failure internally).
6. No file blocks → `Continue`, zero spawns.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p finstack-ai-middleware-knowledge-ingest`
Expected: FAIL.

- [ ] **Step 3: Implement**

`invoke` on `BeforeModel`: iterate the draft's user messages' `File(MediaRef)` blocks; filter `is_supported_media_type`; skip `SourceId`s already in the `Mutex<HashSet<Digest>>` dedup set; for each new one, build `IngestSource::Artifact` (constructing the `ArtifactRef` from the `BlobRef` id within the run's scope — copy the document-ingest middleware's BlobRef↔ArtifactRef resolution) and hand `pipeline.ingest(source)` (wrapped to swallow the result into an observer-visible completion — v1: log-free, the report is dropped; the observer event emission point is the pipeline's own report, noted in the module doc) to the spawner. Always return `Continue`.

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test -p finstack-ai-middleware-knowledge-ingest && cargo test -p finstack-ai-ingest`
Expected: PASS (including the new `Artifact` source tests).

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock extensions/middleware/finstack-ai-middleware-knowledge-ingest/ extensions/ingest/finstack-ai-ingest/
git commit -m "feat: add knowledge-ingest feeder middleware and artifact source"
```

---

### Task 15: Provider embeddings (OpenAI + Ollama)

**Files:**
- Create: `extensions/providers/finstack-ai-provider-openai/src/embeddings.rs`
- Create: `extensions/providers/finstack-ai-provider-ollama/src/embeddings.rs`
- Modify: both crates' `src/lib.rs` (module + re-export), both crates' tests

**Interfaces:**
- Consumes: `EmbeddingModel` port (Task 1); each provider crate's existing config/transport (READ `config.rs` and `provider.rs`/`request.rs` in each crate first — reuse their HTTP client construction, auth header assembly, base-url handling, and error mapping verbatim; do not add a new HTTP dependency).
- Produces:
  - `OpenAiEmbeddings::try_new(config: <the crate's existing config type>, model: &str, dimensions: u32) -> Result<Self, <crate's error>>` — POST `/v1/embeddings` `{"model": …, "input": [strings]}`; response `data[i].embedding`
  - `OllamaEmbeddings::try_new(config, model, dimensions)` — POST `/api/embed` `{"model": …, "input": [strings]}`; response `embeddings: [[f32]]`
  - Both implement `EmbeddingModel`; descriptor from constructor args; `max_batch` from config (default 64 OpenAI, 16 Ollama); provider errors → `EmbeddingError::Provider { code, message }` using each crate's existing stable error-code style

- [ ] **Step 1: Write failing tests**

Follow each provider crate's existing test approach exactly (read their tests first — they use a local HTTP test server or transport injection; copy that harness). Cases per provider: (a) request body shape — model, inputs, in order; (b) response parsing → vectors in input order; (c) HTTP error → `EmbeddingError::Provider` with the crate's stable code; (d) batch larger than `max_batch` → `EmbeddingError::InvalidBatch` before any I/O.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p finstack-ai-provider-openai embeddings && cargo test -p finstack-ai-provider-ollama embeddings`
Expected: FAIL.

- [ ] **Step 3: Implement both `embeddings.rs` modules**

Serde request/response structs local to the module; transport + auth reused from the crate. The `Document | Query` input kind is accepted and ignored (neither API distinguishes); `options` metadata keys are ignored in v1.

- [ ] **Step 4: Run tests to verify pass**

Run: same commands as Step 2.
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add extensions/providers/finstack-ai-provider-openai/ extensions/providers/finstack-ai-provider-ollama/
git commit -m "feat: add EmbeddingModel implementations for openai and ollama providers"
```

---

### Task 16: End-to-end integration lane

**Files:**
- Create: `crates/finstack-ai-test/tests/lanes/knowledge.rs`
- Modify: `crates/finstack-ai-test/tests/lanes/mod.rs` (or however lanes register — read `crates/finstack-ai-test/tests/lanes/session.rs` and its harness first, copy the lane wiring)
- Modify: `crates/finstack-ai-test/Cargo.toml` (dev-deps on the new crates)

**Interfaces:**
- Consumes: everything shipped so far; the document fixture corpus; the `finstack-ai-test` harness's scripted-run machinery (read how `lanes/session.rs` builds agents with scripted models and asserts on runs — reuse its helpers).

- [ ] **Step 1: Write the lane (it IS the test)**

One flow, using `MemoryKnowledgeStore`, `FakeEmbeddingModel` + `ScriptedGraphExtractor` copies, and the harness's scripted model:
1. Build an `IngestPipeline` (embedding + graph, finance-core template) and ingest `fixtures/documents/sample.csv` and a Markdown source with headings.
2. `knowledge_search` through a constructed `KnowledgeToolset` (call the tool the way other lanes drive tools, or directly via `Toolset::call` with the harness's contexts) in each of the three modes → the csv content is found; provenance names the source.
3. Run a `CommunityBuilder` with a scripted model; `knowledge_overview` returns the community.
4. Register `KnowledgeContextProvider` on a scripted agent run; assert the model-visible request contains the cited excerpt block (same assertion style the session lane uses for context contributions).
5. Ingest the scanned-PDF fixture → catalog row is `MetadataOnly`; `list_sources` surfaces both statuses.

- [ ] **Step 2: Run to verify pass**

Run: `cargo test -p finstack-ai-test knowledge`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/finstack-ai-test/
git commit -m "test: add knowledge ingestion end-to-end lane"
```

---

### Task 17: Python binding parity

**Files:**
- Modify: `bindings/finstack-ai-python/Cargo.toml` (deps on the new crates)
- Create: `bindings/finstack-ai-python/src/knowledge.rs`
- Modify: `bindings/finstack-ai-python/src/lib.rs` (module registration)
- Modify: the binding's Python test suite (find it: `ls bindings/finstack-ai-python` — follow existing test layout)

**Interfaces:**
- Read the binding's existing wrapping patterns FIRST: how `agent.rs` wraps builders, how callback-based ports are wrapped (`src/callbacks/model.rs`, `src/callbacks/context_provider.rs` show the Python-callable → port pattern — the embedder callback copies `callbacks/model.rs`'s async-callable handling).
- Produces (Python surface):
  - Classes `MemoryKnowledgeStore()`, `SqliteKnowledgeStore(path)` — each usable wherever a chunk index / graph store / catalog is expected
  - `IngestPipeline(collection=…, catalog=…, embedder=…, chunk_index=…, keyword_only=…, graph_extractor=…, graph_store=…, templates=[json_str…], default_template=…)` with `.ingest(bytes=…, media_type=…, name=…, graph_template=None, force=False)` and `.ingest_markdown(text, name=…)` returning an `IngestReport` dict-like
  - `ModelGraphExtractor(model, model_name)`, `ModelReranker(model, model_name)`, `CommunityBuilder(model, model_name, embedder=None)` with `.build(collection, graph_store)`
  - `PyEmbeddingModel` adapter: any Python object with `def embed(self, texts: list[str], kind: str) -> list[list[float]]` plus `dimensions`/`max_batch` attributes; same shape for `Reranker` (`def rerank(self, query, candidates) -> list[tuple[int, float]]`)
  - `KnowledgeToolset(collection, catalog, …)` and `KnowledgeContextProvider(…)` registrable exactly like existing toolsets/providers; `KnowledgeIngestMiddleware(pipeline, store)` likewise
  - Export/import: `store.export_chunks(collection) -> list[bytes]` (serde-JSON rows) and `store.import_chunks(rows)`; same pair for graph rows
  - Raw sources: `pipeline.reingest(source_id_hex, force=False)`; `store.get_source(collection, source_id_hex) -> (media_type, bytes) | None`
- Tests (pytest, following the binding's existing style): construct memory store + fake Python embedder → ingest markdown → toolset registered on an agent whose model is the binding's scripted/fake model → `knowledge_search` returns the content; template JSON round-trip from Python; sqlite store smoke on a tmp path.

- [ ] **Step 1: Write failing Python tests → Step 2: implement `knowledge.rs` → Step 3: run**

Run: the binding's documented test command (check `bindings/finstack-ai-python` README/justfile/Makefile — use exactly that).
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add bindings/finstack-ai-python/
git commit -m "feat: add knowledge ingestion surface to python binding"
```

---

### Task 18: WASM binding parity + wasm gate

**Files:**
- Modify: `bindings/finstack-ai-wasm/Cargo.toml` (deps: ingest, knowledge-memory, tools-knowledge, context-knowledge, middleware-knowledge-ingest — NOT the sqlite backend or providers)
- Create: `bindings/finstack-ai-wasm/src/knowledge.rs`
- Modify: `bindings/finstack-ai-wasm/src/lib.rs`
- Modify: the wasm JS test suite (find how `bindings/finstack-ai-wasm/js` tests existing surfaces; follow that harness)

**Interfaces:**
- Same surface as Python minus sqlite/providers. Host `EmbeddingModel` = JS object `{ dimensions, maxBatch, async embed(texts, kind) -> Float32Array[] }` — copy the JS-async-callback wrapping from the wasm binding's existing model/context callback code (`bindings/finstack-ai-wasm/src/agent/` shows the pattern).
- Regenerate the JS package the way the repo does (the generated files under `bindings/finstack-ai-wasm/js/generated/` are checked in — find the generation command in `scripts/` or package.json and run it).

- [ ] **Step 1: tests → Step 2: implement → Step 3: run**

Run: the wasm test command from the repo's tooling, plus `python3 scripts/wasm_package/check.py` (path per Global Constraints).
Expected: PASS; wasm package check green with NO `FORBIDDEN_WASM` changes.

- [ ] **Step 4: Commit**

```bash
git add bindings/finstack-ai-wasm/
git commit -m "feat: add knowledge ingestion surface to wasm binding"
```

---

### Task 19: Compatibility fixtures, docs, changelog, full verification

**Files:**
- Modify: `fixtures/compatibility/public-rust-api/cargo-public-api/finstack-ai.txt` (if not already updated in Tasks 1–3; additive entries only)
- Modify: `CHANGELOG.md` (entry under the unreleased section, matching its format)
- Create: `docs/knowledge-ingestion.md` — a short user-facing guide: one full Rust example (memory store + openai embedder + pipeline + toolset + context provider), one Python example, the template JSON format, and the collection/catalog model. Copy structure from whichever doc `docs/` uses for the closest existing extension (check `docs/` for a toolset or store guide and mirror it).

- [ ] **Step 1: Write the guide and changelog entry**

- [ ] **Step 2: Full verification**

Run, and paste outputs into the task record before claiming completion:

```bash
cargo test --workspace
cargo clippy --workspace --all-targets
cargo check -p finstack-ai-knowledge-memory -p finstack-ai-ingest -p finstack-ai-tools-knowledge -p finstack-ai-context-knowledge -p finstack-ai-middleware-knowledge-ingest --target wasm32-unknown-unknown
python3 scripts/wasm_package/check.py
```

Plus the Python and wasm binding test commands from Tasks 17–18.
Expected: all green.

- [ ] **Step 3: Commit**

```bash
git add fixtures/compatibility/ CHANGELOG.md docs/knowledge-ingestion.md
git commit -m "docs: add knowledge ingestion guide, changelog, and compatibility fixtures"
```

---

## Known Risks (read before starting)

1. **`model_call` is the first direct `Model`-port caller outside the runtime.** Even the compaction middleware delegates model calls back to the runtime (`StageOutcome::RequestCompactionModel`). If constructing a detached `ModelRequest` outside a run turns out to be impossible (run-scoped identifiers with no fabricable values), STOP and surface it: the fallback design is a `TextCompletion` local trait in the ingest crate (`fn complete(&self, system, user) -> PortFuture<Result<String, …>>`) with the `Model`-port adapter moved to whichever layer CAN build requests (the bindings/host), and `ModelGraphExtractor`/`ModelReranker`/`CommunityBuilder` generic over it. That is a spec deviation — flag it to your human partner before implementing.
2. **Detached spawning in the feeder (Task 14)** depends on what spawn abstraction the runtime exposes. The injected-spawner design keeps tests deterministic either way; only the *default* spawner is at risk. If no portable abstraction exists, feature-gate the default and document that wasm hosts must inject one.
3. **FTS5 external-content triggers** are fiddly (rowid sync on upsert-as-replace). If trigger-based sync fights rusqlite's `INSERT OR REPLACE`, switch to a contentless FTS table (`content=''`) with explicit insert/delete statements in the worker — behavior-equivalent, slightly more storage.
4. **`Digest`/`Timestamp` constructors**: several tasks reference a `digest_of` discovery step. Resolve it once (Task 2) and reuse the same call everywhere; if the kernel digest requires a specific algorithm tag, use the one the artifact service uses.
5. **Workspace-wide clippy is strict** (`missing_docs` warn + workspace lints). Budget time for doc comments on every public item — the Interfaces blocks give the intended one-liners.

