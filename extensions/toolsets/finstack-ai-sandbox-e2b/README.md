# finstack-ai-sandbox-e2b

T4 remote-principal Toolset that starts an E2B sandbox and runs one command.
Construction requires an explicit API key and never reads environment
variables. Non-loopback endpoints must be HTTPS. This leaf is not Landlock
and is not isolated. Shell stays T1.

```rust
use finstack_ai_sandbox_e2b::{E2bSandboxConfig, E2bSandboxToolset};

let tools = E2bSandboxToolset::try_new(E2bSandboxConfig {
    api_key: "e2b-secret".into(),
    endpoint: "https://api.e2b.dev".into(),
    template: None,
})
.expect("e2b");
```
