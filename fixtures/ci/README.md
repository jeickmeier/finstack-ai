# CI fixtures

| Path | Purpose |
| --- | --- |
| `model-port-leaf/` | Default-feature-free native/browser-WASM leaf implementation of the public `Model` ABI |
| `toolset-port-leaf/` | Default-feature-free native/browser-WASM leaf implementation of the public `Toolset` ABI |
| `extension-port-leaf/` | Default-feature-free native/browser-WASM leaf implementations of the public `ContextProvider`, `Middleware`, and `Observer` ABIs |

They are built and tested by `mise run test`.

These fixtures are classified as `fixture` by architecture policy and must not
become published packages.
