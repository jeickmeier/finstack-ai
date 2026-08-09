# PR-012 candidate evidence

## Status

PR-012 is implemented and locally validated on `codex/pr-012-structured-output`. This directory records candidate evidence only: the working tree is not yet committed, so the evidence is not immutable and PR-012 acceptance remains pending.

## Scope

The candidate adds validator-independent structured output, framework-owned final-output control syntax, deterministic output/tool competition, bounded validation feedback, validation retry/exhaustion, complete pre-run capability-plan activation, four zero-public-event records, and strict conditional `kernel-state` v4.

It does not add a schema validator, Pydantic, runtime catalog, capability provider, model-driven capability activation, binding convenience layer, seventh port, or internal-tool effect execution.

## Evidence

- [`candidate-validation.txt`](candidate-validation.txt) records the focused and aggregate local validation.
- [`security-review.txt`](security-review.txt) records the PR-012 threat-model disposition.
- Five structured-output summaries live under `fixtures/compatibility/golden-trace/v1/structured-output/`.
- The public Rust API corpus contains 83 fixtures through PR-012, including a negative zero-schema-version boundary.

## Handoff condition

Bind these results to an immutable revision, rerun the required validation there, then update the evidence register and delivery ledger before marking any PR-012 acceptance criterion passed or the logical PR done.
