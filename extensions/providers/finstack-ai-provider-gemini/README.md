# finstack-ai-provider-gemini

Native Gemini `generateContent` provider for the public `finstack-ai-runtime`
`Model` port. It talks `POST {base}/{model-path}:streamGenerateContent?alt=sse`
directly (always-streaming, unnamed SSE `data:` frames), preserving native
Gemini surfaces that a text-only OpenRouter route would drop: thought
signatures, Google Search grounding, code execution, context caching, and
native audio/video/file input via `inlineData`/`fileData`.

`GeminiEndpoint` is an auth/URL variant, not a separate protocol: the
`GenerativeLanguage` variant targets the public Generative Language API with
an `x-goog-api-key` credential, while the `Vertex { project, location }`
variant targets Vertex AI's `aiplatform.googleapis.com` host with a bearer
OAuth token supplied by the host's `CredentialStore`. Both variants send an
identical wire body; only URL construction and the credential header shape
differ. This crate is a T1 native adapter and a leaf: neither the kernel nor
the runtime depends on it, and it is not isolated.

`Agent::gemini` (the frozen linked-constructor name, ADR-045) only reaches
the `GenerativeLanguage` endpoint; the `Vertex` variant is reachable by
constructing `GeminiProvider` directly with `GeminiConfig::try_new_vertex`.

## Model-settings keys

Recognized `gemini.*` keys, consumed by the request adapter rather than
flattened onto the wire body:

| Key | Effect |
| --- | --- |
| `gemini.google_search` | Activates Google Search grounding as a provider-executed tool (`{"googleSearch":{}}`), merged into the wire `tools` array next to function declarations. |
| `gemini.code_execution` | Activates code execution as a provider-executed tool (`{"codeExecution":{}}`). |
| `gemini.cached_content` | Maps to the request body's `cachedContent` field. Pass-through + accounting only; `cachedContents` create/update/delete is a host concern (out of scope for this leaf). |
| `gemini.thinking` | Overrides the model-config default thinking budget (`generationConfig.thinkingConfig.thinkingBudget`). |
| `thinking_level` | Shared allowlist across leaves: `low`/`medium`/`high` map to `thinkingBudget` 1024/4096/8192 and set `includeThoughts: true`; falls back to the model-config default when absent. |

`tools`, `model`, `contents`, `systemInstruction`/`system_instruction`,
`toolConfig`/`tool_config`, `stream`, `generationConfig`/
`generation_config`, `cachedContent`/`cached_content`, `responseSchema`,
`responseJsonSchema`, and `responseMimeType` are reserved settings keys —
setting them directly is a `gemini_request_invalid` error.
Any settings key not on the reserved list is passed through to the
request body top-level unchanged; `safetySettings` is the canonical
example.

## Extension usage counters

`Usage::extension_counters` carries two Gemini-specific counters sourced
from `usageMetadata`, suppressed when zero (the Anthropic cache-counter
precedent):

| Counter | Source |
| --- | --- |
| `gemini.thoughts_token_count` | `usageMetadata.thoughtsTokenCount` |
| `gemini.cached_content_token_count` | `usageMetadata.cachedContentTokenCount` |

## Continuation state

Thought signatures round-trip through the opaque
`ModelResponse.continuation_state` blob rather than through visible
content, carrying the complete prior model-turn `Content` objects
(including `thoughtSignature` fields) verbatim in an envelope
`{"provider":"gemini.generate-content","version":1,"replay_contents":[…]}`.
This blob is secret-bearing provider state (SEC-INV-005): never logged,
never in observer payloads, and fixtures use fabricated signatures.
Grounding metadata, executable code, and code-execution results surface
as `ContentBlock::Opaque` blocks, never as fake tool calls.
