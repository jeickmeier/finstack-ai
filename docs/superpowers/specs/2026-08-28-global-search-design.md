# Global search design: federated retrieval over memory, documents, journal, and graph

- **Date:** 2026-08-28
- **Status:** Draft for review
- **Source of truth for the shape:** design session 2026-08-28 (memory semantic
  search, generalized to global search); `AGENTS.md`;
  `extensions/context/finstack-ai-memory`;
  `docs/superpowers/specs/2026-08-28-knowledge-agent-design.md` §3 (the
  vector-retrieval non-goal this initiative discharges).

## 1. Overview

One retrieval capability across the engine's stores of record — memory
records, ingested documents/artifacts, journaled sessions, and an extracted
entity graph — with **single-strategy and hybrid** querying. The default
everywhere is lexical (keyword / BM25 / grep-style) with zero network
dependencies; semantic (embedding) retrieval and graph retrieval are opt-in
per composition, and hybrid plans fuse any combination.

The organizing rule, and the claim under test:

> **Generic at the contract, fusion, and surfacing layers; subsystem-owned at
> the index layer.**

Every index is *derived, rebuildable data* owned by the subsystem whose
lifecycle it tracks. No unified vector/graph store exists, because memory
(mutating records, in-transaction eviction), artifacts (immutable,
content-addressed, pin/orphan lifecycle), and journals (append-only system
of record, read-only taps) cannot share consistency machinery without
inventing exactly the speculative abstraction `AGENTS.md` forbids. What they
share instead is the query contract, the rank-fusion logic, the embedding
primitives, and the agent-facing surfaces.

```text
Layer 3  finstack-ai-search           global façade: search toolset + optional context provider
Layer 2  per-source index extensions  memory | documents/artifacts | journal | graph
Layer 1  finstack-ai-search-core      query/hit/evidence contract, SearchSource trait, hybrid fusion
Layer 0  finstack-ai-embeddings       TextEmbedder, EmbeddingVector, reference HashEmbedder
```

Layers 0–1 are shared machinery implementing no ports (the `provider-wire` /
`store-common` / `net-guard` precedent). Layer 3 surfaces exclusively through
the existing `Toolset` and `ContextProvider` ports.

## 2. Goals

1. A common search contract (`SearchScope`, `SearchStrategy`, `SourceRef`,
   `SearchHit`, `SearchSource`) that four heterogeneous sources implement
   without leaking their storage models.
2. Deterministic hybrid retrieval: reciprocal rank fusion (RRF) as the
   default fusion, weighted-sum as the configurable alternative, stable
   tie-breaks, typed-reference dedup.
3. Memory semantic search delivered first (slice 1) as a self-contained
   upgrade to `finstack-ai-memory`, honoring its README's non-breaking
   promise for `MemoryQuery`.
4. Document/artifact search whose **default is offline lexical**
   (FTS5/BM25 + bounded literal/regex), with chunk-level semantic vectors as
   an opt-in and lexical+semantic hybrid on top.
5. A journal search source fed by an observer (never the append path),
   backfillable via `JournalStore::scan`.
6. A graph/ontology source: sqlite property graph over a small controlled
   vocabulary, populated by conservative rule-based extraction, usable as a
   standalone strategy and as a query-expansion stage in hybrid plans.
7. Every index maintained eventually (reconciler + digest guards),
   rebuildable from its system of record, never in a run's critical path.

## 3. Non-goals

- **A new runtime port.** Six ports stay frozen. The façade composes
  `Toolset`/`ContextProvider`/`Observer`. If any slice appears to require a
  port or kernel change, that workstream stops and goes to change control.
- **`ArtifactStore` / `JournalStore` trait changes.** Both are consumed
  read-only through their existing surfaces.
- **A unified vector or graph database.** No tantivy, no ANN library, no
  graph DB dependency. Sqlite FTS5 + BLOB vectors + relational graph tables,
  brute-force exact KNN under the existing record ceilings. ANN is
  re-evaluated only if a corpus outgrows brute force (expected candidate:
  documents, slice 4+), as its own decision with deny.toml review.
- **Formal ontology stacks.** No OWL/RDF/SPARQL. The "ontology" is a
  validated vocabulary of entity/edge types. Oxigraph is the named escape
  hatch if formal reasoning ever becomes a requirement.
- **Model-based graph extraction** (decision D3): rule-based only until the
  graph proves value.
- **Cross-language embedder/search exposure** (Python/TS/WIT): staged
  follow-ups. Python currently touches none of the affected types; the wasm
  host-memory wire enum degrades safely (§11); WIT has no memory/search
  surface at all.
- **Chunking machinery in shared crates.** Chunking policy belongs to the
  documents indexer; memory never chunks (4 KiB inline cap).

## 4. Layer 0 — embeddings (`extensions/embeddings/`)

Two crates; both implement no ports.

**`finstack-ai-embeddings`** — dependency-free, wasm-buildable:

```rust
pub struct TextEmbedderDescriptor {
    pub embedder_id: Arc<str>,   // stable identity: model + revision + dims,
                                 // e.g. "embed.ollama.nomic-embed-text.768"
    pub dimensions: usize,
    pub max_input_bytes: usize,
}
pub trait TextEmbedder: PortObject {
    fn descriptor(&self) -> TextEmbedderDescriptor;
    fn embed(&self, texts: Vec<Arc<str>>)
        -> PortFuture<Result<Vec<EmbeddingVector>, EmbedError>>; // batch-shaped for backfill
}
pub struct EmbeddingVector(/* Arc<[f32]> */);
```

- `EmbeddingVector::try_new` rejects empty, over-long, and non-finite
  components; `PartialEq`/`Eq`/`Hash` are implemented over `f32::to_bits`
  (sound once NaN/inf are banned), which is what lets `MemoryQuery` keep its
  derived `Eq`. Helpers: unit normalization, dot product, char-boundary
  input truncation to `max_input_bytes`.
- `HashEmbedder`: deterministic token-hash bag-of-words at fixed dims. The
  reference implementation for tests, examples, and offline golden
  questions. Never the production default.
- Embedder identity discipline (convention, documented in rustdoc): a
  changed model revision is a **new `embedder_id`**, i.e. a new space.

**`finstack-ai-embedder-ollama`** — `TextEmbedder` over `POST /api/embed`.
Follows the ollama *provider's* HTTP conventions (own `reqwest` client,
redirects disabled, `vendored-tls` feature parity) rather than `net-guard`,
whose private-IP/loopback vetting is the wrong default for a localhost
daemon. Depends on `finstack-ai-embeddings`, never on `finstack-ai-memory`.
Extension→extension edges are legal per `scripts/ci/check_layering.py`.

## 5. Layer 1 — search contract (`extensions/search/finstack-ai-search-core`)

Dependency-light shared machinery (kernel + runtime + embeddings only), no
ports, wasm-buildable.

```rust
pub struct SearchScope {          // superset scope; each source maps it explicitly
    pub tenant: Arc<str>,
    pub user: Option<Arc<str>>,
    pub agent: Option<Arc<str>>,
    pub workspace: Option<Arc<str>>,
}
pub enum LexicalKind { Keyword, Bm25, Literal, Regex }
pub enum SearchStrategy {         // #[non_exhaustive]
    Lexical(LexicalKind),
    Semantic { space: Arc<str> }, // embedder_id
    Graph(GraphQuery),            // entity | neighborhood { depth } | path
}
pub enum SourceRef {              // #[non_exhaustive] — typed refs drive dedup + citation
    Memory { id: Arc<str> },
    ArtifactChunk { artifact: ArtifactRef, ordinal: u32 },
    JournalSpan { session: Arc<str>, lane: Arc<str>, first_event: Arc<str>, last_event: Arc<str> },
    Entity { id: Arc<str> },
}
pub struct SearchHit {
    pub source: Arc<str>,         // source_id from the descriptor
    pub reference: SourceRef,
    pub score: u32,               // per-leg ordering only; never comparable across legs
    pub preview: Arc<str>,
    pub sensitivity: Sensitivity, // propagated, never dropped by fusion
    pub provenance: SearchProvenance,
}
pub trait SearchSource: PortObject {
    fn descriptor(&self) -> SearchSourceDescriptor; // source_id, supported strategies, durable
    fn search(&self, scope: SearchScope, query: SearchQuery, limit: usize)
        -> PortFuture<Result<Vec<SearchHit>, SearchError>>;
}
```

**Hybrid + fusion.** `HybridPlan { legs: Vec<(source_id, SearchStrategy, weight)>, fusion }`
with `Fusion::Rrf { k }` (default, `k = 60`) and `Fusion::WeightedSum`.
Fusion is a pure function over per-leg ranked lists: RRF is rank-based, so
BM25 floats, cosine similarities, and keyword counts never need a common
scale — the same insight behind the memory provider's evidence tiers,
generalized. Dedup merges hits with equal `SourceRef` (evidence from every
contributing leg is retained); cross-source hits interleave by fused score;
ties break deterministically on `(source, reference)`. An unsupported or
failing leg drops out of the plan without failing the query; the fused
result records which legs ran (`FusedHit.evidence` carries per-leg ranks).

**Scope mapping (decision D1).** Configurable per source at composition
time via a declarative `ScopeMapping` on each adapter; the default is the
strictest mapping the source supports (memory: exact complete scope;
journal: tenant + session filters). Every adapter documents its mapping in
its descriptor rustdoc and tests it explicitly — federation's only new
security surface is this mapping, so it is never implicit.

The contract is **allowed to break freely until two sources implement it**
(end of slice 3), then hardens under the public-API baseline.

## 6. Layer 2 — sources

Shared indexing discipline, restated once and binding on all four:
derived from a system of record; maintained eventually via idempotent
reconciliation with digest/staleness guards; rebuildable from scratch;
best-effort — index maintenance never fails a write, a tool, or a run;
lexical strategies work offline; semantic/graph strategies are composition
opt-ins.

### 6.1 Memory (slice 1 — the store-level design)

Upgrades `finstack-ai-memory` in place; the store API stays `MemoryQuery`,
and the reserved extension point is exercised exactly as its README
promised.

- **Query/evidence:** `MemoryQuery::Embedding { embedder_id: Arc<str>, vector: EmbeddingVector }`
  (the enum is `#[non_exhaustive]`; in-crate exhaustive matches in
  `in_process.rs`, `sqlite/queries.rs`, `provider.rs`, `toolset.rs` are
  compiler-forced updates). `MatchEvidence::Semantic` added and
  `MatchEvidence` marked `#[non_exhaustive]` (it is the structural dual of
  `MemoryQuery`; no downstream exhaustive matches exist today).
- **Multi-space index, trait additions (all default-implemented — object
  safety and host adapters preserved):**

  ```rust
  pub struct EmbeddingSource { pub scope: MemoryScope, pub id: MemoryId,
                               pub text: Arc<str>, pub source_digest: Digest }
  fn pending_embedding_sources(&self, embedder_id, limit) -> ...;  // default Ok(vec![])
  fn store_embedding(&self, embedder_id, scope, id, source_digest, vector) -> ...;
      // default Err(InvalidRequest("memory_embeddings_unsupported"))
  fn forget_embedding_space(&self, embedder_id) -> ...;            // default Ok(()) — rotation
  ```

  "Pending" is **derived, not queued**: live records with no index row for
  that space (anti-join). Schema upgrade makes every pre-existing record
  pending, so backfill is the first drain. `store_embedding` recomputes the
  record's current source digest and silently no-ops on mismatch (record
  rewritten mid-reconcile; the anti-join re-surfaces it).
- **Canonical source text:** one shared helper
  `embedding_source_text(&MemoryRecord)` — preview + inline body + keywords,
  the same fields FTS indexes; blob bodies embed preview+keywords only.
  Digest domain `"memory-embed-source"` v1; changing the helper requires a
  version bump and dropping spaces.
- **Reconciler:** `reconcile_memory_embeddings(store, embedder, limit) -> Result<usize, _>`
  beside `reconcile_memory_artifacts`. Deliberate contrast with the artifact
  outbox: pin/unpin is ownership-critical and fails the tool; the embedding
  index is derived data and is best-effort everywhere.
- **Limits:** `MemoryStoreLimits` gains `max_embedding_dimensions` (4096)
  and `max_embedding_spaces` (4). First vector in a space fixes its
  dimensionality; mismatched writes/queries are `InvalidRequest`; a new
  space beyond the cap is `CapacityExceeded`. (Constructed in-crate only;
  all other `MemoryStoreLimits` in the workspace are the journal store's
  identically-named type.)
- **Sqlite schema v3:** `SCHEMA_USER_VERSION 2 → 3`; new table
  `memory_embeddings(scope_digest, id, embedder_id, dimensions,
  source_digest, vector BLOB, embedded_at, PRIMARY KEY(scope_digest, id,
  embedder_id))` + space index. `write_record` deletes the record's rows in
  the same transaction (beside its FTS/keyword resets); forget/correct
  sites and the expiry sweep mirror the FTS deletes. Three new worker
  commands; brute-force cosine over scope+space-filtered rows on the worker
  thread (≤ 4,096 rows — milliseconds; no `sqlite-vec`).
- **Scoring:** vectors unit-normalized at write; one shared scorer maps
  dot ∈ [−1, 1] to `u32` in `0..=1_000_000`, so in-process and sqlite
  rankings are byte-identical (cross-store agreement test mirrors
  `sqlite_and_in_process_full_text_normalization_match`).
- **Surfaces:**
  - `MemoryContextProvider`: optional embedder; third search leg merged by
    id with the existing two; evidence tiers `ExactId/Keyword = 0`,
    `FullText = 1`, `Semantic = 2`. The semantic leg is **skip-on-any-error**
    (embedder down, store unsupported): enabling semantic must never make
    recall worse; lexical legs keep the availability contract. Embedder
    descriptor joins the provider identity JSON; descriptor version → 0.2.0.
  - `MemoryToolset`: `search_memory` gains `mode: "lexical" | "semantic"`
    (valid only with `text`; advertised only when an embedder is
    configured). Explicit semantic requests **do** surface errors (stable
    reason `memory_semantic_unavailable`) — implicit recall degrades, an
    explicit ask fails honestly. Mutating tools add a bounded,
    error-swallowed embedding drain after `reconcile_artifacts`.
  - `MemoryObserver`: optional embedder; best-effort drain after capture
    batches.
- **Slice 2 adapter:** a thin `SearchSource` mapping
  `Lexical/Keyword → Keywords`, `Lexical/Bm25 → FullText`,
  `Semantic → Embedding`; `Literal`/`Regex`/`Graph` unsupported.

### 6.2 Documents / artifacts (`extensions/search/finstack-ai-index-documents`)

Artifacts are immutable and content-addressed — the easiest lifecycle:
chunks keyed by `(artifact_ref, chunker_config_digest)` never go stale;
they are orphaned when the artifact is unpinned (GC hooks the existing
pin/orphan protocol of `finstack-ai-store-artifact`).

- **Ingestion trigger (decision D2, refined):** indexing is an explicit,
  committed, **idempotent** tool effect — an `index_document` tool exposed
  by this crate's toolset, keyed by `(artifact digest, chunker digest)` so
  replay is a no-op. Ingest flows invoke it as a standard step: the
  knowledge app's `ingest` command calls it after staging, and agent-driven
  ingestion composes it into the ingest instruction flow. The
  `Middleware`-port chain itself never writes the index — the runtime
  contract requires middleware to stay pure (recovery re-runs the chain), so
  `middleware-document-ingest` keeps its current role (model-visible
  conversion only). This preserves the intent "an explicit index tool call
  as a step in the ingest path" while conforming to the port contract.
- **Extraction:** re-parse from artifact bytes via
  `finstack_ai_tools_document::parser` (the same parser the ingest
  middleware uses). Deterministic re-parse over cache-sharing: rebuildability
  wins.
- **Storage:** own sqlite file (path chosen by the composing app): chunk
  table + FTS5 (`bm25`) + optional per-chunk vector BLOBs reusing Layer 0.
  Chunking policy (target size, overlap, heading-aware splits) lives here
  and is part of the chunker config digest.
- **Strategies:** `Keyword`/`Bm25` default (offline); `Literal`/`Regex` over
  stored chunk text via the workspace `regex` crate (pattern size and scan
  bounds enforced); `Semantic` opt-in (slice 4); hybrid lexical+semantic
  within the source via Layer 1 fusion.

### 6.3 Journal (`extensions/search/finstack-ai-index-journal`)

The journal is the system of record for runs; **nothing touches the append
path**. The sanctioned tap is an `Observer` consuming committed events into
a derived FTS index. Durable/terminal event kinds carry only ids/digests —
indexable text arrives in `ModelTextDelta`, grouped per
`(run_id, model_request_id)` exactly as `RuleBasedExtractor` already does
(reuse that grouping approach). Historical backfill replays
`JournalStore::scan` (a default-implemented port method — no port change)
through the same indexing code; the index is rebuildable by re-scan.
Strategies: `Keyword`/`Bm25`/`Literal`/`Regex` with session/lane/time
filters; hits are `JournalSpan` refs. Semantic over journals is deliberately
deferred until a need is demonstrated.

### 6.4 Graph / ontology (`extensions/search/finstack-ai-index-graph`)

Sqlite property graph: `entities(id, kind, label)`, `entity_aliases`,
`edges(src, kind, dst, weight)`, `entity_sources(entity, source_ref, provenance)` —
every entity/edge cites the `SourceRef`s it was extracted from. The
**vocabulary** (allowed entity kinds and edge kinds) is validated
configuration data, not a formal ontology. Population runs through a
`GraphExtractor` trait mirroring `MemoryExtractor`'s conservatism;
rule-based default; model-based extraction is out of scope (D3) and, when
revisited, is an explicit application opt-in with its own trust review
(extracted entities originate in untrusted document content). Queries:
entity lookup, k-hop neighborhood (bounded recursive CTE), path between
entities. Two roles: a standalone `Graph` strategy source (`Entity` hits),
and a **query-expansion stage** in hybrid plans — query text → linked
entities → neighbors → expanded terms seeding lexical/semantic legs.

## 7. Layer 3 — façade (`extensions/search/finstack-ai-search`)

A registry of `Arc<dyn SearchSource>` plus two surfaces over existing
ports, composed (never implemented) by apps:

- **Toolset:** `search { text, strategy?, sources?, limit? }`. With no
  strategy, runs the composition's default `HybridPlan` across registered
  legs; results carry per-leg evidence (which legs ran, per-leg ranks) so
  answers can cite *why* something matched. Component ids `finstack.search.*`.
- **Context provider (opt-in):** budgeted cross-source recall emitting
  `Reference` items with `ContextAuthority::Untrusted`, provenance from
  `SourceRef`s, and a deterministic cache key — the `MemoryContextProvider`
  pattern generalized. `MemoryContextProvider` is not removed; whether the
  global provider supersedes it is decided empirically in slice 3+ (D4).

## 8. Security posture

- **Scope mapping** is per-source, declarative, configurable, and tested
  (D1, §5). Tenant always required end-to-end.
- **Sensitivity** rides every hit; fusion and both façade surfaces preserve
  the highest applicable classification; recall items stay
  `ContextAuthority::Untrusted`.
- **Egress gates:** with no embedder configured, no memory or document
  content ever leaves local storage; the default embedder choice is the
  local ollama daemon. Configuring a remote embedder is the explicit egress
  decision, made in app config, never inside a library.
- **Untrusted content:** documents and model output feed indexes; regex
  strategies bound pattern size and scan cost; graph extraction stays
  rule-based (D3); nothing in an index changes run semantics — indexes are
  read at query time only.

## 9. Workspace placement, dependencies, compatibility

New families/crates (all `publish = false`, standard extension lint headers,
workspace members + `[workspace.dependencies]` entries, `check-layering`
clean):

```text
extensions/embeddings/finstack-ai-embeddings        (L0, dep-free, wasm-buildable)
extensions/embeddings/finstack-ai-embedder-ollama   (L0 impl: reqwest — existing workspace dep)
extensions/search/finstack-ai-search-core           (L1, wasm-buildable)
extensions/search/finstack-ai-search                (L3 façade)
extensions/search/finstack-ai-index-documents       (L2; rusqlite, regex, tools-document — all existing)
extensions/search/finstack-ai-index-journal         (L2; rusqlite — existing)
extensions/search/finstack-ai-index-graph           (L2; rusqlite — existing)
```

**Zero new workspace dependencies across all six slices.** deny.toml's
tight license allowlist and `all-features` wasm32 graph scan are unaffected;
L0/L1 stay out of native-dep territory so the wasm binding graph stays
clean.

Compatibility and baselines:

- `finstack-ai-memory` public-surface changes (new variants, trait methods,
  limits fields) regenerate
  `fixtures/compatibility/public-rust-api/cargo-public-api/finstack-ai-memory.txt`
  via `mise run write-public-api`; new crates add their baselines the same
  way. New stable reason strings (`memory_embeddings_unsupported`,
  `memory_semantic_unavailable`, dimension/space codes, search/index codes)
  land in `fixtures/compatibility/error-codes/v1/codes.json`.
- **Python bindings: zero changes** (they consume memory via
  provider/toolset only; semantic recall arrives through composition).
- **wasm:** `host_memory.rs`'s `QueryWire` wildcard arm already degrades
  unknown query kinds safely; extending the wire enum so JS host stores can
  implement `Embedding` is a recorded parity asymmetry, deferred.
- **WIT/plugins: zero** (no memory/search surface exists there).

## 10. Validation

- Per-crate only, per `AGENTS.md`:
  `cargo nextest run -p <crate> --locked` (memory with `--features sqlite`)
  and matching `cargo clippy -p <crate> --all-targets --locked -- -D warnings`.
- `mise run check-public-api`, `mise run check-layering`, `cargo deny check`
  on every slice that adds a crate or public item.
- Determinism gates: cross-store ranking agreement (memory), fusion
  stability under leg reordering, `HashEmbedder`-based offline semantic
  tests everywhere; scripted `/api/embed` loopback (extending the
  `serve_ndjson` pattern) for app-level golden questions.
- Conformance: each `SearchSource` runs one shared contract test suite
  (scope mapping, strategy support declaration honesty, sensitivity
  propagation, determinism) from `search-core`'s test support.

## 11. Delivery — slices and plan documents

Vertical slices; each ships standalone value; the contract may break until
the end of slice 3, then hardens. Plan documents follow the house checkbox
convention and are authored **at wave start** (not all up front, to avoid
stale plans):

| Slice | Content | Plan doc(s) |
|---|---|---|
| 1 | `finstack-ai-embeddings` + memory semantic (core+in-process; sqlite v3; surfaces; `finstack-ai-embedder-ollama` + knowledge-app opt-in `memory_embedder`) | `2026-08-28-global-search-{a,b,c,d}-*.md` |
| 2 | `search-core` contract + memory `SearchSource` adapter + minimal façade toolset; first hybrid (lexical+semantic within memory) | `…-e-contract-facade.md` |
| 3 | documents index: BM25/keyword default + literal/regex; `index_document` tool; ingest-flow wiring; hybrid across memory+documents; **contract hardens; D4 decided** | `…-f-documents-lexical.md` |
| 4 | opt-in chunk vectors for documents; lexical+semantic hybrid over the corpus | `…-g-documents-semantic.md` |
| 5 | journal index source (observer-fed, scan backfill) | `…-h-journal.md` |
| 6 | graph source + vocabulary + expansion strategy in hybrid plans | `…-i-graph.md` |

Slice 1's four plans map to the previously reviewed memory design: **a** =
embeddings crate + memory core/in-process store, **b** = sqlite schema v3,
**c** = provider/toolset/observer surfaces, **d** = ollama embedder +
knowledge-app wiring (config default `None`, startup backfill drain, one
semantic golden question).

## 12. Resolved decisions

- **D1 — scope mapping:** configurable per source via declarative
  `ScopeMapping` at composition; default is the strictest mapping the
  source supports; every adapter documents and tests its mapping.
- **D2 — indexing trigger:** explicit `index_document` tool call as a step
  in ingest flows. Refinement (runtime-contract conformance): the tool is a
  committed idempotent effect invoked by ingest *flows* (CLI command, agent
  instruction flow); the `Middleware`-port chain stays pure and never
  writes the index.
- **D3 — graph extraction:** rule-based only until the graph proves value;
  model-based extraction is a future explicit opt-in with its own trust
  review.
- **D4 — provider succession:** `MemoryContextProvider` vs. global recall
  provider decided empirically in slice 3+, once both exist.

## 13. Open questions

1. `MatchEvidence::Semantic` payload: unit variant vs. carrying
   `embedder_id`. Default: unit (smallest surface).
2. Aggregate embedding byte ceiling: implicit (records × dims × spaces,
   ~256 MB theoretical worst case) vs. an explicit
   `max_embedding_bytes` limit. Default: implicit; revisit if limits become
   user-configurable.
3. RRF `k` and per-leg weights: fixed defaults vs. exposed in façade
   config. Default: configurable in `HybridPlan`, fixed in the shipped
   default plan.
4. Chunking defaults (target size, overlap, heading-awareness): settle in
   the slice-3 plan with corpus experiments.
5. Binary artifacts with no text extraction (images, media): out of scope
   for lexical/semantic; revisit alongside the media-pipeline initiative if
   caption/OCR-derived text ever becomes an index input.
