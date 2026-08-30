# finstack-ai-memory

Memory extension composition for finstack-ai: one crate implementing store,
recall, model-driven tools, and eventual capture as a cohesive set of
components over a shared record model, rather than another standalone recall
crate. It supersedes `finstack-ai-context-memory`, which implemented only the
recall half of this composition.

```text
finstack-ai-memory
  MemoryStore (trait) ── InProcessMemoryStore, SqliteMemoryStore (feature "sqlite")
                         + per-space embedding index (default-implemented surface)
  MemoryContextProvider  — automatic budgeted recall (ContextProvider port)
  MemoryToolset          — remember, search_memory, inspect_memory, forget_memory, correct_memory (Toolset port)
  MemoryObserver         — eventual asynchronous capture (Observer port)
  MemoryExtractor (trait)— shared candidate-memory extraction
  reconcile_memory_embeddings — bounded eventual drain of the embedding index
```

All three port surfaces take an optional `TextEmbedder`
(`finstack-ai-embeddings`) through a `try_new_with_embedder` constructor;
without one, behavior is exactly the lexical composition described below.

## Components

- **`MemoryStore`** — the object-safe, `Send + Sync` storage trait: `put`,
  `get`, `search`, `forget`, `correct`, `list`. Every mutating call takes an
  idempotency key; replaying the same key returns `PutOutcome::AlreadyApplied`
  instead of duplicating, which is what makes tool-effect replay and observer
  redelivery safe. The embedding-index surface —
  `pending_embedding_sources`, `store_embedding`, `forget_embedding_space` —
  is default-implemented so existing store implementations keep compiling; a
  store without an index simply reports no pending work and rejects vector
  writes (`memory_embeddings_unsupported`).
  - `InProcessMemoryStore` — a mutex-guarded reference implementation. Exact
    id, case-insensitive keyword, naive substring full-text, and brute-force
    cosine semantic matching. Compiles on `wasm32`; used by tests, examples,
    and as the default when no persistence is configured.
  - `SqliteMemoryStore` (feature `sqlite`) — FTS5/BM25 full-text retrieval,
    schema migrations, and idempotency-key tracking in a SQLite database.
    Native-only.
- **`reconcile_memory_embeddings(store, embedder, limit)`** — the bounded,
  idempotent drain that turns pending records into index rows (see
  [Semantic retrieval](#semantic-retrieval)). Its sibling
  `reconcile_memory_artifacts` drains the artifact-ownership outbox.
- **`MemoryContextProvider`** — implements the `ContextProvider` port.
  Derives a query from the request's user input, ranks and dedupes hits
  (dropping superseded/tombstoned records), and emits budgeted `ContextItem`s
  with provenance and `ContextAuthority::Untrusted`. Ranking is tiered with a
  stable id tiebreak, and a bounded stable prefix keeps top recalls in the
  same order across turns so the provider's cache key stays stable when
  nothing relevant changed. With an embedder
  (`try_new_with_embedder`), a third, best-effort semantic leg joins the
  keyword and full-text legs and recalls in its own tier after them.
- **`MemoryToolset`** — implements the `Toolset` port with five
  capability-gated tools: `remember` (write), `search_memory` /
  `inspect_memory` (read), `forget` / `correct` (manage). Gating is controlled
  by `MemoryPolicy { read, write, manage }`. Scope always comes
  from the toolset's configured `MemoryScope`, never from model-supplied
  arguments. With an embedder, `search_memory` additionally advertises
  `mode: "lexical" | "semantic"`, and the mutating tools drain the
  embedding index best-effort after each write.
- **`MemoryObserver`** — implements the `Observer` port for durable-eventual
  capture: it consumes committed run events, feeds them through a
  `MemoryExtractor`, and `put`s each candidate with a
  `capture:{run_id}:{event_id}:{candidate_index}` idempotency key. Observer
  failure never changes run results, per the port contract. With an
  embedder, each observed batch ends in the same best-effort index drain.
- **`MemoryExtractor`** — the trait behind capture. `RuleBasedExtractor`
  (the default) is deliberately conservative and low-recall; aggressive or
  model-based extraction is an application choice via a custom extractor.

## Feature flags

- `sqlite` — enables `SqliteMemoryStore` (via `rusqlite`). Off by default;
  `InProcessMemoryStore` has no optional dependencies and is always
  available.

## Construction example

```rust
use std::sync::Arc;

use finstack_ai_embeddings::embedder::HashEmbedder;
use finstack_ai_memory::provider::{MemoryContextProvider, RecallConfig};
use finstack_ai_memory::record::{MemoryScope, system_clock};
use finstack_ai_memory::store::{InProcessArtifactStore, InProcessMemoryStore};
use finstack_ai_memory::toolset::{MemoryPolicy, MemoryToolset};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let store = Arc::new(InProcessMemoryStore::new());
    let artifact_store = Arc::new(InProcessArtifactStore::default());
    let scope = MemoryScope::try_new("tenant-a")?;
    // Optional: any `TextEmbedder`. `HashEmbedder` is the offline reference
    // (token overlap, not meaning) — use `try_new` on both components
    // instead to compose without semantic retrieval.
    let embedder = Arc::new(HashEmbedder::try_new(256)?);

    let provider = MemoryContextProvider::try_new_with_embedder(
        store.clone(),
        artifact_store.as_ref(),
        scope.clone(),
        RecallConfig::default(),
        embedder.clone(),
    )?;

    let toolset = MemoryToolset::try_new_with_embedder(
        store,
        artifact_store,
        scope,
        MemoryPolicy { read: true, write: true, manage: true },
        system_clock(),
        embedder,
    )?;
    Ok(())
}
```

Register `provider` as a `ContextProvider` and `toolset` as a `Toolset` on
the agent builder; add a `MemoryObserver` over the same store (it, too,
accepts the embedder via `try_new_with_embedder`) when eventual capture
from run events is wanted.

## Deferred: capture middleware

The runtime's `Middleware` port states middleware is never a committed
effect and must be pure with respect to external state, since recovery
re-runs the chain. Until the runtime grows committed middleware effects, the
durable-write paths are the `remember` tool (already a committed, idempotent
tool effect) and `MemoryObserver`. A `MemoryCaptureMiddleware` — a thin
adapter over the same `MemoryExtractor` + idempotent `MemoryStore::put` — is
documented but intentionally not implemented here; it can be added without
changing the store or extractor traits once the runtime supports it.

## Semantic retrieval

The extension point earlier revisions of this README reserved is now
exercised, without the breaking change it promised to avoid:
`MemoryQuery::Embedding { embedder_id, vector }` ranks records by cosine
similarity over a per-store embedding index, and `MatchEvidence::Semantic`
marks the resulting hits. No embedding model, ANN library, or network fetch
ships in this crate — the embedder is injected
(`finstack_ai_embeddings::embedder::TextEmbedder`), and retrieval without
one remains exact-id, keyword, and full-text only.

- **Multi-space model.** Vectors live in named *spaces*, one per embedder
  identity (`TextEmbedderDescriptor::embedder_id`, e.g.
  `embed.hash-v1.256`); vectors are only comparable within one space, and a
  changed model revision is a new `embedder_id` — a new space, never a
  mutation of an existing one. The first vector stored in a space fixes its
  dimensionality; stores bound both axes via
  `MemoryStoreLimits::{max_embedding_spaces, max_embedding_dimensions}`.
  `forget_embedding_space` drops a space on embedder rotation.
- **Eventual indexing.** The index is derived, rebuildable data, never
  written in a query path. "Pending" is an anti-join of live records
  against a space's rows, drained by the bounded, idempotent
  `reconcile_memory_embeddings(store, embedder, limit)`; a source-digest
  guard makes writes for records rewritten mid-drain silent no-ops that
  re-surface on the next pass. Mutating tools and the capture observer
  drain a small batch (16) after each write; applications run larger
  drains for startup backfill.
- **Failure semantics.** Implicit recall degrades silently: the provider's
  semantic leg skips on any error (embedder down, store without an index),
  so configuring an embedder can never make recall worse than lexical
  recall alone. An explicit `search_memory` `mode: "semantic"` ask fails
  honestly with the stable reason `memory_semantic_unavailable`. Index
  maintenance never fails anything: the post-mutation and post-batch
  drains are error-swallowed — in deliberate contrast with artifact
  pin/unpin, which is ownership-critical and does fail the tool.
- **Egress.** With no embedder configured, no memory content leaves the
  store. Configuring one is the application's explicit egress decision:
  record text (preview, inline body, keywords) is sent to that embedder,
  so a remote embedder means memory content leaves the process. The
  offline `HashEmbedder` and local daemon embedders keep it at home.

Design, layering, and the slices that follow (documents, journal, graph):
[`docs/superpowers/specs/2026-08-28-global-search-design.md`](../../../docs/superpowers/specs/2026-08-28-global-search-design.md).
