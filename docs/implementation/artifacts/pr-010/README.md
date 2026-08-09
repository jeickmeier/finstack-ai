# PR-010 local integration evidence

## Status

PR-010 is integrated on local `main` at `ff2e6e7b80e34061dae4dcc5ceb4b259a34a89b5`. The integration was a fast-forward of `codex/pr-010-tool-reducer`; no GitHub issue or actual pull request exists.

## Scope

The integrated change freezes and implements tool-call and tool-batch reducer semantics in the deterministic kernel: validated plans, execution groups, direct and external settlement, source-ordered result finalization, synthetic closures, fail-run draining, records/events, fingerprints, capacity preflight, conditional `kernel-state` v2, compatibility fixtures, and golden traces.

PR-010 does not implement a toolset, schema compiler, runtime executor, middleware invocation, limits, retries, cancellation state, store loop, bindings, or a new dependency. PR-011 retains cancellation inputs and `EffectCancelled`.

## Evidence

- [`integrated-validation.txt`](integrated-validation.txt) records the immutable candidate and post-integration validation, including the retained transient network failure and the clean final aggregate.
- [`security-review.txt`](security-review.txt) records the Threat Model section 18 / TM-02 review and residual later-PR obligations.
- [`environment.txt`](environment.txt) records the integration environment and exact commit.

## Acceptance disposition

- A01 passed through reverse-order parallel-completion tests that retain source-ordered messages/events.
- A02 passed through committed-prefix replay, fail-run draining, deferred external completion, and restorable failure histories; cancellation closure remains PR-011 scope.
- A03 passed through equal duplicate and conflicting direct/external completion tests.
- A04 passed through eight deterministic golden traces, including mixed groups and partial failure.

PR-010 closes no phase exit or program gate. Phase 1 remains in progress and G1 remains not ready.
