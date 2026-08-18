# finstack-ai-middleware-compaction

> **Status: sliding-window, large-tool-output, and summarize land.**
> Summarize still never holds a `Model` handle; the runtime-owned
> phase (ADR-042) fulfills `RequestCompactionModel`. Read
> [Middleware](../../../docs/site/middleware.md)
> before working on it.

One `MiddlewareRole::ContextCompactor` leaf. Strategies are selected by
configuration, not by registering a second compactor:

- `finstack.compaction.sliding_window`
- `finstack.compaction.large_tool_output`
- `finstack.compaction.summarize`

Deterministic strategies complete as `CompactContext`. Summarize returns
`RequestCompactionModel` and never depends on a `Model` handle or a
middleware-owned child effect. Canonical history is not mutated.

## Landing

- **`CompactContext` lands** when the last source entry is a protected
  user. That bit is authoritative-from-the-context-port plus the
  structural rule (system/developer and the trailing current user). A
  compactor still cannot set it.
- **`RequestCompactionModel` is fulfilled by the runtime phase**
  (ADR-042), not by `fold.rs`. Middleware stays non-effect-bearing. If
  the outcome reaches `StageFold::accumulate`, it is still
  `middleware_stage_unlandable`.

This crate is a T1 native adapter. It is not isolated.

```rust
use finstack_ai_middleware_compaction::{CompactionConfig, CompactionMiddleware};

let config = CompactionConfig::sliding_window(1_024, 256);
let middleware = CompactionMiddleware::try_new(config).expect("compaction");
```
