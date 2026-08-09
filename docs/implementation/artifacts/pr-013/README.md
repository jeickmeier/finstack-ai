# PR-013 candidate evidence

PR-013 hardens the existing candidate-v1 kernel without adding runtime or
interaction-routing behavior.

Local evidence currently includes:

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

Hosted cross-platform CI and a manual long-fuzz workflow run are required before
acceptance A03, PR-013, Phase 1, or G1 can be marked passed. No gate decision is
claimed by this candidate evidence directory.
