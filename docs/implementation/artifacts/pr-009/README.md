# PR-009 local candidate artifacts

## Status

These files describe a local, dirty, uncommitted PR-009 candidate on `main` at base `fa4735801781011ae340db9d850ecbe0aded128b`. They are reproducible working-tree artifacts only: they are not immutable evidence, have no evidence IDs or reviewer approval, and do not establish hosted checks, acceptance passage, merge, PR completion, Phase 1 completion, or G1 passage.

## Scope

The candidate covers the uncommitted PR-009 planning amendment, model-only kernel reducer and records/events, focused tests, real conformance adapter, compatibility fixtures, schemas, and synthetic benchmark. [`environment.txt`](environment.txt) records the exact local environment and dirty-worktree status. [`security-review.txt`](security-review.txt) records the completed local candidate review and its later-runtime obligations.

## Initial candidate validation

The original logs in this directory were captured before whole-change review remediation. They remain local historical artifacts and do not substantiate the remediated tree by themselves.

- `mise run test-kernel` — pre-remediation log: [`test-kernel.log`](test-kernel.log).
- `mise run conformance` — pre-remediation log: [`conformance.log`](conformance.log).
- `mise run check-kernel` — pass, exit 0. Log: [`check-kernel.log`](check-kernel.log).
- `mise run check-wasm` — pass, exit 0 for kernel and facade/WASM target checks. Log: [`check-wasm.log`](check-wasm.log).
- `mise run schema-governance` — pass, exit 0; `schema-governance: ok`. Log: [`schema-governance.log`](schema-governance.log).
- `mise run benchmark-smoke` — pre-remediation log: [`benchmark-smoke.log`](benchmark-smoke.log).
- `mise run architecture` — pass, exit 0; 21 tests passed and no architecture findings were reported. Log: [`architecture.log`](architecture.log).
- `mise run ci` — pre-remediation log: [`ci.log`](ci.log).
- `git diff --check` — pass, exit 0 with no output. Log: [`git-diff-check.log`](git-diff-check.log).

## Post-review remediation validation

[`remediation-validation.txt`](remediation-validation.txt) records the fresh final-tree command summary after correlation, event sequencing, strict decoding, capacity precedence, request-shape, path-containment, state-projection, and child-record replay fixes. All nine required commands passed: `test-kernel` (84 kernel unit tests, 38 reducer tests, 6 doctests, 4 runtime tests, and 4 public-Rust-API tests), `conformance` (29 tests), `check-kernel`, `check-wasm`, `schema-governance`, `benchmark-smoke`, `architecture`, `ci`, and `git diff --check`.

After simplification removed the duplicate semantic-apply pass, the direct reducer workload measured `[179.95 µs 180.47 µs 182.54 µs]` and the end-to-end conformance runner measured `[194.62 µs 195.74 µs 200.21 µs]`. Quick-mode measurements are smoke evidence, not a regression decision. These remain dirty-worktree candidate results, not immutable evidence.

Current checked deterministic state hashes are:

- direct model completion, split-chunk completion, and the completed public-Rust-API state: `58a34b469371d122d76c71740cf6aa2c5b246788239e965e09e750a6386a493c`;
- deferred model completion: `d4bfdb61087556bf76e4ebe726ed58f72e703dfa8131176736bbf55777366c2b`;
- `before_finalize` continuation: `39b343b7925f315d66a26be1b317c20fdd30d759d379e4244b0889ce1b007a49`;
- empty external completion: `2728b1d17148683f4363ee3f77281459f14b1542dd0d6805898456a4f24beff2`; and
- default kernel state: `2bb5c2fabe0b6669de360dfa118eabac52821dd3378b09d762e8fc08ab32f0c8`.

After the candidate artifact and report refresh, the required post-edit checks were run again. Aggregate result: 2 passed, 0 failed.

- `mise run schema-governance` — pass, exit 0; `schema-governance: ok`. Final log: [`schema-governance-final.log`](schema-governance-final.log).
- `git diff --check` — pass, exit 0 with no output. Final log: [`git-diff-check-final.log`](git-diff-check-final.log).

## Security review and limits

The initial local security review in [`security-review.txt`](security-review.txt) predates whole-change review findings and is explicitly superseded as a current-tree claim. The remediation strengthens event sensitivity/correlation, strict/bounded failures, fixture path containment, and fail-closed settlement validation, but a fresh dedicated whole-change security review remains required before evidence binding. None of this is immutable security evidence or reviewer approval. Authenticated tenant-scoped external completion ingress, store enforcement, and observer sensitivity handling remain later-runtime obligations.

PR-009 remains uncommitted with no GitHub issue or actual pull request. Acceptance is 0/5 and PR-009 is only `In review`; there are no hosted, nightly, approval, merge, phase-exit, or gate artifacts here.

`SHA256SUMS` covers every artifact file in this directory except the checksum manifest itself.
