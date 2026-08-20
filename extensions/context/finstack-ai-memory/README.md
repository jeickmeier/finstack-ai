# finstack-ai-memory

Memory extension composition for finstack-ai: one crate implementing store,
recall, model-driven tools, and eventual capture as a cohesive set of
components over a shared record model, rather than another standalone recall
crate. It supersedes `finstack-ai-context-memory`, which implemented only the
recall half of this composition.

```text
finstack-ai-memory
  MemoryStore (trait) ── InProcessMemoryStore, SqliteMemoryStore (feature "sqlite")
  MemoryContextProvider  — automatic budgeted recall (ContextProvider port)
  MemoryToolset          — remember, search_memory, inspect_memory, forget_memory, correct_memory (Toolset port)
  MemoryObserver         — eventual asynchronous capture (Observer port)
  MemoryExtractor (trait)— shared candidate-memory extraction
```

## Components

- **`MemoryStore`** — the object-safe, `Send + Sync` storage trait: `put`,
  `get`, `search`, `forget`, `correct`, `list`. Every mutating call takes an
  idempotency key; replaying the same key returns `PutOutcome::AlreadyApplied`
  instead of duplicating, which is what makes tool-effect replay and observer
  redelivery safe.
  - `InProcessMemoryStore` — a mutex-guarded reference implementation. Exact
    id, case-insensitive keyword, and naive substring full-text matching.
    Compiles on `wasm32`; used by tests, examples, and as the default when no
    persistence is configured.
  - `SqliteMemoryStore` (feature `sqlite`) — FTS5/BM25 full-text retrieval,
    schema migrations, and idempotency-key tracking in a SQLite database.
    Native-only.
- **`MemoryContextProvider`** — implements the `ContextProvider` port.
  Derives a query from the request's user input, ranks and dedupes hits
  (dropping superseded/tombstoned records), and emits budgeted `ContextItem`s
  with provenance and `ContextAuthority::Untrusted`. Ranking is tiered with a
  stable id tiebreak, and a bounded stable prefix keeps top recalls in the
  same order across turns so the provider's cache key stays stable when
  nothing relevant changed.
- **`MemoryToolset`** — implements the `Toolset` port with five
  capability-gated tools: `remember` (write), `search_memory` /
  `inspect_memory` (read), `forget` / `correct` (manage). Gating is controlled
  by `MemoryPolicy { read, write, manage, consolidate, profile }` — the last
  two are reserved capability names with no tools yet. Scope always comes
  from the toolset's configured `MemoryScope`, never from model-supplied
  arguments.
- **`MemoryObserver`** — implements the `Observer` port for durable-eventual
  capture: it consumes committed run events, feeds them through a
  `MemoryExtractor`, and `put`s each candidate with a
  `capture:{run_id}:{event_id}:{candidate_index}` idempotency key. Observer
  failure never changes run results, per the port contract.
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

use finstack_ai_memory::{
    InProcessArtifactStore, InProcessMemoryStore, MemoryContextProvider, MemoryPolicy,
    MemoryScope, MemoryToolset, RecallConfig, system_clock,
};

let store = Arc::new(InProcessMemoryStore::new());
let artifact_store = Arc::new(InProcessArtifactStore::default());
let scope = MemoryScope::try_new("tenant-a")?;

let provider = MemoryContextProvider::try_new(store.clone(), scope.clone(), RecallConfig::default())?;

let toolset = MemoryToolset::try_new(
    store,
    artifact_store,
    scope,
    MemoryPolicy { read: true, write: true, manage: true, consolidate: false, profile: false },
    system_clock(),
)?;
# Ok::<(), finstack_ai_memory::MemoryError>(())
```

Register `provider` as a `ContextProvider` and `toolset` as a `Toolset` on
the agent builder; add a `MemoryObserver` over the same store when eventual
capture from run events is wanted.

## Deferred: capture middleware

The runtime's `Middleware` port states middleware is never a committed
effect and must be pure with respect to external state, since recovery
re-runs the chain. Until the runtime grows committed middleware effects, the
durable-write paths are the `remember` tool (already a committed, idempotent
tool effect) and `MemoryObserver`. A `MemoryCaptureMiddleware` — a thin
adapter over the same `MemoryExtractor` + idempotent `MemoryStore::put` — is
documented but intentionally not implemented here; it can be added without
changing the store or extractor traits once the runtime supports it.

## Future work: vector retrieval

`MemoryQuery` and `MemoryStore` are shaped so an `Embedding` query variant
and a vector-backed store implementation can be added without a breaking
change. No embedding model, ANN index, or network fetch ships in this crate;
today's retrieval is exact-id, keyword, and full-text only.
