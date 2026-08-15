# finstack-ai-plugin-host

Optional isolated Wasmtime (T3) component host and host-owned compiled-component
cache. Constructing `PluginHost` is the opt-in; the default `finstack-ai` bundle
does not depend on this crate or on Wasmtime.

This path instantiates `toolset-plugin` and `context-plugin` guests compiled
against experimental `@0.0.4` WIT. In-process adapters in `finstack-ai-wit`
inherit host authority and are not a sandbox. WASI is deny-by-default: a
permission name is not a linked capability. Fuel, memory, table, and instance
ceilings contain exhaustion. Signature policy is host configuration. This crate
reads one local lockfile through `PluginHost::load_enabled` and does not
claim that WASM alone is complete application-level safety. Discovery does
not walk a plugin directory and does not fetch from a registry.

Author a guest with [`finstack-ai-guest-sdk`](../finstack-ai-guest-sdk/README.md)
and the templates under `plugins/templates/`. Host proofs for the
published calculator, context-provider, and filesystem-sandbox
components live in this crate:

```text
cargo test -p finstack-ai-plugin-host --offline --locked -- calculator_matches
cargo test -p finstack-ai-plugin-host --offline --locked -- context_provider_matches
cargo test -p finstack-ai-plugin-host --offline --locked -- filesystem_sandbox
cargo test -p finstack-ai-plugin-host --offline --locked -- conformance_tests::
mise run gen-plugin-lock
mise run check-plugin-lock
mise run check-plugin-template
```

`load_enabled` on `plugins/reference/plugin.lock.json` loads calculator and
context-provider. Filesystem-sandbox stays `enabled: false` until the host
offers a filesystem grant and a preopen.

See [COMPATIBILITY.md](../finstack-ai-wit/COMPATIBILITY.md) and Technical Design
§27.6–27.7.
