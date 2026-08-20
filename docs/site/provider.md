# Providers

Model providers are separate crates. The default SDK graph does not construct
a client on import.

| Crate | Notes |
| --- | --- |
| `finstack-ai-provider-openai` | Official OpenAI Responses; HTTPS Bearer |
| `finstack-ai-provider-ollama` | Native `/api/chat`; keyless loopback |
| `finstack-ai-provider-anthropic` | Anthropic Messages leaf |
| `finstack-ai-provider-openrouter` | OpenRouter Responses (`/api/v1/responses`); HTTPS Bearer; optional attribution headers; model-catalog fetch |
| `finstack-ai-test` (`ScriptedModel`) | Semantic scripted tests; no HTTP |

Python lazy extras (`finstack_ai.providers.*`) load on attribute access.
`Agent.openai()`, `Agent.anthropic()`, `Agent.ollama()`,
`Agent.openrouter()`, and `Agent.gateway()` are the same Rust-owned
constructors on Python and WASM. wasm-host methods exist; fail-closed is
a Rust platform error (`agent_run_unsupported_plan`), not a missing
method. They accept the same keyword-only T2 Python ports as
`Agent.from_python`. `openai` maps required keyword-only `api_key` to
Bearer auth and always targets official Responses. `ollama` stays
keyless. `openrouter` targets `https://openrouter.ai/api/v1/responses`
and maps optional `referer`/`title` keywords to the non-secret
`HTTP-Referer`/`X-Title` attribution headers; OpenRouter's provider
routing (a `provider` object, a `models` fallback array, and
`:nitro`/`:floor` model-name suffixes) passes through model settings
unchanged rather than through dedicated constructor arguments. The
factories do not read environment variables. `Agent.gateway()` stays as
a thin dispatcher onto the three dedicated crates. Supported
`wire_protocol` values are `openai_responses`, `anthropic_messages`, and
`ollama_chat`. `openai_chat` is a configuration error. There is no
multi-protocol gateway crate. `Agent.e2b_sandbox()` is the T4 sandbox
constructor on both bindings; see [toolsets](toolset.md).

Two native media toolsets are registrable from any linked constructor.
`finstack-ai-tools-openrouter-media` publishes five tools —
`openrouter_generate_image`, `openrouter_generate_speech`,
`openrouter_transcribe_audio`, `openrouter_generate_video`, and
`openrouter_get_video` (which accepts a `wait_seconds` argument, 0-300,
to poll a video job inline until it completes or the time budget is
spent) — and always authenticates against OpenRouter, so it carries its
own API key even when the chat model is served by another provider.
`Agent::openrouter` reuses its own key and attribution (`media_tools:
bool`); `Agent::openai`, `Agent::anthropic`, and `Agent::ollama` accept
an independent `openrouter_media: Option<OpenRouterMediaToolsSpec>`.
`finstack-ai-tools-openai-media` publishes three tools —
`openai_generate_image`, `openai_generate_speech`, and
`openai_transcribe_audio` — reusing the OpenAI Responses credential; only
`Agent::openai` exposes it, via `media_tools: bool`. Both toolsets may be
registered on the same `Agent::openai` construction simultaneously. All
OpenRouter media tool calls are billed to the configured OpenRouter API
key, independent of which provider serves the chat model.

## Media input

A host-supplied `MediaResolver` (ADR-049) turns a kernel `BlobRef` into
bytes or a URL a provider can put on the wire. It is not a registered
port — construct an implementation and attach it to a provider config
with `with_media_resolver` (`OpenRouterConfig`, `OpenAIConfig`,
`AnthropicConfig`, `OllamaConfig`), mirroring ADR-048's
`with_credential_store`. Without a configured resolver, a media-bearing
user message fails closed rather than being silently dropped. Each
provider also advertises per-model `InputCapabilities` toggles
(`with_input_images`, `with_input_audio`, `with_input_files`) that gate
which `ContentBlock` variants are accepted; the OpenRouter catalog fetch
can set these automatically from `architecture.input_modalities`. The
supported modalities differ by provider:

| Provider | Images | Files / documents | Audio |
| --- | --- | --- | --- |
| `finstack-ai-provider-openrouter` | Yes | Yes | Yes (unverified on Responses; see crate README) |
| `finstack-ai-provider-openai` | Yes | Yes | Yes |
| `finstack-ai-provider-anthropic` | Yes | Yes (documents) | No |
| `finstack-ai-provider-ollama` | Yes (base64 only) | No | No |

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
