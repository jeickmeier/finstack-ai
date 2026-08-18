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
`Agent.openai()`, `Agent.anthropic()`, and `Agent.ollama()` are
explicit T1 constructors. They accept the same keyword-only T2 Python ports
as `Agent.from_python`. `openai` maps required keyword-only `api_key` to
Bearer auth and always targets official Responses. `ollama` stays keyless.
The factories do not read environment variables. Opt-in
`finstack-ai-provider-gateway` covers config-selected OpenAI-compatible,
Anthropic Messages, and Ollama chat endpoints without adding a client to
the default SDK graph.

Never put secrets in `AgentSpec`, bundle defaults, resolution locks, logs,
or source files. Pass credentials only through redacted `Authentication`
wrappers. See [provider security](provider-security.md).

Native providers are [T1](security-trust-levels.md). They are not isolated.

## License

[MIT](../../licenses/LICENSE-MIT) OR [Apache-2.0](../../licenses/LICENSE-APACHE).
[Contributing](../../CONTRIBUTING.md). [Security](../../SECURITY.md).
