# PR-011 local integration evidence

## Status

PR-011 is integrated on local `main` at `01380ead5c5ca7b9e7d28d681d84719c9bf0279e`. The integration was a fast-forward of `codex/pr-011-kernel-termination`; no GitHub issue or actual pull request exists.

## Scope

The integrated change implements deterministic kernel limits, checked integer-micro-unit cost accounting, deadlines, whole-run retry state, explicit cancellation, incremental reconciliation, terminal-race precedence, lineage propagation decisions, deferred-effect closure, source-ordered tool cancellation, and conditional strict `kernel-state` v3 while preserving v1/v2 hashes.

It does not add real clocks, sleeping, Tokio cancellation tokens, runtime dispatch, a store commit loop, low-level effect re-execution, actual child fan-out, cross-run budget services, interaction routing, or bindings.

## Evidence

- [`candidate-validation.txt`](candidate-validation.txt) records focused and aggregate validation at the immutable integrated commit.
- [`security-review.txt`](security-review.txt) records the TM-02, TM-14, TM-15, and SEC-INV disposition.
- [`environment.txt`](environment.txt) records the local integration environment.

## Acceptance disposition

- A01 passed through below/exact/above tests for every PR-011 limit dimension.
- A02 passed through exact integer-cost fixtures, unknown-usage policies, registered extension counters, and checked overflow failures.
- A03 passed through every reachable nonterminal phase, idempotent replay, deferred effects, tool batches, and terminal immutability.
- A04 passed through durable retry replay, equal timer firing, and budget exhaustion tests.
- A05 passed through completion/cancellation/limit race permutations and explicit uncertainty suspension.
- A06 passed through cascade and preauthorized-detach lineage decisions plus deterministic deferred/model/tool cancellation closure.

