# finstack-ai-wit

Dual-major `@0.0.4` and `@1.0.0` WIT packages and in-process Rust
bindings for the toolset and context-provider worlds.

The crate compiles host and guest traits from the checked-in WIT roots
under `wit/v0.0.4/` and `wit/v1.0.0/`. Regenerated bindings live in
`src/generated.rs` and are owned by `mise run gen-wit` /
`mise run check-wit`. Do not hand-edit generated files.

This crate does **not** instantiate Wasmtime. Isolated Wasmtime (T3)
instantiation lives in `finstack-ai-plugin-host`. In-process adapters
here inherit host authority and are not a sandbox. `@0.x` guests keep
loading. `@1.0.0` is the frozen exact-world line.

Guest authors should use [`finstack-ai-guest-sdk`](../finstack-ai-guest-sdk/README.md)
rather than depending on this crate.

See [COMPATIBILITY.md](COMPATIBILITY.md) for exact-world rules.
