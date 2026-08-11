# CI fixtures

| Path | Purpose | Evidence eligible |
| --- | --- | --- |
| `release-smoke/` | Private facade-dependent release binary for PR-003-A03 | Yes, when built by `mise run release-smoke` |
| `model-port-leaf/` | Default-feature-free native/browser-WASM leaf implementation of the public PR-015 `Model` ABI | Yes, when built by `mise run test-model` |
| `toolset-port-leaf/` | Default-feature-free native/browser-WASM leaf implementation of the public PR-016 `Toolset` ABI | Yes, when built by `mise run test-tool` |
| `extension-port-leaf/` | Default-feature-free native/browser-WASM leaf implementations of the public PR-018 `ContextProvider`, `Middleware`, and `Observer` ABIs | Yes, when built by `mise run test-extensions` |

These fixtures are classified as `fixture` by architecture policy and must not
become published packages.
