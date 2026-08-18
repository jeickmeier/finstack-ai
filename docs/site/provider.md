# Providers

Model providers are separate crates. The default SDK graph does not construct
a client on import.

| Crate | Notes |
| --- | --- |
| `finstack-ai-provider-openai` | Official OpenAI Responses; HTTPS Bearer |
| `finstack-ai-provider-ollama` | Native `/api/chat`; keyless loopback |
| `finstack-ai-provider-anthropic` | Anthropic Messages leaf |
| `finstack-ai-test` (`ScriptedModel`) | Semantic scripted tests; no HTTP |

Python lazy extras (`finstack_ai.providers.*`) load on attribute access.
`Agent.openai()`, `Agent.anthropic()`, `Agent.ollama()`, and
`Agent.gateway()` are the same Rust-owned constructors on Python and
WASM. wasm-host methods exist; fail-closed is a Rust platform error
(`agent_run_unsupported_plan`), not a missing method. They accept the
same keyword-only T2 Python ports as `Agent.from_python`. `openai` maps
required keyword-only `api_key` to Bearer auth and always targets
official Responses. `ollama` stays keyless. The factories do not read
environment variables. `Agent.gateway()` stays as a thin dispatcher onto
the three dedicated crates. Supported `wire_protocol` values are
`openai_responses`, `anthropic_messages`, and `ollama_chat`.
`openai_chat` is a configuration error. There is no multi-protocol
gateway crate. `Agent.e2b_sandbox()` is the T4 sandbox constructor on
both bindings; see [toolsets](toolset.md).

`GatewayAgentSpec.wire_protocol` selects the dedicated leaf. Required
construction fields include `hard_input_bytes` and `max_output_tokens`.
Dedicated crate rustdoc on each `*Provider::try_new` is the same
constructor.

Never put secrets in `AgentSpec`, bundle defaults, resolution locks, logs,
or source files. Pass credentials only through redacted `Authentication`
wrappers. See [provider security](provider-security.md).

Native providers are [T1](security-trust-levels.md). They are not isolated.

## License

[MIT](../../licenses/LICENSE-MIT) OR [Apache-2.0](../../licenses/LICENSE-APACHE).
[Contributing](../../CONTRIBUTING.md). [Security](../../SECURITY.md).
