# finstack-ai-plugin-host

Optional isolated Wasmtime (T3) component host and host-owned compiled-component
cache. Constructing `PluginHost` is the opt-in; the default `finstack-ai` bundle
does not depend on this crate or on Wasmtime.

This path instantiates `toolset-plugin` and `context-plugin` guests compiled
against experimental `@0.0.4` WIT. In-process adapters in `finstack-ai-wit`
inherit host authority and are not a sandbox. This crate does not link WASI,
verify signatures, or read a plugin lockfile.

See [COMPATIBILITY.md](../finstack-ai-wit/COMPATIBILITY.md) and Technical Design
§27.6–27.7.
