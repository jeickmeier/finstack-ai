# PR-013 candidate evidence

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

The command/result inventory is in
[`candidate-validation.txt`](candidate-validation.txt), and the TM-10/TM-11/
TM-14/TM-15/TM-16 disposition is in
[`security-review.txt`](security-review.txt).

Hosted cross-platform CI and a manual long-fuzz workflow run are required before
acceptance A03, PR-013, Phase 1, or G1 can be marked passed. No gate decision is
claimed by this candidate evidence directory.
