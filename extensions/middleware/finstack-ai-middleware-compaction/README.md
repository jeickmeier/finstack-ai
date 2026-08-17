# finstack-ai-middleware-compaction

> **Status: sliding-window and large-tool-output land.** Summarize
> (`RequestCompactionModel`) stays unlandable. That remaining gap is a
> framework limitation, not a bug in the strategies. Read
> [Why summarize compaction cannot complete](../../../docs/site/middleware.md#why-summarize-compaction-cannot-complete)
> before working on it.

One `MiddlewareRole::ContextCompactor` leaf. Strategies are selected by
configuration, not by registering a second compactor:

- `finstack.compaction.sliding_window`
- `finstack.compaction.large_tool_output`
- `finstack.compaction.summarize`

Deterministic strategies complete as `CompactContext`. Summarize returns
`RequestCompactionModel` and never depends on a `Model` handle. Canonical
history is not mutated.

## Landing

- **`CompactContext` lands** when the last source entry is a protected
  user. That bit is authoritative-from-the-context-port plus the
  structural rule (system/developer and the trailing current user). A
  compactor still cannot set it.
- **`RequestCompactionModel` has no landing path** in the aggregate-fold
  design at all: it needs a committed child model effect under a
  committed middleware parent, and no middleware invocation is ever a
  committed effect. It fails with `middleware_stage_unlandable`. This
  one needs a design change, not port wiring.

This crate is a T1 native adapter. It is not isolated.

```rust
use finstack_ai_middleware_compaction::{CompactionConfig, CompactionMiddleware};

let config = CompactionConfig::sliding_window(1_024, 256);
let middleware = CompactionMiddleware::try_new(config).expect("compaction");
```
