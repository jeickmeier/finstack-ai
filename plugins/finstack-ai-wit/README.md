# finstack-ai-wit

Experimental `@0.0.4` WIT packages and in-process Rust bindings for the
initial toolset world.

The crate compiles host and guest traits from the checked-in WIT roots
under `wit/v0.0.4/`. Regenerated bindings live in `src/generated.rs` and
are owned by `mise run gen-wit` / `mise run check-wit`. Do not hand-edit
generated files.

This crate does **not** instantiate Wasmtime. The plugin host lands in
PR-051. The context-provider world lands in PR-050. `@1.0.0` generation
is blocked until the framework 1.0 gate.

See [COMPATIBILITY.md](COMPATIBILITY.md) for exact-world rules.
