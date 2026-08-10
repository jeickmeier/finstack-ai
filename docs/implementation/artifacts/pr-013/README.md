# PR-013 evidence

PR-013 hardens the existing candidate-v1 kernel without adding runtime or
interaction-routing behavior.

Immutable local evidence is bound to implementation commit
`aba26764a448f6b2691bfac62a0f36f865f61ed6` and includes:

- fixed-seed property/model tests (`256` cases, generated paths capped at `64`
  transitions);
- strict candidate-v1 malformed and corrupt-replay fixtures;
- four fixed-seed `cargo-fuzz` smoke campaigns (`256` runs per target);
- native tests plus compile-only kernel test targets for
  `wasm32-unknown-unknown`;
- compiler-exhaustive semantic-reference drift checks;
- architecture, schema governance, nightly, and separate production/fuzz
  supply-chain checks;
- the manually reviewed [`golden-trace-SHA256SUMS`](golden-trace-SHA256SUMS)
  catalog, unchanged from local `main` at `9c04c58`.

The local command/result inventory is in
[`candidate-validation.txt`](candidate-validation.txt), and the TM-10/TM-11/
TM-14/TM-15/TM-16 disposition is in
[`security-review.txt`](security-review.txt).

Hosted cross-platform CI, the manually dispatched long-fuzz campaign, retained
artifact identity, and merge are recorded in
[`hosted-validation.txt`](hosted-validation.txt). Pull request
[#6](https://github.com/jeickmeier/finstack-ai/pull/6) merged as
`fa6222f20e4a4616f600e867be94afe12967dcb9` after all required checks passed.

The separate named G1 approval and all four Phase 1 exit dispositions are in
[`g1-decision.txt`](g1-decision.txt). PR-013 is accepted 4/4, Phase 1 is
`Done`, and G1 is `Passed`. ADR-025 through ADR-028 remain partial for their
explicit later lifecycle work.

[`SHA256SUMS`](SHA256SUMS) binds the local candidate, golden catalog, security
review, hosted validation, gate decision, and semantic-reference artifacts.
