# Isolated plugins

Optional WIT/Wasmtime isolation path (`finstack-ai-wit`,
`finstack-ai-plugin-host`, `finstack-ai-guest-sdk`).

Permanent worlds are `finstack:ai-*@1.0.0`. Experimental `@0.0.4` worlds
stay loadable and labeled. Guest authors copy a template, depend on
`finstack-ai-guest-sdk` only, and retarget `@0.0.4` guests with
[`finstack-ai-guest-sdk/MIGRATION.md`](finstack-ai-guest-sdk/MIGRATION.md).

`finstack-ai-wit` compiles host and guest traits from the checked-in WIT
roots. In-process WIT guests inherit host authority and are not a sandbox.

`finstack-ai-plugin-host` is the optional isolated Wasmtime (T3) leaf:
engine, host-owned compiled-component cache, deny-by-default WASI,
resource limits, signature policy, and `WasmToolsetAdapter` /
`WasmContextAdapter`. The default SDK bundle does not depend on it.

Copy `templates/toolset-plugin/` or `templates/context-plugin/`. Encode
with `tools/plugin_wasm/encoder`. Published reference components live
under `reference/` (calculator, context provider, read-only filesystem
sandbox). The sandbox is a T3 fixture over a granted preopen; it is not
`finstack-ai-tools-filesystem`. Runtime discovery reads
`reference/plugin.lock.json` only.

```text
rust-version: 1.97.1
target: wasm32-unknown-unknown
wit-bindgen: 0.57.1
WIT packages: finstack:ai-*@1.0.0 (frozen); finstack:ai-*@0.0.4 (experimental, loadable)
encode: tools/plugin_wasm/encoder
```

```text
mise run gen-guest-sdk
mise run check-guest-sdk
mise run gen-plugin-wasm
mise run check-plugin-wasm
mise run gen-plugin-lock
mise run check-plugin-lock
mise run check-plugin-template
cargo test -p finstack-ai-guest-sdk --offline --locked
cargo test -p finstack-ai-plugin-host --offline --locked -- reference_
```

Trusted native in-process batteries (providers, toolsets, stores, observers) live under [`../extensions/`](../extensions/), not here. See [Technical Design §2](../docs/planning/03-finstack-ai-technical-design.md).

Trust class: isolated Wasmtime guests are [T3](../docs/site/security-trust-levels.md).
In-process WIT guests inherit host authority and are not a sandbox.

## Quick start

```text
mise run check-plugin-template
```

## License and governance

[MIT](../licenses/LICENSE-MIT) OR [Apache-2.0](../licenses/LICENSE-APACHE).
[DCO](../CONTRIBUTING.md). [Maintainers](../GOVERNANCE.md).
[ADRs](../docs/implementation/adr-register.md).
[RFCs](../docs/rfcs/README.md).
