# finstack-ai-guest-sdk

Author-facing Rust helpers for experimental `@0.0.4` toolset and context
guests. This crate does not depend on `finstack-ai-plugin-host`,
`finstack-ai-wit`, the kernel, or Wasmtime.

## Pinned toolchain

```text
rust-version: 1.97.1
target: wasm32-unknown-unknown
wit-bindgen: 0.57.1
WIT packages: finstack:ai-*@0.0.4 (default); @1.0.0 retarget in MIGRATION.md
encode: scripts/plugin_wasm/encoder
```

Regenerate vendored WIT with
`uv run --no-project python scripts/plugin_wasm/sync_guest_wit.py`.
`uv run --no-project python scripts/plugin_wasm/sync_guest_wit.py --check`
fails on drift. Runtime discovery is lockfile-only
(`PluginHost::load_enabled`); guests are not searched.

## Author a guest

Copy `plugins/templates/toolset-plugin/` (or `context-plugin/`), depend
on this crate only, and invoke:

```rust
finstack_ai_guest_sdk::toolset_plugin!();
```

Then implement the generated `Guest` trait. After the macro, call
`finstack::ai_host::logging::log` for host logging. Use
`reject_log_message` first. Do not import `wasi:*` unless the host
grants the matching capability and concrete resource.

## Guest traps

A panic, arithmetic overflow (debug builds), allocation abort, or capacity
overflow inside guest code traps the component. The isolated host contains
the trap — the failing call returns a stable `plugin_trap` error and the
host process survives — but the plugin instance is dead and every later call
fails until the host reinstantiates. Validate untrusted input first
(`parse_args`, `reject_log_message`) instead of unwrapping, and avoid
panicking paths such as out-of-bounds slicing.

## Local host test commands

```text
uv run --no-project python scripts/plugin_wasm/sync_guest_wit.py
uv run --no-project python scripts/plugin_wasm/sync_guest_wit.py --check
uv run --no-project python scripts/plugin_wasm/generate.py
uv run --no-project python scripts/plugin_wasm/generate.py --check
uv run --no-project python scripts/plugin_wasm/template_check.py
cargo test -p finstack-ai-guest-sdk --offline --locked
cargo test -p finstack-ai-plugin-host --offline --locked -- reference_
```

See [MIGRATION.md](MIGRATION.md) for exact-world pinning and the
`@0.0.4` → `@1.0.0` retarget. Compatibility policy lives in
[`../finstack-ai-wit/COMPATIBILITY.md`](../finstack-ai-wit/COMPATIBILITY.md)
and ADR-035.
