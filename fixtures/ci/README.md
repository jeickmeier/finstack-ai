# CI fixtures

| Path | Purpose | Evidence eligible |
| --- | --- | --- |
| `release-smoke/` | Private facade-dependent release binary for PR-003-A03 | Yes, when built by `mise run release-smoke` |
| `model-port-leaf/` | Default-feature-free native/browser-WASM leaf implementation of the public PR-015 `Model` ABI | Yes, when built by `mise run test-model` |

These fixtures are classified as `fixture` by architecture policy and must not
become published packages.
