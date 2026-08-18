# Providers

Model providers are separate crates. The default SDK graph does not construct
a client on import.

| Crate | Notes |
| --- | --- |
| `finstack-ai-provider-openai` | Official OpenAI Responses; HTTPS Bearer |
| `finstack-ai-provider-ollama` | Native `/api/chat`; keyless loopback |
| `finstack-ai-provider-anthropic` | Anthropic Messages leaf |
| `finstack-ai-provider-gateway` | Config-selected wire protocol; explicit profiles |
| `finstack-ai-test` (`ScriptedModel`) | Semantic scripted tests; no HTTP |

Python lazy extras (`finstack_ai.providers.*`) load on attribute access.
`Agent.openai()`, `Agent.anthropic()`, `Agent.ollama()`, and
`Agent.gateway()` are the same Rust-owned constructors on Python and
WASM. wasm-host methods exist; fail-closed is a Rust platform error
(`agent_run_unsupported_plan`), not a missing method. They accept the
same keyword-only T2 Python ports as `Agent.from_python`. `openai` maps
required keyword-only `api_key` to Bearer auth and always targets
official Responses. `ollama` stays keyless. The factories do not read
environment variables. `Agent.gateway()` reuses the opt-in
`finstack-ai-provider-gateway` leaf for config-selected OpenAI-compatible,
Anthropic Messages, and Ollama chat endpoints without adding a client to
the default SDK graph. The leaf can still be constructed directly.
`Agent.e2b_sandbox()` is the T4 sandbox constructor on both bindings;
see [toolsets](toolset.md).

```rust
use finstack_ai_provider_gateway::{
    Authentication, CredentialReference, CredentialStore, GatewayCapabilityFlags,
    GatewayModelConfig, GatewayModelSpec, GatewayProvider, GatewayRouteConfig,
    WireProtocol,
};
use finstack_ai_runtime::{
    InputCapabilities, StructuredOutputCapability, TokenEstimatorRef,
    TokenEstimatorSource,
};
use std::sync::Arc;

let route = GatewayRouteConfig::try_new(
    WireProtocol::OpenaiChat,
    "http://127.0.0.1:9/v1/chat/completions",
    CredentialReference::try_new("local").expect("reference"),
)
.expect("route");
let spec = GatewayModelSpec::try_from_config(GatewayModelConfig {
    name: Some("fixture-model".to_owned()),
    hard_input_bytes: Some(1_000_000),
    context_window_tokens: Some(8_192),
    max_output_tokens: Some(1_024),
    reserved_output_tokens: Some(1_024),
    provider_overhead_tokens: Some(64),
    estimator: Some(TokenEstimatorRef {
        id: Arc::from("gateway.utf8-byte-upper-bound"),
        version: Arc::from("1"),
        source: TokenEstimatorSource::ConservativeUpperBound,
    }),
    capabilities: Some(GatewayCapabilityFlags {
        input: InputCapabilities {
            text: true,
            json: true,
            images: false,
            audio: false,
            files: false,
        },
        native_tool_calls: true,
        parallel_tool_calls: false,
        structured_output: StructuredOutputCapability::Unsupported,
        reasoning: false,
        prompt_cache: false,
        resumable_stream: false,
        idempotent_requests: false,
    }),
})
.expect("spec");
let mut store = CredentialStore::empty();
store
    .insert("local", Authentication::None)
    .expect("store");
let _provider = GatewayProvider::try_new(route, vec![spec], store).expect("provider");
```

Required construction fields include `hard_input_bytes` and
`max_output_tokens`. The crate rustdoc on `GatewayProvider::try_new` is
the same constructor.

Never put secrets in `AgentSpec`, bundle defaults, resolution locks, logs,
or source files. Pass credentials only through redacted `Authentication`
wrappers. See [provider security](provider-security.md).

Native providers are [T1](security-trust-levels.md). They are not isolated.

## License

[MIT](../../licenses/LICENSE-MIT) OR [Apache-2.0](../../licenses/LICENSE-APACHE).
[Contributing](../../CONTRIBUTING.md). [Security](../../SECURITY.md).
