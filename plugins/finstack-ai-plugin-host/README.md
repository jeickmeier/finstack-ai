# finstack-ai-plugin-host

Optional isolated Wasmtime (T3) component host and host-owned compiled-component
cache. Constructing `PluginHost` is the opt-in; the default `finstack-ai` bundle
does not depend on this crate or on Wasmtime.

This path instantiates `toolset-plugin` and `context-plugin` guests compiled
against experimental `@0.0.4` or frozen `@1.0.0` WIT. Manifest `version`
selects the linker; a `@0.0.4` component against a `@1.0.0` manifest fails
closed. In-process adapters in `finstack-ai-wit`
inherit host authority and are not a sandbox. WASI is deny-by-default: a
permission name is not a linked capability. Fuel, memory, table, and instance
ceilings contain exhaustion. Signature policy is host configuration. This crate
reads one local lockfile through `PluginHost::load_enabled` and does not
claim that WASM alone is complete application-level safety. Discovery does
not walk a plugin directory and does not fetch from a registry.

`InstancePolicy::Serialized` keeps a successful instance between calls and
replenishes its fuel budget for each invocation. A guest-call error, trap,
timeout, cancellation, or dropped in-flight call discards that instance; the
next call reconstructs it. Guests using this policy must tolerate losing their
in-memory state after an interrupted or failed call. `Exclusive` continues to
instantiate a fresh store for every call.

Author a guest with [`finstack-ai-guest-sdk`](../finstack-ai-guest-sdk/README.md)
and the templates under `plugins/templates/`. Host proofs for the
published calculator, context-provider, and filesystem-sandbox
components live in this crate:

```text
cargo test -p finstack-ai-plugin-host --offline --locked -- calculator_matches
cargo test -p finstack-ai-plugin-host --offline --locked -- context_provider_matches
cargo test -p finstack-ai-plugin-host --offline --locked -- filesystem_sandbox
cargo test -p finstack-ai-plugin-host --offline --locked -- conformance_tests::
uv run --no-project python scripts/plugin_lock/lock.py
uv run --no-project python scripts/plugin_lock/lock.py --check
uv run --no-project python scripts/plugin_wasm/template_check.py
```

`load_enabled` on `plugins/reference/plugin.lock.json` loads calculator and
context-provider. Filesystem-sandbox stays `enabled: false` until the host
offers a filesystem grant and a preopen.

See [COMPATIBILITY.md](../finstack-ai-wit/COMPATIBILITY.md) and Technical Design
§27.6–27.7.
