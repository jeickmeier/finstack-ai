# finstack-ai-middleware-compaction

> **Status: does not work end to end.** Neither outcome this leaf
> produces can be applied to a run. This is a framework gap, not a bug
> in the strategies, and it is not fixed by changing this crate. Read
> [Why compaction cannot complete](../../../docs/site/middleware.md#why-compaction-cannot-complete)
> before working on it.

One `MiddlewareRole::ContextCompactor` leaf. Strategies are selected by
configuration, not by registering a second compactor:

- `finstack.compaction.sliding_window`
- `finstack.compaction.large_tool_output`
- `finstack.compaction.summarize`

Deterministic strategies complete as `CompactContext`. Summarize returns
`RequestCompactionModel` and never depends on a `Model` handle. Canonical
history is not mutated.

## The two blockers

- **`CompactContext` is rejected by `validate_compaction_result`**,
  which requires the last source entry to be `protected`. That bit is
  authoritative-from-the-context-port and the `ContextProvider` port has
  no production driver, so every entry a run hands this leaf is
  unprotected and every result fails with `compaction_result_invalid`.
  **Wiring `ContextProvider` is the unblock** — do not patch this crate
  and do not fabricate the `protected` bit at the seam.
- **`RequestCompactionModel` has no landing path** in the aggregate-fold
  design at all: it needs a committed child model effect under a
  committed middleware parent, and no middleware invocation is ever a
  committed effect. It fails with `middleware_stage_unlandable`. This
  one needs a design change, not port wiring.

The unit and conformance suites for this crate hand-construct their
source entries, including `protected: true` ones a live run cannot
produce, so they pass while the end-to-end path does not. Green tests
here are not evidence that compaction works in a run.
