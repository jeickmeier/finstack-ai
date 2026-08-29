# Plan C: memory semantic surfaces (provider, toolset, observer)

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development or superpowers:executing-plans, task-by-task. Steps use checkbox syntax.

**Goal:** Semantic retrieval reaches compositions: the recall provider
gains an optional third search leg, `search_memory` gains an explicit
`mode`, and the capture observer drains the index after batches. Failure
semantics per spec §6.1: **implicit recall degrades silently; an explicit
semantic ask fails honestly; index maintenance never fails anything.**

**Architecture:** The embedder is an optional constructor argument
(`Option<Arc<dyn TextEmbedder>>` internally) on all three components — no
behavior change whatsoever when absent (existing tests must stay green
untouched, except the provider version bump). Spec §6.1.

**Tech Stack:** Rust; `finstack-ai-memory` + `finstack-ai-embeddings`
(`HashEmbedder` and a purpose-built failing embedder drive the tests).

**Spec:** `docs/superpowers/specs/2026-08-28-global-search-design.md`
**Depends on:** Plan A complete (Plan B recommended first so sqlite tests can
exercise the surfaces, but only A is required to compile).

## Global Constraints

- Verify per task: `cargo nextest run -p finstack-ai-memory --features sqlite --locked`
  and matching clippy `-D warnings`; build once without the feature.
  Never run workspace-wide tests.
- Public-surface or stable-reason changes regenerate baselines via
  `mise run write-public-api` in the same task.
- One commit per task; short imperative subject; do not push.

---

### Task C1: provider — optional semantic leg

**Files:**
- Modify: `src/provider.rs`
- Test: `src/tests/provider.rs` (extend)

**Interfaces:**
- `MemoryContextProvider::try_new_with_embedder(store, artifact_store: &dyn ArtifactStore, scope, config, embedder: Arc<dyn TextEmbedder>) -> Result<Self, MemoryError>`;
  existing `try_new` unchanged.
- Identity JSON gains `"embedder": { "id", "dimensions" }` when configured;
  descriptor version `0.1.0 → 0.2.0` (unconditionally — the component's
  behavior contract changed).
- `search_candidates` grows a third leg when the embedder is present:
  truncate the derived query text to `descriptor.max_input_bytes`
  (`truncate_to_bytes`), `embed`, issue
  `MemoryQuery::Embedding { embedder_id, vector }`, merge by id into the
  existing map (max score, strongest evidence tier). **The semantic leg is
  skip-on-any-error** — embedder failure, store `InvalidRequest`, store
  `Unavailable`, anything: the leg contributes nothing and the other legs
  proceed. Tier ordering: `ExactId/Keyword = 0`, `FullText = 1`,
  `Semantic = 2` (arm added in Plan A).

- [ ] **Step 1:** Failing tests: with `HashEmbedder` + a seeded store, a
  paraphrase query with no keyword/substring overlap recalls the record via
  the semantic leg with `Semantic` evidence, ordered after any lexical
  hits; a record hit by both legs appears once with the stronger (lexical)
  evidence; a failing embedder (always-`Err` test impl) degrades to
  keyword/FTS results with no error; a store whose `search` rejects
  `Embedding` (default-impl behavior) likewise degrades; configuration
  digest differs with vs. without embedder and between embedder ids;
  cache-key stability across identical collects still holds.
- [ ] **Step 2:** Implement.
- [ ] **Step 3:** Green (existing provider tests unmodified except the
  version assertion); clippy; `mise run write-public-api`.
- [ ] **Step 4:** Commit `Recall memory through optional semantic leg`

### Task C2: toolset — explicit `mode` + post-mutation drain

**Files:**
- Modify: `src/toolset.rs`
- Test: `src/tests/toolset.rs` (extend)

**Interfaces:**
- `MemoryToolset::try_new_with_embedder(store, artifact_store, scope, policy, clock, embedder) -> Result<Self, MemoryError>`;
  existing `try_new` unchanged.
- `SearchArguments` gains `mode: Option<String>` — accepted values
  `"lexical"` (default) and `"semantic"`; any other value, `mode` without
  `text`, or `"semantic"` on a toolset with no embedder is
  `invalid_arguments` with stable reason `memory_semantic_unavailable`
  where the embedder is the missing piece, `memory_query_invalid`
  otherwise. The tool's JSON schema and description advertise `mode` only
  when an embedder is configured.
- Semantic path: embed `text` (embedder failure → retryable tool error
  `memory_semantic_unavailable`), issue the `Embedding` query, render hits
  as today (`matched: "semantic"` from Plan A).
- After the existing `reconcile_artifacts` in `remember`/`forget`/
  `correct`: a bounded (`limit 16`), **error-swallowed**
  `reconcile_memory_embeddings` drain when an embedder is configured.
  Contrast with artifacts is deliberate and documented: pin/unpin is
  ownership-critical and fails the tool; the embedding index is derived
  data and never does.

- [ ] **Step 1:** Failing tests: schema advertises `mode` only with an
  embedder; `mode: "semantic"` without embedder → stable error; with
  `HashEmbedder` returns semantic hits; failing embedder →
  `memory_semantic_unavailable` on explicit search but `remember` still
  succeeds (drain swallowed); after `remember`, pending for the space is
  empty (drain indexed it); `mode: "lexical"` and omitted `mode` behave
  identically to today.
- [ ] **Step 2:** Implement.
- [ ] **Step 3:** Green; clippy; `mise run write-public-api` (new reason
  string).
- [ ] **Step 4:** Commit `Expose semantic mode on memory search tool`

### Task C3: observer — post-batch drain

**Files:**
- Modify: `src/observer.rs`
- Test: `src/tests/observer.rs` (extend)

**Interfaces:**
- `MemoryObserver::try_new_with_embedder(store, scope, extractor, clock, embedder) -> Result<Self, MemoryError>`;
  existing `try_new` unchanged.
- After a batch's candidate `put` loop: bounded (`limit 16`),
  error-swallowed `reconcile_memory_embeddings`. Observer failure isolation
  is preserved absolutely — a failing embedder changes no observer result,
  no stats beyond an optional `last_diagnostic`, and no run outcome.

- [ ] **Step 1:** Failing tests: captured candidates become embedded
  (pending empty) with `HashEmbedder`; a failing embedder leaves capture
  results and existing stats identical to the no-embedder run.
- [ ] **Step 2:** Implement.
- [ ] **Step 3:** Green; clippy; `mise run write-public-api`.
- [ ] **Step 4:** Commit `Drain memory embeddings from capture observer`

### Task C4: crate documentation

**Files:**
- Modify: `extensions/context/finstack-ai-memory/README.md` — component
  list gains the semantic index and reconciler; construction example shows
  `try_new_with_embedder`; the **"Future work: vector retrieval"** section
  is replaced by a "Semantic retrieval" section (multi-space model,
  eventual indexing, failure semantics, egress note: configuring an
  embedder is the decision that memory content may leave the store) linking
  the global-search spec.
- Modify: rustdoc touch-ups on items added in A/B/C where review found
  gaps.

- [ ] **Step 1:** Docs updated; doc tests compile
  (`cargo test -p finstack-ai-memory --doc --features sqlite --locked`).
- [ ] **Step 2:** Clippy + nextest still green; `mise run check-public-api`
  clean.
- [ ] **Step 3:** Commit `Document memory semantic retrieval`
