# finstack-ai-middleware-compaction

> **Status: sliding-window, large-tool-output, and summarize land.**
> Summarize still never holds a `Model` handle; the runtime-owned
> phase fulfills `RequestCompactionModel`. Read the current runtime and
> middleware contracts before working on it.

One `MiddlewareRole::ContextCompactor` leaf. Strategies are selected by
configuration, not by registering a second compactor:

- `CompactionConfig::sliding_window(threshold_tokens, hysteresis_tokens)`
  (`finstack.compaction.sliding_window`)
- `CompactionConfig::large_tool_output(threshold_tokens, hysteresis_tokens, byte_limit)`
  (`finstack.compaction.large_tool_output`)
- `CompactionConfig::summarize(threshold_tokens, hysteresis_tokens, model, budget_scope, residency_policy_digest)`
  (`finstack.compaction.summarize`)

Deterministic strategies complete as `CompactContext`. Summarize never
depends on a `Model` handle or a middleware-owned child effect. Above its
threshold, the first invocation returns `RequestCompactionModel`; the runtime
authorizes and fulfills the related child effect, then reinvokes the same leaf
with `compaction_resume`. Configuration cannot self-authorize secondary-model
dispatch; the runtime owns authorization.
Canonical history is not mutated.

Evidence accounts for the complete rebuilt request plus derived summaries.
Large tool-output previews enforce the byte limit across the whole result,
truncate text only at UTF-8 boundaries, and replace oversized non-text blocks
with a bounded marker.

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
use finstack_ai_kernel::{BudgetScopeId, ComponentId, ComponentRef, Digest, Version};
use finstack_ai_middleware_compaction::{CompactionConfig, CompactionMiddleware};

let sliding = CompactionConfig::sliding_window(1_024, 256);
let large = CompactionConfig::large_tool_output(1_024, 256, 2_048);
let summarize = CompactionConfig::summarize(
    1_024,
    256,
    ComponentRef::new(
        ComponentId::parse("finstack.model.summarize").expect("id"),
        Some(Version {
            major: 0,
            minor: 0,
            patch: 4,
        }),
    ),
    BudgetScopeId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("budget"),
    Digest::raw_json(b"residency-policy"),
);
let middleware = CompactionMiddleware::try_new(sliding).expect("compaction");
let _ = (large, summarize, middleware);
```
