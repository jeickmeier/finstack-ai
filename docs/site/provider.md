# Providers

Model providers are separate crates. The default SDK graph does not construct
a client on import.

| Crate | Notes |
| --- | --- |
| `finstack-ai-provider-openai-compatible` | HTTPS or explicit loopback; Ollama/local constructor |
| `finstack-ai-provider-anthropic` | Anthropic Messages leaf |
| `finstack-ai-provider-test` | Scripted tests |

Python lazy extras (`finstack_ai.providers.*`) load on attribute access.
`Agent.openai_compatible()`, `Agent.anthropic()`, and `Agent.ollama()` are
explicit constructors.

Never put secrets in `AgentSpec`, bundle defaults, resolution locks, logs,
or source files. Pass credentials only through redacted `Authentication`
wrappers. See [provider security](provider-security.md).

Native providers are [T1](security-trust-levels.md). They are not isolated.

## License and governance

[MIT](../../licenses/LICENSE-MIT) OR [Apache-2.0](../../licenses/LICENSE-APACHE).
[DCO](../../CONTRIBUTING.md). [Maintainers](../../GOVERNANCE.md).
[ADRs](../implementation/adr-register.md). [RFCs](../rfcs/README.md).
