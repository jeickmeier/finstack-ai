# Context plugin template

Copy this directory and keep the path dependency on
`finstack-ai-guest-sdk`. The default template requests `logging` only.

Trust class: [T3](../../../docs/site/security-trust-levels.md) when loaded
by the isolated host.

## Quick start

```text
cargo build --target wasm32-unknown-unknown --release
uv run --no-project python scripts/plugin_wasm/template_check.py
```
