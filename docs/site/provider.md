# Providers

Model providers are separate crates. The default SDK graph does not construct
a client on import.

| Crate | Notes |
| --- | --- |
| `finstack-ai-provider-openai` | Official OpenAI Responses; HTTPS Bearer |
| `finstack-ai-provider-ollama` | Native `/api/chat`; keyless loopback |
| `finstack-ai-provider-anthropic` | Anthropic Messages leaf |
| `finstack-ai-provider-gemini` | Gemini `generateContent` leaf (always-streaming SSE); Generative Language API or Vertex AI |
| `finstack-ai-provider-openrouter` | OpenRouter Responses (`/api/v1/responses`); HTTPS Bearer; optional attribution headers; model-catalog fetch |
| `finstack-ai-test` (`ScriptedModel`) | Semantic scripted tests; no HTTP |

Python lazy extras (`finstack_ai.providers.*`) load on attribute access.
`Agent.openai()`, `Agent.anthropic()`, `Agent.gemini()`,
`Agent.ollama()`, `Agent.openrouter()`, and `Agent.gateway()` are the
same Rust-owned constructors on Python and WASM. wasm-host methods
exist; fail-closed is a Rust platform error
(`agent_run_unsupported_plan`), not a missing method. They accept the
same keyword-only T2 Python ports as `Agent.from_python`. `openai` maps
required keyword-only `api_key` to Bearer auth and always targets
official Responses. `ollama` stays keyless. `openrouter` targets
`https://openrouter.ai/api/v1/responses` and maps optional
`referer`/`title` keywords to the non-secret `HTTP-Referer`/`X-Title`
attribution headers; OpenRouter's provider routing (a `provider`
object, a `models` fallback array, and `:nitro`/`:floor` model-name
suffixes) passes through model settings unchanged rather than through
dedicated constructor arguments. The factories do not read environment
variables. Linked constructors do not attach a `MediaResolver`; vision,
file, and audio **input** require a host-built provider. ADR-049
rejected FFI resolvers on those factories. `Agent.gateway()` stays as a
thin dispatcher onto the dedicated crates. Supported `wire_protocol`
values are `openai_responses`, `anthropic_messages`, `ollama_chat`, and
`gemini_generate_content`. `openai_chat` is a configuration error.
There is no multi-protocol gateway crate. `Agent.e2b_sandbox()` is the
T4 sandbox constructor on both bindings; see [toolsets](toolset.md).

## Gemini

```rust,ignore
use finstack_ai::{Agent, GeminiAgentSpec, LinkedCommon};

let agent = Agent::gemini(GeminiAgentSpec {
    endpoint: "https://generativelanguage.googleapis.com".into(),
    model: "gemini-2.5-flash".into(),
    api_key: Some("...".into()),
    openrouter_media: None,
    common: LinkedCommon::default(),
})
.await?;
```

`Agent.gemini(endpoint, model, api_key=None, ...)` maps 1:1 onto
`GeminiAgentSpec` (ADR-045: no logic in the facade); HTTPS is required
whenever `api_key` is set, keyless HTTP loopback is allowed, and the
binding never reads environment variables. `endpoint` is passed
straight into `GeminiConfig::try_new` — the crate does not hardcode the
Google host. `GeminiAgentSpec` has no Vertex-specific fields yet; the
Vertex AI endpoint variant is reachable only by constructing
`GeminiProvider` directly with `GeminiConfig::try_new_vertex` (a
host-built provider, not through the linked constructor).

`GeminiEndpoint` is an auth/URL variant of the same wire protocol, not
a fourth protocol:

| Endpoint | Host | Credential |
| --- | --- | --- |
| `GenerativeLanguage` (`Agent::gemini`, `GeminiConfig::try_new`) | `generativelanguage.googleapis.com` | `x-goog-api-key` (ApiKey) or `Authorization: Bearer` |
| `Vertex { project, location }` (`GeminiConfig::try_new_vertex`, host-built only) | `{location}-aiplatform.googleapis.com` | `x-goog-api-key` (express mode) or `Authorization: Bearer` (OAuth token from the host's `CredentialStore`; the crate never mints tokens) |

Both variants send an identical wire body; only URL construction and
the credential header shape differ.

Recognized `gemini.*` model-settings keys (consumed by the adapter, not
flattened onto the wire body):

| Key | Effect |
| --- | --- |
| `gemini.google_search` | Activates Google Search grounding as a provider-executed tool (`{"googleSearch":{}}`), merged into the wire `tools` array. |
| `gemini.code_execution` | Activates code execution as a provider-executed tool (`{"codeExecution":{}}`). |
| `gemini.cached_content` | Maps to the request body's `cachedContent` field (context-cache pass-through; `cachedContents` CRUD is a host concern, out of scope). |
| `gemini.thinking` | Overrides the model-config thinking budget (`generationConfig.thinkingConfig.thinkingBudget`). |
| `thinking_level` | Shared allowlist (`low`/`medium`/`high` → thinkingBudget 1024/4096/8192, `includeThoughts: true`); model-config default otherwise. |

`Usage::extension_counters` carries two Gemini-specific counters (the
Anthropic cache-counter precedent), sourced from `usageMetadata` and
suppressed when zero:

| Counter | Source |
| --- | --- |
| `gemini.thoughts_token_count` | `usageMetadata.thoughtsTokenCount` |
| `gemini.cached_content_token_count` | `usageMetadata.cachedContentTokenCount` |

Thought signatures, grounding metadata, executable code, and
code-execution results round-trip through the opaque
`continuation_state` blob and `ContentBlock::Opaque` blocks
respectively — never as fake tool calls. The continuation envelope's
provider string is `gemini.generate-content`.

Out of scope for v1 (see spec §9): `cachedContents` create/update/delete
API; a Gemini Files API upload helper (hosts implement one as a
`MediaResolver` returning `ResolvedMedia::Url` file URIs); Live API /
`bidiGenerateContent`, image generation, embeddings, and TTS; OAuth
token minting/refresh for Vertex; non-streaming `generateContent`; and
multi-candidate responses (`candidateCount` is pinned to `1`).

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
(`with_input_images`, `with_input_audio`, `with_input_files`) that both
advertise and enforce which `ContentBlock` variants are accepted: draft
translation rejects a media block with that crate's `*_request_invalid`
error when its modality flag is off, even if the block resolved
successfully. The OpenRouter catalog fetch can set image and file flags
from `architecture.input_modalities`. OpenRouter Responses `input_audio`
is unverified and opt-in: it is off by default and is never enabled by
catalog fetch. Hosts enable it only with `with_input_audio(true)` after
confirming the target model. The supported modalities differ by
provider:

| Provider | Images | Files / documents | Audio |
| --- | --- | --- | --- |
| `finstack-ai-provider-openrouter` | Yes | Yes | Opt-in only; Responses `input_audio` is unverified |
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
