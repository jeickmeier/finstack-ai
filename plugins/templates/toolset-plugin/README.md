# Toolset plugin template

Copy this directory, rename the crate, and keep the path dependency on
`finstack-ai-guest-sdk`. Request only the permissions you need. The
default template requests `logging` only.

```text
cargo build --target wasm32-unknown-unknown --release
# encode with tools/plugin_wasm/encoder
mise run check-plugin-template
```

The experimental host default is one Wasmtime instance per store.
This template sets `resource_limits.max_instances` / `max_tables` so
the encoded component can instantiate. Copy those fields when you
start from the template.

Pinned toolchain: rustc 1.97.1, `wasm32-unknown-unknown`, wit-bindgen
0.57.1, WIT `@0.0.4`. See `plugins/finstack-ai-guest-sdk/README.md`.

Trust class: [T3](../../../docs/site/security-trust-levels.md) when loaded
by the isolated host. This template stays on experimental `@0.0.4`.
Permanent `@1.0.0` worlds exist; retarget with
[guest MIGRATION.md](../../finstack-ai-guest-sdk/MIGRATION.md).

## Quick start

```text
mise run check-plugin-template
```
