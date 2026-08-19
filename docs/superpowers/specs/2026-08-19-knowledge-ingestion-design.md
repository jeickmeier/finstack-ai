# Knowledge Ingestion & Retrieval Extension — Spec

Date: 2026-08-19. Follow-up to `docs/superpowers/specs/2026-08-19-document-ingestion-design.md`
(which produces Markdown and explicitly defers "chunking, embeddings, RAG ingestion") and its
implementation plan `docs/superpowers/plans/2026-08-19-document-ingestion.md`. Prerequisite:
document plan Tasks 1–2 (fixture corpus and the `parser` module of
`finstack-ai-tools-document`). Nothing else in that plan is a dependency; the two plans can
execute in parallel after document Task 2.

## Goal

Give the library a customizable, extensible pipeline that turns ingested documents into
searchable knowledge — vector embeddings for semantic search/RAG and an entity/relation
graph for knowledge-graph queries — plus the query surfaces agents use to consume them.

Three layers:

- **Ports (runtime)**: `EmbeddingModel`, `VectorIndex`, and `GraphStore` port traits in
  `finstack-ai-runtime`, following the `JournalStore` precedent (trait in runtime ports,
  implementations in `extensions/`), with `PortObject`/`PortFuture` bounds for native+wasm
  portability.
- **Pipeline (engine)**: new crate `extensions/ingest/finstack-ai-ingest` — a fixed-stage,
  pluggable-implementation pipeline: parse → chunk → [embed ∥ graph-extract] → index. Host
  apps drive it directly; a thin middleware feeds run attachments into it.
- **Query surfaces**: a `finstack-ai-tools-knowledge` toolset (`semantic_search`,
  `graph_query`) and a `finstack-ai-context-knowledge` `ContextProvider` for automatic
  top-k retrieval into run context.

## Crate layout

```
crates/finstack-ai-runtime/src/ports/
  embedding/     — EmbeddingModel port trait + types
  knowledge/     — VectorIndex + GraphStore port traits + shared types

extensions/
  ingest/finstack-ai-ingest                        — pipeline engine (new category dir)
  stores/finstack-ai-knowledge-memory              — in-memory VectorIndex + GraphStore (wasm-clean)
  stores/finstack-ai-knowledge-sqlite              — SQLite VectorIndex + GraphStore (native-only)
  toolsets/finstack-ai-tools-knowledge             — semantic_search / graph_query tools
  context/finstack-ai-context-knowledge            — ContextProvider for automatic retrieval
  middleware/finstack-ai-middleware-knowledge-ingest — run-attachment feeder
```

Provider crates gain `EmbeddingModel` implementations: `finstack-ai-provider-openai`,
`finstack-ai-provider-ollama`, and OpenRouter when it lands. `finstack-ai-provider-anthropic`
does not (no embeddings API).

## Decisions

### Ports

1. **`EmbeddingModel` is a runtime port** (like `ModelDriver`), object-safe, `PortObject`
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
   hint (`Document | Query`) for models with asymmetric encodings. `EmbeddingOutput` is one
   `Vec<f32>` per input in order, plus optional usage counts. `EmbeddingCallContext` mirrors
   other port call contexts: run locator where available, deadline, cancellation.
2. **`VectorIndex` and `GraphStore` are runtime ports** in a new `ports/knowledge` module:

   ```rust
   pub trait VectorIndex: PortObject {
       fn upsert(&self, records: Arc<[VectorRecord]>) -> PortFuture<Result<(), KnowledgeStoreError>>;
       fn search(&self, query: VectorQuery) -> PortFuture<Result<Vec<VectorHit>, KnowledgeStoreError>>;
       fn delete_by_source(&self, source: SourceId) -> PortFuture<Result<u64, KnowledgeStoreError>>;
   }

   pub trait GraphStore: PortObject {
       fn upsert(&self, fragment: GraphFragment) -> PortFuture<Result<(), KnowledgeStoreError>>;
       fn query(&self, query: GraphQuery) -> PortFuture<Result<GraphResult, KnowledgeStoreError>>;
       fn delete_by_source(&self, source: SourceId) -> PortFuture<Result<u64, KnowledgeStoreError>>;
   }
   ```
3. **Shared types** (in `ports/knowledge`):
   - `SourceId(Digest)` — the ingested document's content digest. For staged attachments
     this is the artifact digest verbatim, i.e. the same digest the document plan's
     BlobRef↔ArtifactRef convention (document spec decision 19) already verifies. For raw
     byte ingestion the pipeline computes the same digest algorithm over the input bytes.
   - `ChunkId { source: SourceId, index: u32 }` — stable across re-ingestion of identical
     bytes.
   - `VectorRecord { chunk_id: ChunkId, vector: Arc<[f32]>, text: Arc<str>, metadata: Metadata }`
     — metadata carries document name, format, page/heading provenance.
   - `VectorQuery { vector: Arc<[f32]>, top_k: u32, filter: Metadata }` — filter is exact
     key/value match in v1. `VectorHit { record fields, score: f32 }`, cosine similarity,
     descending.
   - `GraphFragment { source: SourceId, entities: Vec<Entity>, relations: Vec<Relation> }`.
     `Entity { name: Arc<str>, entity_type: Arc<str>, description: Arc<str>, source_chunks: Vec<ChunkId> }`.
     `Relation { from: Arc<str>, to: Arc<str>, relation_type: Arc<str>, description: Arc<str>, source_chunks: Vec<ChunkId> }`
     — endpoints reference entities by name; stores resolve/merge by
     case-insensitive name + type.
   - `GraphQuery` v1 is deliberately modest: entity lookup by name/type prefix,
     neighborhood expansion to `depth: u8` (max 3), relation-type filtering, result
     ceilings. Not a Cypher engine. `GraphResult` is a bounded subgraph
     (entities + relations + provenance).
4. **Idempotent re-ingestion**: ingesting a source is `delete_by_source` + insert in both
   stores. Re-running ingestion for identical bytes converges to the same state. There is
   no chunk-level incremental update in v1.
5. **`KnowledgeStoreError`** follows existing port-error shape: stable non-secret variants
   (`Unavailable`, `InvalidQuery`, `DimensionMismatch { expected, got }`, `Internal`), no
   provider/store internals leaked. A `VectorIndex` implementation must reject vectors
   whose dimension differs from the first-inserted dimension with `DimensionMismatch`.

### Pipeline (`extensions/ingest/finstack-ai-ingest`)

6. **Fixed stages, pluggable implementations**, builder-configured:

   ```rust
   let pipeline = IngestPipeline::builder()
       .limits(DocumentLimits::default())            // parse ceilings (document spec)
       .chunker(Arc::new(MarkdownChunker::default()))
       .embedding(embedder, vector_index)            // Arc<dyn EmbeddingModel>, Arc<dyn VectorIndex>
       .graph(extractor, graph_store)                // Arc<dyn GraphExtractor>, Arc<dyn GraphStore>
       .build()?;                                    // error if neither branch configured

   let report: IngestReport = pipeline.ingest(source).await?;
   ```

   Parse uses `finstack_ai_tools_document::parser::parse` (free functions — the document
   plan's churn boundary; no anydoc types appear here). The embedding and graph branches
   run in parallel over the same chunks; each is independently optional. A pipeline with
   neither branch is a build error.
7. **`IngestSource`** mirrors the document toolset's source union: `Bytes { bytes, media_type_hint, name }`
   or `Artifact { store: Arc<dyn ArtifactStore>, scope, artifact }` (resolved and
   digest-verified exactly as the document middleware does). Markdown that is already
   parsed can be submitted as `Markdown { text, source_id, name }` to skip the parse stage
   (e.g. when a host has the document-ingest middleware's output in hand).
8. **`Chunker` is a local trait in the ingest crate** (pure computation, not a runtime
   port): `fn chunk(&self, doc: &ChunkInput) -> Result<Vec<Chunk>, ChunkError>`. The
   default `MarkdownChunker` is heading-aware (splits at heading boundaries, then packs to
   a target size in bytes with configurable overlap; tables kept atomic; hard ceiling per
   chunk). `Chunk { id: ChunkId, text: Arc<str>, heading_path: Vec<Arc<str>>, metadata }`.
9. **`GraphExtractor` is a local trait in the ingest crate**:

   ```rust
   pub trait GraphExtractor: PortObject {
       fn extract(
           &self,
           ctx: ExtractContext,
           chunks: Arc<[Chunk]>,
       ) -> PortFuture<Result<GraphFragment, ExtractError>>;
   }
   ```

   The shipped default `ModelGraphExtractor` wraps an existing `Arc<dyn ModelDriver>`
   handle: structured-output entity/relation extraction per chunk batch (JSON-schema
   constrained), followed by an in-memory dedup/merge pass (case-insensitive name + type).
   What to extract and how is not hard-coded — it comes from a **graph extraction
   template** (decision 9a). Hosts can still supply their own extractor implementation
   for rule-based/NER approaches.

9a. **Graph extraction templates are data, not code.** A template is a JSON document
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

9b. **Templates are registered on the pipeline and selected per ingestion.**
    `ModelGraphExtractor` is constructed with one or more templates plus a default id;
    `pipeline.ingest_with(source, IngestOptions { graph_template: Some("finance-core"), .. })`
    selects one for that run (plain `ingest(source)` uses the default). Selecting an
    unregistered id is an `IngestError` before any work starts. The crate ships one
    built-in template, `finance-core` v1 (entity types `company, person, instrument,
    metric, event, date, jurisdiction, sector, currency`), as a checked-in JSON asset
    under `extensions/ingest/finstack-ai-ingest/templates/` loaded via `include_bytes!` —
    the same file a future UI would edit. Hosts load additional templates from JSON at
    runtime.

9c. **Template provenance**: every extracted `GraphFragment`'s entities/relations carry
    `template_id` + `template_version` in their metadata, so stores can answer "what
    produced this edge" and re-ingesting a source with a different template (which
    replaces the source's graph wholesale, per decision 4) is observable.
10. **`requires_ocr` documents ingest as metadata-only**: the source is registered (so
    idempotency and delete-by-source work) but produces zero chunks, and the report says
    why. Consistent with "a scanned PDF is a success" (document spec decision 9). Empty
    markdown from any source behaves the same.
11. **Bounded everything**: `IngestLimits { max_chunks_per_document (default 2048),
    max_concurrent_embed_batches (4), max_concurrent_extract_calls (2), per_call_deadline }`
    plus the parse-stage `DocumentLimits`. Same defensive posture as the document spec.
12. **Failure policy**: parse failure is fatal (`IngestError`). Per-chunk embed/extract
    failures are collected into the report, not fatal; a branch whose store `upsert` fails
    marks the whole branch failed in the report. `IngestReport` records per-stage counts,
    collected failures, extractor token usage where the driver reports it, and elapsed
    time. The pipeline never panics on malformed model output — unparseable extraction
    responses count as per-chunk failures.
13. **The pipeline does no journaling and holds no run state.** It is host-driven and
    side-band to the agent runtime; determinism guarantees of runs are unaffected because
    nothing the pipeline does enters the journaled conversation.

### Run-path feeder

14. **New crate `extensions/middleware/finstack-ai-middleware-knowledge-ingest`**,
    component id `finstack.middleware.knowledge-ingest`, stage `BeforeModel`, tier
    `ContextMutation`, ordered after `finstack.middleware.document-ingest`. It **always
    returns `StageOutcome::Continue`** — it never rewrites the draft (the document-ingest
    middleware owns that; per its plan's refinement of spec decision 14, middleware cannot
    append to canonical messages, and this crate does not need to touch the model-visible
    request at all).
15. **Feeder behavior**: scan user messages for `ContentBlock::File` blocks whose media
    type is a supported document format, resolve bytes from the `ArtifactStore` using the
    BlobRef↔ArtifactRef convention, and submit `IngestSource::Bytes` to a configured
    `Arc<IngestPipeline>`. Ingestion runs detached from the turn (spawned; the middleware
    future resolves immediately) with completion/failure reported through observer-visible
    events. Fail-soft: no feeder error ever affects the run.
16. **Per-source dedup in the feeder**: because `BeforeModel` fires every turn, the feeder
    keeps an in-memory set of `SourceId`s it has submitted and skips repeats. Replay
    re-submission is harmless anyway — ingestion is idempotent (decision 4) — the dedup
    only avoids wasted work.
17. **Duplicate parse relative to document-ingest is accepted.** Both crates call
    `parser::parse` independently (median < 5 ms); neither middleware crate depends on the
    other.

### Query surfaces

18. **Toolset `finstack-ai-tools-knowledge`**, toolset id `finstack.tools.knowledge`,
    following the calculator/document toolset pattern (`try_new`, eager validated
    `ToolSpec`s, `Arc<[ToolSpec]>`, `deny_unknown_fields` args, single
    `ToolStreamItem::Completed`). Constructed with `Arc<dyn EmbeddingModel>` +
    `Arc<dyn VectorIndex>` and/or `Arc<dyn GraphStore>`; only the tools whose dependencies
    are present appear in `tools()`.
    - `semantic_search { query: String, top_k?: u32 (default 8, max 32), filter?: object }`
      → embeds the query (`Query` input kind), searches, returns ranked chunks with text,
      score, and source provenance (document name, heading path).
    - `graph_query { entity: String, entity_type?: String, depth?: u8 (default 1, max 3),
      relation_types?: [String] }` → bounded subgraph as structured JSON.
    - Tool metadata: read-only, parallel, safe to retry, no approval — same values as the
      document toolset.
19. **Stable error codes** (lower snake_case values in `pub const` SCREAMING_SNAKE names,
    matching the `document_*` precedent): `KNOWLEDGE_INVALID_ARGUMENTS`,
    `KNOWLEDGE_STORE_UNAVAILABLE`, `KNOWLEDGE_EMBEDDING_FAILED`, `KNOWLEDGE_QUERY_FAILED`.
20. **Context provider `finstack-ai-context-knowledge`** implements `ContextProvider`:
    embeds the run's latest user text via the `EmbeddingModel`, retrieves top-k within the
    request's budget ceilings, and contributes one bounded block formatted as cited
    excerpts (source name + heading path per excerpt). Retrieved content is data:
    `trusted_application_instructions: false`. If embedding or search fails, it
    contributes nothing (empty contribution) — fail-soft, observable via events. Graph
    context injection is out of scope for v1 (the toolset covers graph access).

### Storage backends

21. **`finstack-ai-knowledge-memory`**: `Vec`-backed brute-force cosine `VectorIndex` and
    adjacency-map `GraphStore` in one crate. Wasm-clean; the backend used by all tests and
    the wasm binding.
22. **`finstack-ai-knowledge-sqlite`**: follows the store-sqlite worker/schema pattern.
    Vectors as BLOBs with brute-force scan in v1 — fine at library scale; an
    sqlite-vec/ANN upgrade later is a non-breaking internal change. Graph as
    entity/relation tables with indexed name/type lookups. Native-only; not in the wasm
    build.
23. **External backends (Qdrant, LanceDB, pgvector, Neo4j) are future extension crates**
    behind the same ports; out of scope for v1.

### Provider embeddings

24. **`finstack-ai-provider-openai` and `finstack-ai-provider-ollama` implement
    `EmbeddingModel`** against their embeddings endpoints, exposed as a separate
    constructor from the chat driver (e.g. `OpenAiEmbeddings::try_new(config)`), reusing
    each crate's existing transport/auth/config machinery. Batch limits and dimensions
    come from configuration with sane per-provider defaults. The OpenRouter provider adds
    the same when that plan lands (its spec's MediaResolver work is unrelated).

### Bindings and portability

25. **Native + wasm, all bindings in v1.** All three ports use `PortObject` bounds. The
    memory backend, ingest pipeline, toolset, and context provider compile for wasm32; the
    sqlite backend and provider embedding impls follow each crate's existing target
    policy. No additions to `FORBIDDEN_WASM` in `scripts/wasm_package/check.py`.
26. **Python binding**: construct pipelines from built-ins (memory/sqlite stores, provider
    embedders, `ModelGraphExtractor` with templates supplied as JSON strings/dicts),
    pass a host-implemented `EmbeddingModel` as a Python callable, run `ingest` (with
    per-call template selection), and register the toolset/context provider/middleware
    like existing extensions.
27. **WASM binding**: same surface with memory stores only; host `EmbeddingModel` as a JS
    async callback returning `Float32Array`s.

### Testing

28. **Deterministic test doubles** in the existing test-support location:
    `FakeEmbeddingModel` (seeded hash → unit vector, fixed dimensions) and
    `ScriptedGraphExtractor` (canned fragments per chunk). No test calls a real provider.
29. **Test tiers**: unit tests per crate (chunker boundary cases on heading-heavy/table
    Markdown; dimension-mismatch rejection; graph merge/dedup; idempotent re-ingest;
    toolset arg validation and error codes; template JSON load/validation, including
    rejection of malformed and oversized templates, unregistered-id selection errors,
    and template provenance on extracted fragments); a `finstack-ai-test` lane running the
    document fixture corpus end-to-end: ingest → `semantic_search` returns the staged
    content → context provider injects it into a scripted run; binding smoke tests in
    Python and wasm.
30. **Compatibility**: new runtime ports and types update
    `fixtures/compatibility/public-rust-api/cargo-public-api/finstack-ai.txt` (additive);
    the workspace lint/version/edition inheritance and per-task commit discipline of the
    document plan apply unchanged.

## Non-goals

- Reranking, hybrid/BM25 search, ANN indexes (memory/sqlite are brute-force in v1).
- Graph community detection/summarization (GraphRAG-style global queries).
- Chunk-level incremental updates — idempotency is whole-source only.
- External vector/graph database backends.
- OCR'd sources (inherits the document spec's OCR non-goal; scanned PDFs ingest as
  metadata-only).
- Embeddings via `finstack-ai-provider-anthropic` (no such API).
- A query planner or graph query language; `GraphQuery` stays a bounded structured lookup.
- Automatic graph context injection (toolset-only for graph access in v1).
