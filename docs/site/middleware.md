# Middleware

Workspace version is **1.0.0**. Middleware components run at seven
coarse stage boundaries. A run's registered components form one locked,
ordered chain; at each boundary the chain runs in order and its results
are **folded into exactly one settlement** for that `(cycle, stage)`
cursor. The kernel sees one outcome per cursor and never sees an
individual component.

That single-settlement rule is the whole design, and everything on this
page follows from it.

## Read this first

**The `finstack-ai-middleware-compaction` leaf does not work end to
end.** It is one of only two shipping middleware leaves, and neither of
its two outcomes can be applied to a run. This is not a bug in the
compactor. See [Shipping leaves](#shipping-leaves) below for the two
independent causes and the one change that unblocks the first of them.

## What an author must know

- **Replay safety is your obligation, not the framework's.** Individual
  invocations are never journaled. After a crash, a stage that had not
  finished settling re-runs its **entire** chain from the first
  component — including components that already returned. A component
  that performs a non-repeatable external action will repeat it. The
  `InvocationRecovery::NonRepeatable` marker does not help: nothing
  reads it at a stage boundary. Keep `invoke` pure with respect to
  external state.
- **Order is semantics.** The first `Fail` or `Retry` short-circuits the
  chain: components after it never run. Resolved order is order tier,
  then `before`/`after` constraints, then registration index. A
  component that must observe every run has to be ordered ahead of
  anything that can fail.
- **The effect id you are handed is not an effect.** No middleware
  invocation is a committed effect, so the `effect_id` in your call
  context (and in the WIT `call-context` a plugin guest receives) is a
  deterministic correlation id derived from
  `(locator, cycle, stage)`. It is not a journal key. Looking it up will
  find nothing.
- **Contributions are aggregated, not sequenced.** Added instructions
  and context concatenate in chain order, tool filters intersect, and a
  replacement re-bases the payload that additions then apply to. There
  is no cross-kind "last writer wins".

## What can be applied, and where

Every outcome first passes the stage/outcome matrix for its own
component. The fold then decides whether the aggregate has anywhere to
land. Both checks can reject; neither ever coerces.

| Outcome | Applied at |
| --- | --- |
| `Continue` | every stage (contributes nothing) |
| `Fail` | every stage |
| `AddInstructions` / `AddContext` | `prepare_context`, `before_model` |
| `FilterTools` | `before_model`, `before_tool_batch` |
| `Replace` | `prepare_context`, `before_model` only |
| `Retry` | `before_finalize` only |
| `CompactContext` | never applied today — see below |
| `RequestCompactionModel` | never |
| `RequestInteraction` | never |
| `Suspend` | never |
| `Complete` | never |

"Never" means the run fails with `middleware_stage_unlandable` rather
than the outcome being silently dropped. `Suspend`, `Complete`, and
`RequestInteraction` have no single-settlement shape: parking a run,
completing it, or opening an interaction each need a kernel input the
stage settlement cannot also emit. `Replace` outside the two
context-bearing stages, and `Retry` outside `before_finalize`, have no
settlement that can carry them.

## Shipping leaves

| Leaf | Status |
| --- | --- |
| [`finstack-ai-middleware-verify`](../../extensions/middleware/finstack-ai-middleware-verify/README.md) | Works in its `Accept` and `Fail` modes. Its `RequestInteraction` mode fails the run instead of prompting. |
| [`finstack-ai-middleware-compaction`](../../extensions/middleware/finstack-ai-middleware-compaction/README.md) | Does not work end to end. Both outcomes are unapplicable. |

### Why compaction cannot complete

The two causes are independent; fixing either alone is not enough.

**Deterministic strategies return `CompactContext`, which fails
validation.** `validate_compaction_result` requires the last source
entry handed to the compactor to be `protected`. `protected` is
authoritative-from-the-context-port: a compactor is not allowed to
assert it, precisely because the check exists to constrain the
compactor. The `ContextProvider` port has **no production driver** —
nothing in a live run collects a contribution or assembles context — so
no protected context item is ever created, every source entry is
structurally unprotected, and every compaction result is rejected with
`compaction_result_invalid`.

**Wiring the `ContextProvider` port is the unblock.** Patching the
compactor, or fabricating the `protected` bit where the source entries
are assembled, is not: it would let the constrained party certify
itself.

**The `summarize` strategy returns `RequestCompactionModel`, which has
no design slot at all.** It needs a committed child model effect
attached to a committed middleware parent, plus a re-entry into the
chain carrying the model's answer. A stage settles exactly once and
there is no committed middleware parent to attach a child to, so the
aggregate design has nowhere to put either half. This one is not
unblocked by the context port; it needs a design change.

Two further gaps sit behind the first cause and become live the moment
`ContextProvider` is wired. Both are documented at
`apply_model_draft` in
[`crates/finstack-ai-runtime/src/exec/stage_settlement/`](../../crates/finstack-ai-runtime/src/exec/stage_settlement/):
a landed compaction result drops its derived summaries and checkpoint,
and a chain producing both a replacement and a compaction would apply a
projection validated against the original message array on top of the
replacement array.

## Stable codes

Match on the `code` string; display messages may differ.

| Code | Meaning |
| --- | --- |
| `middleware_resolution_invalid` | The chain could not be resolved (duplicate component, bad index, second compactor). |
| `middleware_order_cycle` | `before`/`after` constraints do not admit a total order. |
| `middleware_outcome_not_allowed` | A component returned an outcome its stage or role does not permit. |
| `middleware_stage_unlandable` | The aggregate has no settlement shape at this cursor. See the table above. |
| `middleware_stage_bounds_exceeded` | Accumulated additions would exceed a kernel array bound. |
| `middleware_stage_identity_missing` | The run has no dispatch identity for the stage (unreachable after run acceptance). |
| `middleware_stage_input_invalid` | Kernel state carries no payload the stage's input can be built from. |
| `middleware_stage_payload_invalid` | A folded payload could not be canonicalized, parsed, or rebuilt. |
| `middleware_commit_required` | A committed-invocation guard was given a record that does not match. |
| `compaction_result_invalid` | A compaction result failed integrity validation. Today this is the normal outcome; see above. |
| `context_budget_exceeded` | A compacted projection still exceeds the hard model-input token budget. |
| `compaction_model_not_authorized` | A compaction child model effect is not related to an authorized parent. |

A middleware failure aborts the run that produced it and leaves the
runtime worker healthy.

## Source of truth

The enforced contract lives with the code, not on this page:

- [`crates/finstack-ai-runtime/src/exec/middleware_driver/`](../../crates/finstack-ai-runtime/src/exec/middleware_driver/)
  — the module contract: the fold invariant, replay safety, the derived
  effect id, ordering, and the full limitation list.
- [`crates/finstack-ai-runtime/src/exec/stage_settlement/`](../../crates/finstack-ai-runtime/src/exec/stage_settlement/)
  — where each stage's fold is applied.

See also [conformance](conformance.md) for the published middleware and
compaction suites, and [troubleshooting](troubleshooting.md) for
run-level error classes.

## License

[MIT](../../licenses/LICENSE-MIT) OR [Apache-2.0](../../licenses/LICENSE-APACHE).
[Contributing](../../CONTRIBUTING.md). [Security](../../SECURITY.md).
