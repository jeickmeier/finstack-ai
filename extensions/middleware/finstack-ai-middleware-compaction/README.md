# finstack-ai-middleware-compaction

One `MiddlewareRole::ContextCompactor` leaf. Strategies are selected by
configuration, not by registering a second compactor:

- `finstack.compaction.sliding_window`
- `finstack.compaction.large_tool_output`
- `finstack.compaction.summarize`

Deterministic strategies complete as `CompactContext`. Summarize returns
`RequestCompactionModel` and never depends on a `Model` handle. Canonical
history is not mutated.
