# Isolated plugins

Optional WIT/Wasmtime isolation path (`finstack-ai-wit`, `finstack-ai-plugin-host`).

`finstack-ai-wit` currently ships experimental `@0.0.4` types, host, and
toolset worlds with in-process Rust bindings. `finstack-ai-plugin-host`
remains a placeholder until PR-051.

Trusted native in-process batteries (providers, toolsets, stores, observers) live under [`../extensions/`](../extensions/), not here. See [Technical Design §2](../docs/planning/03-finstack-ai-technical-design.md).
