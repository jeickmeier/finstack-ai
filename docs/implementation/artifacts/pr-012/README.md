# PR-012 evidence

## Status

PR-012 is implemented, locally validated, and integrated into local `main` at immutable commit `dc58a11fbc871e70326d04fd9840297b5179023f` (fast-forward of `codex/pr-012-structured-output`; no GitHub issue or actual PR). Acceptance A01-A05 passed against that revision.

## Scope

The candidate adds validator-independent structured output, framework-owned final-output control syntax, deterministic output/tool competition, bounded validation feedback, validation retry/exhaustion, complete pre-run capability-plan activation, four zero-public-event records, and strict conditional `kernel-state` v4.

It does not add a schema validator, Pydantic, runtime catalog, capability provider, model-driven capability activation, binding convenience layer, seventh port, or internal-tool effect execution.

## Evidence

- [`candidate-validation.txt`](candidate-validation.txt) records the focused and aggregate local validation at the immutable integration commit.
- [`security-review.txt`](security-review.txt) records the PR-012 threat-model disposition.
- Five structured-output summaries live under `fixtures/compatibility/golden-trace/v1/structured-output/`.
- The public Rust API corpus contains 83 fixtures through PR-012, including a negative zero-schema-version boundary.
- [`SHA256SUMS`](SHA256SUMS) records the durable artifact digests.

The complete aggregate gate passed after the implementation was committed and integrated. The existing informational duplicate-`syn` warning from `cargo-deny` remained non-blocking; the supply-chain task passed.
