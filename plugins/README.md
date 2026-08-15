# Isolated plugins

Optional WIT/Wasmtime isolation path (`finstack-ai-wit`, `finstack-ai-plugin-host`).

`finstack-ai-wit` ships experimental `@0.0.4` types, host, toolset, and
context worlds with in-process Rust bindings, host-side manifest
validation, and lifecycle adapters. Those in-process guests inherit host
authority and are not a sandbox.

`finstack-ai-plugin-host` is the optional isolated Wasmtime (T3) leaf:
engine, host-owned compiled-component cache, deny-by-default WASI,
resource limits, signature policy, and `WasmToolsetAdapter` /
`WasmContextAdapter`. The default SDK bundle does not depend on it.

Trusted native in-process batteries (providers, toolsets, stores, observers) live under [`../extensions/`](../extensions/), not here. See [Technical Design §2](../docs/planning/03-finstack-ai-technical-design.md).
