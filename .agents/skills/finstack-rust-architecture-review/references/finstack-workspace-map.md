# Finstack Quant Workspace Architecture Map

Use this reference before broad Rust architecture reviews.

## Crate Roles

- `finstack-quant/core`: foundational dates, market data, math, money, identifiers, and shared domain primitives.
- `finstack-quant/valuations`: pricing, instruments, sensitivities, attribution, and valuation integration API.
- `finstack-quant/scenarios`: scenario specs, engines, market-data transformations, and stress workflows.
- `finstack-quant/portfolio`: portfolio aggregation, performance, and attribution-facing workflows.
- `finstack-quant/margin`: SIMM, margin calculators, XVA-related types, and collateral terms.
- `finstack-quant/monte_carlo`: stochastic engines and payoff evaluation.
- `finstack-quant/statements*`: financial statement modeling and analytics.
- `finstack-quant-py`: PyO3 bindings and Python package/stub surface.
- `finstack-quant-wasm`: wasm-bindgen bindings and JS facade.

## Dependency Direction

Domain logic should flow from lower-level crates toward higher-level workflows. Bindings depend on Rust crates; Rust crates should not depend on bindings.

Watch for:

- valuation logic leaking into `finstack-quant-py` or `finstack-quant-wasm`,
- scenario/portfolio crates reimplementing valuation math instead of calling `valuations`,
- public APIs exposing internal builder stages or registry plumbing,
- serde or metric-key names changing without compatibility review,
- parallel and serial paths diverging.

## Evidence To Collect

- `Cargo.toml` workspace members and dependencies.
- `src/lib.rs` public exports and prelude contents.
- Error types and `Result` aliases.
- Public builders, constructors, traits, and serde types.
- Tests, examples, benches, bindings, parity contract, and docs for the reviewed surface.
