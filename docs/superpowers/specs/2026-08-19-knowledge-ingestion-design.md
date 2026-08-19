# Knowledge Ingestion & Retrieval Extension — Spec

Date: 2026-08-19. Follow-up to `docs/superpowers/specs/2026-08-19-document-ingestion-design.md`
(which produces Markdown and explicitly defers "chunking, embeddings, RAG ingestion") and its
implementation plan `docs/superpowers/plans/2026-08-19-document-ingestion.md`. Prerequisite:
document plan Tasks 1–2 (fixture corpus and the `parser` module of
`finstack-ai-tools-document`). Nothing else in that plan is a dependency; the two plans can
execute in parallel after document Task 2.

## Goal

Give the library a customizable, extensible pipeline that turns ingested documents into
searchable knowledge — vector embeddings for semantic search/RAG, keyword/hybrid search,
and an entity/relation graph with community summaries for knowledge-graph queries — plus
the query surfaces agents use to consume them.

Three layers:

- **Ports (runtime)**: `EmbeddingModel`, `ChunkIndex`, `GraphStore`, `SourceCatalog`, and
  `Reranker` port traits in `finstack-ai-runtime`, following the `JournalStore` precedent
  (trait in runtime ports, implementations in `extensions/`), with
  `PortObject`/`PortFuture` bounds for native+wasm portability.
- **Engine**: new crate `extensions/ingest/finstack-ai-ingest` — a fixed-stage,
  pluggable-implementation pipeline (parse → chunk → [embed ∥ graph-extract] → index)
  plus the model-backed knowledge components that share its `Model`-port-wrapping
  machinery: `ModelGraphExtractor`, `ModelReranker`, and the community-summary builder.
  Host apps drive it directly; a thin middleware feeds run attachments into it.
- **Query surfaces**: a `finstack-ai-tools-knowledge` toolset (`knowledge_search`,
  `graph_query`, `list_sources`, `knowledge_overview`) and a
  `finstack-ai-context-knowledge` `ContextProvider` for automatic top-k retrieval into
  run context.

## Crate layout

```
crates/finstack-ai-runtime/src/ports/
  embedding/     — EmbeddingModel port trait + types
  knowledge/     — ChunkIndex, GraphStore, SourceCatalog, Reranker, FusionStrategy traits + shared types

extensions/
  ingest/finstack-ai-ingest                        — pipeline engine + model-backed components (new category dir)
  stores/finstack-ai-knowledge-memory              — in-memory ChunkIndex + GraphStore + SourceCatalog (wasm-clean)
  stores/finstack-ai-knowledge-sqlite              — SQLite ChunkIndex + GraphStore + SourceCatalog (native-only)
  toolsets/finstack-ai-tools-knowledge             — search / graph / catalog / overview tools
  context/finstack-ai-context-knowledge            — ContextProvider for automatic retrieval
  middleware/finstack-ai-middleware-knowledge-ingest — run-attachment feeder
```

Provider crates gain `EmbeddingModel` implementations: `finstack-ai-provider-openai`,
`finstack-ai-provider-ollama`, and OpenRouter when it lands. `finstack-ai-provider-anthropic`
does not (no embeddings API).

## Decisions

### Ports

1. **`EmbeddingModel` is a runtime port** (like the `Model` port), object-safe, `PortObject`
   bound:

   ```rust
   pub trait EmbeddingModel: PortObject {
       fn descriptor(&self) -> EmbeddingModelDescriptor;
       fn embed(
           &self,
           ctx: EmbeddingCallContext,
           batch: EmbeddingBatch,
       ) -> PortFuture<Result<EmbeddingOutput, EmbeddingError>>;
   }
   ```

   `EmbeddingModelDescriptor` carries model id, output `dimensions: u32`, `max_batch: u32`,
   and `max_input_tokens: u32`. `EmbeddingBatch` is `Arc<[Arc<str>]>` plus an input-kind
   hint (`Document | Query`) for models with asymmetric encodings, plus an
   `options: Metadata` passthrough (decision 7a). `EmbeddingOutput` is one
   `Vec<f32>` per input in order, plus optional usage counts. `EmbeddingCallContext` mirrors
   other port call contexts: run locator where available, deadline, cancellation.
2. **`ChunkIndex`, `GraphStore`, and `SourceCatalog` are runtime ports** in a new
   `ports/knowledge` module. `ChunkIndex` (not "vector index" — it owns chunk text as well
   as vectors, and serves keyword and hybrid search over the same records):

   ```rust
   pub trait ChunkIndex: PortObject {
       fn capabilities(&self) -> ChunkIndexCapabilities;
       fn upsert(&self, records: Arc<[ChunkRecord]>) -> PortFuture<Result<(), KnowledgeStoreError>>;
       fn search(&self, query: ChunkQuery) -> PortFuture<Result<Vec<ChunkHit>, KnowledgeStoreError>>;
       fn get_chunks(&self, collection: CollectionId, source: SourceId, range: ChunkRange) -> PortFuture<Result<Vec<ChunkRecord>, KnowledgeStoreError>>;
       fn delete_chunks(&self, collection: CollectionId, chunks: Arc<[ChunkId]>) -> PortFuture<Result<u64, KnowledgeStoreError>>;
       fn delete_by_source(&self, collection: CollectionId, source: SourceId) -> PortFuture<Result<u64, KnowledgeStoreError>>;
       fn export(&self, collection: CollectionId) -> PortFuture<Result<PortStream<Result<ChunkRecord, KnowledgeStoreError>>, KnowledgeStoreError>>;
       fn import(&self, records: PortStream<ChunkRecord>) -> PortFuture<Result<u64, KnowledgeStoreError>>;
   }

   pub trait GraphStore: PortObject {
       fn upsert(&self, fragment: GraphFragment) -> PortFuture<Result<(), KnowledgeStoreError>>;
       fn query(&self, query: GraphQuery) -> PortFuture<Result<GraphResult, KnowledgeStoreError>>;
       fn replace_communities(&self, collection: CollectionId, communities: Arc<[Community]>) -> PortFuture<Result<(), KnowledgeStoreError>>;
       fn communities(&self, query: CommunityQuery) -> PortFuture<Result<Vec<Community>, KnowledgeStoreError>>;
       fn delete_by_source(&self, collection: CollectionId, source: SourceId) -> PortFuture<Result<u64, KnowledgeStoreError>>;
       fn export(&self, collection: CollectionId) -> PortFuture<Result<PortStream<Result<GraphExportRecord, KnowledgeStoreError>>, KnowledgeStoreError>>;
       fn import(&self, records: PortStream<GraphExportRecord>) -> PortFuture<Result<u64, KnowledgeStoreError>>;
   }

   pub trait SourceCatalog: PortObject {
       fn put(&self, record: SourceRecord) -> PortFuture<Result<(), KnowledgeStoreError>>;
       fn get(&self, collection: CollectionId, source: SourceId) -> PortFuture<Result<Option<SourceRecord>, KnowledgeStoreError>>;
       fn list(&self, query: SourceListQuery) -> PortFuture<Result<Vec<SourceRecord>, KnowledgeStoreError>>;
       fn delete(&self, collection: CollectionId, source: SourceId) -> PortFuture<Result<bool, KnowledgeStoreError>>;
   }
   ```
3. **Collections partition everything.** `CollectionId(Arc<str>)`, validated as a bounded
   slug; the default collection is `"default"` so simple hosts never see the concept.
   Every stored record (`ChunkRecord`, `Entity`, `Relation`, `Community`, `SourceRecord`)
   and every query carries a `CollectionId`; deletes and export/import are
   collection-scoped. Cross-collection queries do not exist in v1 — hosts issue one query
   per collection. Retrofitting partitioning later would be a breaking schema/data
   migration in every backend, which is why it is in v1's types.
4. **Shared types** (in `ports/knowledge`):
   - `SourceId(Digest)` — the ingested document's content digest. For staged attachments
     this is the artifact digest verbatim, i.e. the same digest the document plan's
     BlobRef↔ArtifactRef convention (document spec decision 19) already verifies. For raw
     byte ingestion the pipeline computes the same digest algorithm over the input bytes.
   - `ChunkId { source: SourceId, index: u32 }` — stable across re-ingestion of identical
     bytes. `ChunkRange { start: u32, end: u32 }` (inclusive, clamped to the source's
     chunk count) for neighborhood fetch.
   - `ChunkRecord { collection: CollectionId, chunk_id: ChunkId, chunk_digest: Digest,
     vector: Option<Arc<[f32]>>, text: Arc<str>, metadata: Metadata }` — metadata carries
     document name, format, page/heading provenance. `chunk_digest` (digest of the chunk
     text) drives incremental re-ingest (decision 14). `vector` is optional so keyword-only
     collections work without an embedder.
   - `ChunkQuery { collection: CollectionId, mode: SearchMode, top_k: u32, filter: Metadata, options: Metadata }`
     with `SearchMode::Semantic { vector: Arc<[f32]> } | Keyword { text: Arc<str> } |
     Hybrid { vector: Arc<[f32]>, text: Arc<str> }`. Filter is exact key/value match in
     v1. `options` is a backend passthrough (unknown keys ignored) so engine-specific
     tuning never needs a port change. `ChunkHit { record fields, score: f32 }`,
     descending; scores are comparable within one result set only, never across
     backends. **Scoring algorithms are backend-defined, not port contract**: the port
     specifies mode semantics (vector similarity / text relevance / a fusion of both),
     while each backend implements them with whatever library or engine it wraps — the
     shipped backends use cosine, BM25, and RRF (decisions 26–27), an external engine
     (Tantivy, Qdrant, pgvector+FTS) plugs in its native ranking unchanged.
     `ChunkIndexCapabilities` reports which `SearchMode`s are native plus feature flags
     (e.g. `native_hybrid`); callers adapt (decision 22), and querying an unsupported
     mode returns `KnowledgeStoreError::Unsupported`.
   - `SourceRecord { collection: CollectionId, source: SourceId, name: Option<Arc<str>>,
     format: Arc<str>, page_count: Option<u32>, ingested_at: Timestamp, chunk_count: u32,
     chunk_digests: Arc<[Digest]>, graph_template: Option<(Arc<str>, u32)>,
     status: SourceStatus, metadata: Metadata }` with
     `SourceStatus::Indexed | MetadataOnly { reason } | PartialFailure`. The catalog is
     the answer to "what is in this knowledge base" for agents, hosts, and the future UI.
     `SourceListQuery` filters by collection (required), name substring, format, status,
     with a result ceiling.
   - `GraphFragment { collection: CollectionId, source: SourceId, entities: Vec<Entity>, relations: Vec<Relation> }`.
     `Entity { name: Arc<str>, entity_type: Arc<str>, description: Arc<str>,
     embedding: Option<Arc<[f32]>>, source_chunks: Vec<ChunkId>, metadata: Metadata }`.
     `Relation { from: Arc<str>, to: Arc<str>, relation_type: Arc<str>, description: Arc<str>,
     source_chunks: Vec<ChunkId>, metadata: Metadata }` — endpoints reference entities by
     name; stores resolve/merge by case-insensitive name + type. Entity `embedding` is the
     embedded description (decision 13); optional so graph-only pipelines work. **Stores
     keep per-source attribution for merged graph data**: when several sources attest the
     same entity or relation, `delete_by_source` removes only that source's contribution
     (its `source_chunks` provenance, its description sentences) and the entity/relation
     survives while at least one source still attests it. `GraphExportRecord` rows carry
     the same attribution so import rebuilds it.
   - `GraphQuery { collection, lookup: EntityLookup, depth: u8 (max 3), relation_types: Vec<Arc<str>>, ceilings }`
     with `EntityLookup::Name { name, entity_type: Option } | Semantic { vector, top_k }`.
     Semantic lookup ranks entities by cosine over stored entity embeddings, then expands
     the neighborhood exactly like name lookup. Not a Cypher engine. `GraphResult` is a
     bounded subgraph (entities + relations + provenance).
   - `Community { collection: CollectionId, id: Arc<str>, level: u8, title: Arc<str>,
     summary: Arc<str>, entity_names: Vec<Arc<str>>, embedding: Option<Arc<[f32]>>,
     metadata: Metadata }` and `CommunityQuery { collection, lookup: Name-prefix |
     Semantic { vector, top_k }, level: Option<u8>, ceiling }`. `GraphExportRecord` is an
     enum over `Entity`/`Relation`/`Community` rows with their source attribution.
5. **`Reranker` is a runtime port**:

   ```rust
   pub trait Reranker: PortObject {
       fn rerank(
           &self,
           ctx: RerankCallContext,
           query: Arc<str>,
           candidates: Arc<[Arc<str>]>,
       ) -> PortFuture<Result<Vec<RerankEntry>, RerankError>>;
   }
   ```

   `RerankEntry { index: u32, score: f32 }`, best first; implementations may return fewer
   entries than candidates (dropped candidates are treated as rejected). Query surfaces
   use it as an optional post-search stage (decision 22).
6. **Idempotent re-ingestion**: ingesting a source converges to the same stored state for
   identical bytes. The embedding branch is incremental at chunk level (decision 14); the
   graph branch and catalog entry are whole-source replace (`delete_by_source` + insert).
7. **`KnowledgeStoreError`** follows existing port-error shape: stable non-secret variants
   (`Unavailable`, `InvalidQuery`, `DimensionMismatch { expected, got }`,
   `UnknownCollection`, `Unsupported { what }`, `Internal`), no provider/store internals
   leaked. A `ChunkIndex` implementation must reject vectors whose dimension differs from
   the collection's first-inserted dimension with `DimensionMismatch`. Import validates
   records against the same rules as upsert.

7a. **Evolution policy — the whole surface is built to be amended.** AI retrieval is
    moving fast; the port layer must absorb new methods, modes, and fields without
    breaking implementors or stored data:
    - **Non-exhaustive everything**: every public struct and enum in `ports/embedding`
      and `ports/knowledge` (`ChunkRecord`, `ChunkQuery`, `SearchMode`, `ChunkHit`,
      `SourceRecord`, `Entity`, `Relation`, `Community`, `GraphQuery`, `EntityLookup`,
      capability structs, descriptors, error enums) is `#[non_exhaustive]` with
      constructor/builder functions, so fields and variants are additive, and matches in
      host code carry wildcard arms from day one.
    - **Additive trait methods**: new port-trait methods ship with default
      implementations returning `KnowledgeStoreError::Unsupported` (or the port's
      equivalent), so existing store/embedder/reranker implementations keep compiling.
      Capability structs are how callers discover what a given implementation actually
      supports; capability structs gain fields with defaults for the same reason.
    - **Options escape hatches**: `ChunkQuery`, `GraphQuery`, `CommunityQuery`,
      `EmbeddingBatch`, and `IngestOptions` each carry an `options: Metadata` passthrough
      for backend/model-specific parameters. Unknown keys are ignored, never errors —
      a new technique can be piloted through options before it earns a typed field.
    - **Metadata on every record**: stored records keep their `Metadata` bags, so new
      per-record annotations (e.g. an embedding-model id, a late-interaction payload, a
      new provenance kind) need no schema change. Reserved keys are documented under a
      `finstack.` prefix; the pipeline stamps `finstack.embedding_model` on chunk and
      entity records so future model migrations are detectable.
    - **Pluggable strategies over hard-coded algorithms**: keyword/hybrid scoring is
      backend-defined (decision 4); fusion for non-native-hybrid backends is a
      `FusionStrategy` trait (decision 8a); chunking, extraction, and reranking are
      already traits. No port contract names a specific algorithm or library — shipped
      defaults (`RrfFusion`, `MarkdownChunker`, BM25 in backends) are replaceable
      implementations beside the traits, never requirements of them.

### Pipeline (`extensions/ingest/finstack-ai-ingest`)

8. **Fixed stages, pluggable implementations**, builder-configured:

   ```rust
   let pipeline = IngestPipeline::builder()
       .collection(CollectionId::parse("deals-2026")?)   // default: "default"
       .limits(DocumentLimits::default())                // parse ceilings (document spec)
       .chunker(Arc::new(MarkdownChunker::default()))
       .catalog(source_catalog)                          // Arc<dyn SourceCatalog>, required
       .embedding(embedder, chunk_index)                 // Arc<dyn EmbeddingModel>, Arc<dyn ChunkIndex>
       // — or, mutually exclusive with .embedding(..):
       // .keyword_only(chunk_index)                     // chunk indexing without vectors
       .graph(extractor, graph_store)                    // Arc<dyn GraphExtractor>, Arc<dyn GraphStore>
       .build()?;                                        // error if no indexing branch configured

   let report: IngestReport = pipeline.ingest(source).await?;
   ```

   Parse uses `finstack_ai_tools_document::parser::parse` (free functions — the document
   plan's churn boundary; no anydoc types appear here). The embedding and graph branches
   run in parallel over the same chunks; each is independently optional
   (`.embedding(..)` and `.keyword_only(..)` are mutually exclusive ways to configure the
   chunk branch). The catalog is mandatory: every ingest writes a `SourceRecord`, and
   incremental re-ingest depends on it. The engine crate also hosts the model-backed
   query/maintenance components (`ModelGraphExtractor`, `ModelReranker`,
   `CommunityBuilder`) because they share its `Model`-port-wrapping machinery; hosts
   construct them here and hand them to query surfaces as trait objects.
8a. **`FusionStrategy` makes hybrid search library-agnostic.** A trait in
    `ports/knowledge` (NOT the engine crate — the toolset and context provider consume
    it, and query surfaces must not depend on the ingestion engine and its document
    dependencies): `fn fuse(&self, rankings: &[&[ChunkHit]], top_k: u32) -> Vec<ChunkHit>`.
    It is synchronous pure computation, so no `PortFuture`. When a `ChunkIndex` reports
    `native_hybrid` (Qdrant-style engines, or the shipped backends' internal fusion),
    `SearchMode::Hybrid` goes straight to the backend; when it does not, query surfaces
    run the semantic and keyword searches separately and fuse with the configured
    strategy. The runtime ships `RrfFusion` (k=60, chosen as default because it needs no
    score normalization across backends) and `WeightedScoreFusion { alpha }` next to the
    trait — both are dependency-free pure functions, which is the bar for shipping an
    implementation in the runtime rather than an extension; hosts plug in their own. The
    toolset and context provider accept an optional `Arc<dyn FusionStrategy>` (default
    `RrfFusion`).
9. **`IngestSource`** mirrors the document toolset's source union: `Bytes { bytes, media_type_hint, name }`
   or `Artifact { store: Arc<dyn ArtifactStore>, scope, artifact }` (resolved and
   digest-verified exactly as the document middleware does). Markdown that is already
   parsed can be submitted as `Markdown { text, source_id, name }` to skip the parse stage
   (e.g. when a host has the document-ingest middleware's output in hand).
10. **`Chunker` is a local trait in the ingest crate** (pure computation, not a runtime
    port): `fn chunk(&self, doc: &ChunkInput) -> Result<Vec<Chunk>, ChunkError>`. The
    default `MarkdownChunker` is heading-aware (splits at heading boundaries, then packs to
    a target size in bytes with configurable overlap; tables kept atomic; hard ceiling per
    chunk). `Chunk { id: ChunkId, text: Arc<str>, heading_path: Vec<Arc<str>>, metadata }`.
11. **`GraphExtractor` is a local trait in the ingest crate**:

    ```rust
    pub trait GraphExtractor: PortObject {
        fn extract(
            &self,
            ctx: ExtractContext,
            chunks: Arc<[Chunk]>,
        ) -> PortFuture<Result<GraphFragment, ExtractError>>;
    }
    ```

    The shipped default `ModelGraphExtractor` wraps an existing `Arc<dyn Model>`
    handle: structured-output entity/relation extraction per chunk batch (JSON-schema
    constrained), followed by an in-memory dedup/merge pass (case-insensitive name + type).
    What to extract and how is not hard-coded — it comes from a **graph extraction
    template** (decision 11a). Hosts can still supply their own extractor implementation
    for rule-based/NER approaches.

11a. **Graph extraction templates are data, not code.** A template is a JSON document
    (serde round-trip, `deny_unknown_fields`, easily hand-editable now and
    UI-maintainable later):

    ```rust
    pub struct GraphExtractionTemplate {
        pub id: Arc<str>,             // stable slug, e.g. "finance-core"
        pub version: u32,
        pub name: Arc<str>,
        pub description: Arc<str>,
        pub entity_types: Vec<TypeDef>,     // { name, description }
        pub relation_types: Vec<TypeDef>,   // { name, description }
        pub instructions: Arc<str>,         // domain guidance injected into the prompt
        pub examples: Vec<ExtractionExample>, // optional few-shot: { input, entities, relations }
    }
    ```

    `GraphExtractionTemplate::from_json(&[u8])` validates on load: non-empty id/name, at
    least one entity type, unique type names, bounded sizes (instructions/examples byte
    ceilings). The prompt scaffolding (role text, output JSON schema, chunk framing)
    stays `const` in the crate; the template supplies only the domain content. JSON is
    chosen over TOML/YAML because the workspace already standardizes on serde JSON
    (`RawJson`, tool schemas) and it maps 1:1 onto a future editing UI's payloads.

11b. **Templates are registered on the pipeline and selected per ingestion.**
    `ModelGraphExtractor` is constructed with one or more templates plus a default id;
    `pipeline.ingest_with(source, IngestOptions { graph_template: Some("finance-core"), .. })`
    selects one for that run (plain `ingest(source)` uses the default). Selecting an
    unregistered id is an `IngestError` before any work starts. The crate ships one
    built-in template, `finance-core` v1 (entity types `company, person, instrument,
    metric, event, date, jurisdiction, sector, currency`), as a checked-in JSON asset
    under `extensions/ingest/finstack-ai-ingest/templates/` loaded via `include_bytes!` —
    the same file a future UI would edit. Hosts load additional templates from JSON at
    runtime.

11c. **Template provenance**: every extracted `GraphFragment`'s entities/relations carry
    `template_id` + `template_version` in their metadata, so stores can answer "what
    produced this edge" and re-ingesting a source with a different template (which
    replaces the source's graph wholesale, per decision 6) is observable.
12. **`requires_ocr` documents ingest as metadata-only**: the source is registered in the
    catalog with `SourceStatus::MetadataOnly { reason }` but produces zero chunks, and the
    report says why. Consistent with "a scanned PDF is a success" (document spec decision
    9). Empty markdown from any source behaves the same.
13. **Entity-description embeddings**: when both the embedding and graph branches are
    configured, the pipeline embeds each merged entity's `"{name}: {description}"` with
    the `Document` input kind and stores the vector on the entity (community summaries
    are embedded analogously, but by the `CommunityBuilder` — decision 16). This enables `EntityLookup::Semantic` (GraphRAG-style local
    search entry points). Skipped when no embedder is configured;
    `IngestOptions { embed_entities: bool }` (default true) can disable it to save cost.
14. **Incremental re-ingest (chunk level, embedding branch)**: before indexing, the
    pipeline reads the catalog's existing `SourceRecord`. If the source digest is
    unchanged, the whole ingest short-circuits to a no-op (reported as `skipped_unchanged`
    unless `IngestOptions { force: true }`). If the content changed, chunk digests are
    diffed: unchanged chunks keep their stored vectors (no re-embedding — embeddings are
    the expensive artifact), new/changed chunks are embedded and upserted, removed chunks
    are deleted via `delete_chunks`. The diff is positional (same digest at same index =
    unchanged), so an insertion early in a document shifts later indices and marks them
    changed; implementations may recover shifted chunks' vectors by digest lookup
    instead of re-embedding — an internal optimization, not a port contract. The graph
    branch does not diff: it re-extracts and replaces the source's fragment wholesale
    (LLM extraction is not chunk-separable after the merge pass). The catalog record is
    rewritten last, making it the commit point: a crash mid-ingest leaves the old
    catalog record, and the next ingest's diff re-converges the index (upserts and
    deletes are idempotent).
15. **Bounded everything**: `IngestLimits { max_chunks_per_document (default 2048),
    max_concurrent_embed_batches (4), max_concurrent_extract_calls (2), per_call_deadline }`
    plus the parse-stage `DocumentLimits`. Same defensive posture as the document spec.
16. **Community summaries are a host-driven batch step**, not part of per-document
    ingest: `CommunityBuilder::build(collection, graph_store, options)` in the engine
    crate. It runs label-propagation community detection over the collection's graph
    (pure Rust, deterministic given a seed, single level in v1 with the `level` field
    reserved for future hierarchies), then generates one bounded summary per community
    via the `Model` port (structured output: title + summary), embeds summaries when an
    embedder is supplied, and calls `replace_communities` — wholesale replace per
    collection, idempotent. Communities are derived data: any ingest invalidates them
    only in the sense that the host re-runs the builder when it chooses (the report and
    observer events make staleness visible). Ceilings: max communities, max entities
    summarized per community, per-call deadline.
17. **Failure policy**: parse failure is fatal (`IngestError`). Per-chunk embed/extract
    failures are collected into the report, not fatal; a branch whose store `upsert` fails
    marks the whole branch failed in the report and the catalog record
    `SourceStatus::PartialFailure`. `IngestReport` records per-stage counts (including
    chunks skipped/reused by incremental re-ingest), collected failures, extractor token
    usage where the driver reports it, and elapsed time. The pipeline never panics on
    malformed model output — unparseable extraction responses count as per-chunk failures.
18. **The pipeline does no journaling and holds no run state.** It is host-driven and
    side-band to the agent runtime; determinism guarantees of runs are unaffected because
    nothing the pipeline does enters the journaled conversation.

### Run-path feeder

19. **New crate `extensions/middleware/finstack-ai-middleware-knowledge-ingest`**,
    component id `finstack.middleware.knowledge-ingest`, stage `BeforeModel`, tier
    `ContextMutation`, ordered after `finstack.middleware.document-ingest`. It **always
    returns `StageOutcome::Continue`** — it never rewrites the draft (the document-ingest
    middleware owns that; per its plan's refinement of spec decision 14, middleware cannot
    append to canonical messages, and this crate does not need to touch the model-visible
    request at all).
20. **Feeder behavior**: scan user messages for `ContentBlock::File` blocks whose media
    type is a supported document format, resolve bytes from the `ArtifactStore` using the
    BlobRef↔ArtifactRef convention, and submit `IngestSource::Bytes` to a configured
    `Arc<IngestPipeline>` (into the pipeline's configured collection). Ingestion runs
    detached from the turn (spawned; the middleware future resolves immediately) with
    completion/failure reported through observer-visible events. Fail-soft: no feeder
    error ever affects the run.
21. **Per-source dedup in the feeder**: because `BeforeModel` fires every turn, the feeder
    keeps an in-memory set of `SourceId`s it has submitted and skips repeats. Replay
    re-submission is harmless anyway — ingestion is idempotent (decision 6) and unchanged
    sources short-circuit (decision 14) — the dedup only avoids wasted work. Duplicate
    parse relative to document-ingest is accepted: both crates call `parser::parse`
    independently (median < 5 ms); neither middleware crate depends on the other.

### Query surfaces

22. **Toolset `finstack-ai-tools-knowledge`**, toolset id `finstack.tools.knowledge`,
    following the calculator/document toolset pattern (`try_new`, eager validated
    `ToolSpec`s, `Arc<[ToolSpec]>`, `deny_unknown_fields` args, single
    `ToolStreamItem::Completed`). Constructed with a `CollectionId`, an
    `Arc<dyn SourceCatalog>`, and optionally `Arc<dyn EmbeddingModel>` +
    `Arc<dyn ChunkIndex>`, `Arc<dyn GraphStore>`, and `Arc<dyn Reranker>`; only the tools
    whose dependencies are present appear in `tools()`.
    - `knowledge_search { query: String, top_k?: u32 (default 8, max 32), mode?: "semantic" | "keyword" | "hybrid", expand?: u8 (default 0, max 2), filter?: object }`
      → embeds the query (`Query` input kind) as needed by mode, searches, optionally
      reranks and expands. Mode handling is capability-driven: the default is the
      richest mode the index supports (hybrid → semantic → keyword), hybrid uses the
      backend natively or the configured `FusionStrategy` (decision 8a), and an
      explicitly requested unsupported mode returns `KNOWLEDGE_INVALID_ARGUMENTS`
      naming the supported modes. The tool description reflects the constructed
      capabilities. The search then optionally
      reranks (when a `Reranker` is configured, over 4× top_k candidates, returning
      top_k), then expands each hit by `expand` neighboring chunks per side via
      `get_chunks` (merged, deduplicated). Returns ranked excerpts with text, score, and
      source provenance (document name, heading path).
    - `graph_query { entity?: String, entity_type?: String, semantic_query?: String,
      depth?: u8 (default 1, max 3), relation_types?: [String] }` — exactly one of
      `entity` / `semantic_query`; the latter embeds and uses `EntityLookup::Semantic` →
      bounded subgraph as structured JSON.
    - `list_sources { name_contains?: String, format?: String, status?: String }` →
      catalog listing so the model knows what it can search.
    - `knowledge_overview { query?: String, top_k?: u32 (default 5, max 16) }` →
      community titles+summaries, semantically ranked when `query` is given (embedded
      against community-summary embeddings), otherwise listed. GraphRAG-style global
      questions start here.
    - Tool metadata: read-only, parallel, safe to retry, no approval — same values as the
      document toolset.
23. **Stable error codes** (lower snake_case values in `pub const` SCREAMING_SNAKE names,
    matching the `document_*` precedent): `KNOWLEDGE_INVALID_ARGUMENTS`,
    `KNOWLEDGE_STORE_UNAVAILABLE`, `KNOWLEDGE_EMBEDDING_FAILED`, `KNOWLEDGE_QUERY_FAILED`,
    `KNOWLEDGE_RERANK_FAILED`. Rerank failure is fail-soft inside `knowledge_search`
    (results returned un-reranked, failure noted in the payload); the code exists for the
    hard failure paths (e.g. invalid reranker output).
24. **Context provider `finstack-ai-context-knowledge`** implements `ContextProvider`:
    embeds the run's latest user text via the `EmbeddingModel`, retrieves top-k for its
    configured collection (hybrid mode when available, optional `Reranker` applied)
    within the request's budget ceilings, and contributes one bounded block formatted as
    cited excerpts (source name + heading path per excerpt). Retrieved content is data:
    `trusted_application_instructions: false`. If embedding or search fails, it
    contributes nothing (empty contribution) — fail-soft, observable via events. Graph
    context injection is out of scope for v1 (the toolset covers graph access).
25. **`ModelReranker`** (in the engine crate) is the shipped `Reranker`: listwise
    reranking via `Model`-port structured output (query + numbered candidates → ranked
    indices with scores), bounded candidate count and per-call deadline. Hosts with a
    dedicated rerank API (Cohere, Voyage) implement the port themselves in v1.

### Storage backends

26. **`finstack-ai-knowledge-memory`**: one crate implementing all three stores —
    `Vec`-backed brute-force cosine search, an in-memory inverted index with BM25 for
    keyword search, internal RRF for hybrid (reports all three modes native),
    adjacency-map graph with communities, and a `HashMap` catalog. These algorithm
    choices are private to the backend, per decision 4. Wasm-clean; the backend used by
    all tests and the wasm binding.
27. **`finstack-ai-knowledge-sqlite`**: follows the store-sqlite worker/schema pattern.
    Vectors as BLOBs with brute-force scan in v1; keyword search via SQLite FTS5
    (bundled in the workspace's existing SQLite dependency) with BM25 ranking; hybrid
    via internal RRF (reports all three modes native). Graph as
    entity/relation/community tables with indexed name/type lookups; catalog as a
    sources table. All of this is private implementation behind the ports — swapping in
    sqlite-vec/ANN, a different tokenizer, or a different fusion later is a non-breaking
    internal change. Native-only; not in the wasm build.
28. **External backends (Qdrant, LanceDB, pgvector, Neo4j) are future extension crates**
    behind the same ports; out of scope for v1.

### Provider embeddings

29. **`finstack-ai-provider-openai` and `finstack-ai-provider-ollama` implement
    `EmbeddingModel`** against their embeddings endpoints, exposed as a separate
    constructor from the chat driver (e.g. `OpenAiEmbeddings::try_new(config)`), reusing
    each crate's existing transport/auth/config machinery. Batch limits and dimensions
    come from configuration with sane per-provider defaults. The OpenRouter provider adds
    the same when that plan lands (its spec's MediaResolver work is unrelated).

### Bindings and portability

30. **Native + wasm, all bindings in v1.** All new ports use `PortObject` bounds. The
    memory backend, ingest pipeline, community builder, toolset, and context provider
    compile for wasm32; the sqlite backend and provider embedding impls follow each
    crate's existing target policy. No additions to `FORBIDDEN_WASM` in
    `scripts/wasm_package/check.py`.
31. **Python binding**: construct pipelines from built-ins (memory/sqlite stores, provider
    embedders, `ModelGraphExtractor` with templates supplied as JSON strings/dicts,
    `ModelReranker`, `CommunityBuilder`), pass a host-implemented `EmbeddingModel` or
    `Reranker` as a Python callable, run `ingest` (with per-call template selection,
    force flag) and community builds, use export/import and catalog listing, and register
    the toolset/context provider/middleware like existing extensions.
32. **WASM binding**: same surface with memory stores only; host `EmbeddingModel` as a JS
    async callback returning `Float32Array`s.

### Testing

33. **Deterministic test doubles** in the existing test-support location:
    `FakeEmbeddingModel` (seeded hash → unit vector, fixed dimensions),
    `ScriptedGraphExtractor` (canned fragments per chunk), and `ScriptedReranker`. No
    test calls a real provider or model.
34. **Test tiers**: unit tests per crate — chunker boundary cases on heading-heavy/table
    Markdown; dimension-mismatch and unknown-collection rejection; collection isolation
    (records in one collection never surface in another's queries); keyword/hybrid
    ranking sanity per backend; capability-driven mode selection, including a test
    double that reports no `native_hybrid` to prove the `FusionStrategy` composition
    path and a custom strategy plugging in; `Unsupported` returned (not panics) for
    unimplemented modes/methods; `get_chunks` range clamping and `expand` merging; graph
    merge/dedup and semantic entity lookup; community detection determinism and
    wholesale replace; catalog round-trip and list filters; idempotent re-ingest,
    unchanged-source short-circuit, and chunk-level diff (reused vs re-embedded vs
    deleted counts); export/import round-trip across memory↔sqlite; toolset arg
    validation and error codes; rerank fail-soft; template JSON load/validation,
    including rejection of malformed and oversized templates, unregistered-id selection
    errors, and template provenance on extracted fragments. Plus a `finstack-ai-test`
    lane running the document fixture corpus end-to-end: ingest → `knowledge_search`
    (each mode) returns the staged content → `knowledge_overview` after a community
    build → context provider injects excerpts into a scripted run; binding smoke tests
    in Python and wasm.
35. **Compatibility**: new runtime ports and types update
    `fixtures/compatibility/public-rust-api/cargo-public-api/finstack-ai.txt` (additive);
    the workspace lint/version/edition inheritance and per-task commit discipline of the
    document plan apply unchanged.

## Non-goals

- ANN index structures (memory/sqlite are brute-force/FTS5 in v1; internal upgrades are
  non-breaking).
- External vector/graph database backends and dedicated rerank-API integrations
  (Cohere/Voyage rerank) — future extension crates behind the existing ports.
- Hierarchical (multi-level) community summaries — the `level` field is reserved; v1
  builds one level.
- Chunk-level incremental updates for the **graph** branch — graph re-extraction is
  whole-source; only the embedding branch diffs chunks.
- Cross-collection queries — one collection per query in v1.
- TTL/retention policies — hosts (e.g. the future UI app) drive deletion off the
  catalog's `ingested_at`; policy does not belong in the library.
- OCR'd sources (inherits the document spec's OCR non-goal; scanned PDFs ingest as
  metadata-only).
- Embeddings via `finstack-ai-provider-anthropic` (no such API).
- A query planner or graph query language; `GraphQuery` stays a bounded structured lookup.
- Automatic graph context injection (toolset-only for graph access in v1).
