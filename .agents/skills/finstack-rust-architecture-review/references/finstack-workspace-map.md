# Finstack Workspace Architecture Map

Use this reference before broad Rust architecture reviews.

## Crate Roles

- `crates/finstack-ai-kernel`: deterministic semantic state, records, events, effects, and the pure decide/apply model. Synchronous and I/O-free.
- `crates/finstack-ai-runtime`: six primary ports and effect execution.
- `crates/finstack-ai`: SDK composition, registration, resolution, and ergonomic public APIs.
- `crates/finstack-ai-protocol`: codecs, journal frames, and outward-facing protocol types.
- `crates/finstack-ai-server`: remote/server adapters.
- `crates/finstack-ai-test`: shared test helpers.
- `bindings/finstack-ai-python`: PyO3 bindings and Python package/stub surface.
- `bindings/finstack-ai-wasm`: wasm-bindgen bindings and JS facade.
- `extensions/`: trusted native stores, providers, tools, observers, and middleware.
- `plugins/`: isolated WIT/Wasmtime hosts, guest SDK, and reference components.

## Dependency Direction

Domain logic should flow from kernel toward runtime, SDK, then bindings and leaves. Bindings depend on Rust crates; Rust crates should not depend on bindings.

Watch for:

- semantic or lifecycle logic leaking into `bindings/finstack-ai-python` or `bindings/finstack-ai-wasm`,
- kernel depending on runtime, protocol, or I/O crates,
- extensions or plugins reimplementing kernel decisions instead of calling contracts,
- public APIs exposing internal builder stages or registry plumbing,
- serde, error-code, or WIT names changing without compatibility review,
- parallel and serial paths diverging on durable history.

## Evidence To Collect

- `Cargo.toml` workspace members and dependencies.
- `src/lib.rs` public exports and prelude contents.
- Error types and `Result` aliases.
- Public builders, constructors, traits, and serde types.
- Tests, examples, benches, bindings, public-item inventory, and docs for the reviewed surface.
