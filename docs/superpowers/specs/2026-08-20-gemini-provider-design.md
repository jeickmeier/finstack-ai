# Gemini Native Provider — Design

Date: 2026-08-20
Status: Draft
Related: [ADR-040](../../implementation/adrs/ADR-040-openai-responses-native-ollama.md),
[ADR-045](../../implementation/adrs/ADR-045-first-class-sdk-constructors.md),
[ADR-047](../../implementation/adrs/ADR-047-retire-multi-protocol-gateway.md),
[ADR-048](../../implementation/adrs/ADR-048-shared-authority-and-provider-secret.md),
[ADR-049](../../implementation/adrs/ADR-049-media-resolver.md),
[Implementation plan](../plans/2026-08-20-gemini-provider.md)

## 1. Problem

OpenRouter can route to Gemini text models, but it drops Gemini's native
surfaces: thought signatures, Google Search grounding, code execution,
context caching, and native audio/video/file input. Those surfaces only
exist on the native `generateContent` protocol. ADR-040 already committed
the workspace to one dedicated leaf per vendor protocol, so a native
Gemini leaf is consistent with the architecture, not an exception to it.

## 2. Decision summary

1. New leaf crate `finstack-ai-provider-gemini` at
   `extensions/providers/finstack-ai-provider-gemini`, implementing the
   `Model` port over
   `POST {base}/{model-path}:streamGenerateContent?alt=sse` (SSE, always
   streaming), shaped exactly like the Anthropic/OpenAI leaves.
2. **Vertex AI is an auth/endpoint variant of this crate, not a fourth
   protocol.** `GeminiEndpoint` is an enum
   (`GenerativeLanguage` | `Vertex { project, location }`) that only
   changes URL construction and the credential header shape
   (`x-goog-api-key` vs `Authorization: Bearer`). The wire body and
   stream assembly are identical.
3. Thought signatures round-trip through the opaque
   `ModelResponse.continuation_state` blob (ADR-040 clause 3), envelope
   `{"provider":"gemini.generate-content","version":1,"replay_contents":[…]}`
   carrying the complete prior model-turn `Content` objects (including
   `thoughtSignature` fields) verbatim. Continuation blobs are
   secret-bearing provider state (SEC-INV-005): never logged, never in
   observer payloads, fixtures use fabricated signatures.
4. Google Search grounding and code execution are **provider-executed
   tools activated via model settings keys** (`gemini.google_search`,
   `gemini.code_execution`), merged by the adapter into the wire `tools`
   array next to function declarations. `tools` itself stays a reserved
   settings key. Grounding metadata and code-execution parts surface as
   `ContentBlock::Opaque` blocks (the Anthropic thinking-signature
   precedent), never as fake tool calls.
5. Context caching v1 = pass-through + accounting: settings key
   `gemini.cached_content` maps to the body's `cachedContent` field, and
   `usageMetadata.cachedContentTokenCount` /
   `thoughtsTokenCount` land in `Usage::extension_counters` under
   `gemini.cached_content_token_count` / `gemini.thoughts_token_count`
   (the Anthropic cache-counter precedent). `cachedContents` CRUD is a
   host concern and out of scope for the Model leaf.
6. Public constructor `Agent::gemini` (frozen name per ADR-045); leaf in
   `FORBIDDEN_WASM`; wasm-host mirror returns
   `agent_run_unsupported_plan`. `Agent::gateway` gains a
   `"gemini_generate_content"` wire-protocol arm (ADR-047 keeps gateway
   as a thin dispatcher over dedicated leaves).
7. A new ADR-051 records decisions 1–6 (ADR-050 is taken by the object-store contract).

## 3. Wire protocol facts the design leans on

- SSE frames from `streamGenerateContent?alt=sse` are unnamed `data:`
  lines, each a `GenerateContentResponse` JSON chunk — unlike Anthropic
  there are **no event names**, so the Gemini SSE wrapper must accept
  unnamed frames (opposite of the Anthropic wrapper's fail-closed rule).
- A chunk carries `candidates[0].content.parts[]`, optional
  `finishReason`, optional `groundingMetadata`, optional cumulative
  `usageMetadata`, `responseId`, `modelVersion`.
- A `Part` is a struct of optional fields: `text`, `thought: bool`,
  `thoughtSignature` (base64, opaque), `inlineData {mimeType, data}`,
  `fileData {mimeType, fileUri}`, `functionCall {id?, name, args}`,
  `functionResponse {id?, name, response}`, `executableCode
  {language, code}`, `codeExecutionResult {outcome, output}`.
- `functionCall.id` is optional on the wire; when absent the leaf omits
  `provider_call_id` (ADR-040 clause 5, Ollama precedent).
- System prompt is `systemInstruction: {parts:[{text}]}` (one field, so
  system/developer messages must be a stable leading prefix — same rule
  as the Anthropic adapter).
- Structured output is native: `generationConfig.responseMimeType:
  "application/json"` + `generationConfig.responseJsonSchema` (accepts
  JSON Schema 2020-12) → `StructuredOutputCapability::Native`.
- Thinking knobs: `generationConfig.thinkingConfig {thinkingBudget,
  includeThoughts}`.
- Media: `inlineData` (base64, bounded by the ~20 MB request cap) or
  `fileData.fileUri` (Files API URIs, GCS URIs on Vertex, YouTube URLs).
  `ResolvedMedia::Url` → `fileData`, `ResolvedMedia::Bytes` →
  `inlineData` (ADR-049). Video rides on `ContentBlock::File` with a
  `video/*` media type — the kernel has no Video block and does not need
  one.
- Vertex path shape:
  `https://{location}-aiplatform.googleapis.com/v1/projects/{project}/locations/{location}/publishers/google/models/{model}:streamGenerateContent?alt=sse`
  (global endpoint: host `aiplatform.googleapis.com`). Body is identical.

## 4. Crate layout

```
extensions/providers/finstack-ai-provider-gemini/
├── Cargo.toml            # canonical leaf shape, vendored-tls feature
├── README.md
├── src/
│   ├── lib.rs            # module decls + re-exports + standard lint preamble
│   ├── config.rs         # GeminiConfig, GeminiEndpoint, GeminiModelConfig, SecretHeader
│   ├── error.rs          # gemini_* stable codes + error() constructor
│   ├── request.rs        # draft → GenerateContentRequest translation + replay
│   ├── provider.rs       # GeminiProvider, Model impl, drive_response
│   └── sse.rs            # thin wrapper over shared SseEventParser (unnamed frames OK)
├── tests/
│   ├── conformance.rs
│   ├── provider_fixtures.rs
│   └── capability_catalog.rs
└── benches/request_overhead.rs
```

Plus a shared normalizer in the runtime (the pattern every leaf follows):
`crates/finstack-ai-runtime/src/ports/model/provider_util/gemini_generate_content.rs`
exporting `GeminiGenerateContentAssembly` with the same
`new(request_id, structured)` / `consume(data) -> Vec<ModelStreamItem>` /
finish-on-terminal shape as `AnthropicMessagesAssembly`.

## 5. Config and auth

`GeminiConfig` builder fields: `endpoint: GeminiEndpoint`,
`api_version` (default `v1beta` for GenerativeLanguage, `v1` for
Vertex), `CredentialStore` + `CredentialReference`,
`Vec<SecretHeader>`, `request_timeout` (2 min), `max_event_bytes`
(1 MiB), `max_stream_bytes` (16 MiB), optional `Arc<dyn MediaResolver>`.

Auth mapping (ADR-048, no env reads, hand-written redacting `Debug`):

| Endpoint | `Authentication::ApiKey` | `Authentication::Bearer` |
|---|---|---|
| GenerativeLanguage | `x-goog-api-key` (sensitive) | `Authorization: Bearer` |
| Vertex | `x-goog-api-key` (express mode) | `Authorization: Bearer` (OAuth token from the host's `CredentialStore`) |

The crate never mints OAuth tokens; Vertex hosts supply a ready token
via `CredentialStore` (token refresh is a host concern, same boundary as
every other leaf). HTTPS is required whenever any credential or secret
header is present; keyless loopback HTTP is the only plaintext path.
`SecretHeader::try_new` rejects provider-owned names (`authorization`,
`x-goog-api-key`, `content-type`).

`GeminiModelConfig` fact table: `name`, `hard_input_bytes`,
`context_window_tokens`, `max_output_tokens`, `reserved_output_tokens`,
`provider_overhead_tokens`, plus toggles `parallel_tool_calls`,
`thinking` / `thinking_budget_tokens`, `input_images`, `input_audio`,
`input_files` (covers video), `google_search`, `code_execution`,
`cached_content`. `capabilities()` / `apply_capabilities()` /
`estimator_ref()` mirror the Anthropic config. Estimator:
`gemini.utf8-byte-upper-bound` v1, `ConservativeUpperBound`,
`estimate_input_tokens = canonical_request.len()` (`countTokens` is a
network call, so it cannot be the estimator).

Capabilities: `structured_output: Native`, `reasoning` from the
`thinking` flag, `prompt_cache` from `cached_content`,
`native_tool_calls: true`, `idempotent_requests: false`,
`native_capabilities = {"gemini.generate-content", "gemini.sse"}` ∪
`{"gemini.google-search" if google_search}` ∪
`{"gemini.code-execution" if code_execution}`.

## 6. Request translation rules

- `RESERVED_SETTINGS`: `model`, `contents`, `systemInstruction`,
  `system_instruction`, `tools`, `toolConfig`, `tool_config`, `stream`,
  `generationConfig`, `generation_config`, `cachedContent`,
  `cached_content`, `responseSchema`, `responseJsonSchema`,
  `responseMimeType`, `safetySettings` stays allowed as passthrough?
  No — `safetySettings` is allowed passthrough (top-level flatten);
  everything listed before it is a hard `gemini_request_invalid`.
- Recognized settings keys consumed by the adapter (not flattened):
  `gemini.google_search: true|{...}`, `gemini.code_execution: true`,
  `gemini.cached_content: "cachedContents/…"`, `gemini.thinking`
  (budget override), plus the shared `thinking_level` allowlist
  (`low|medium|high` → thinkingBudget 1024/4096/8192, and
  `includeThoughts: true`), model-config default otherwise.
- System/developer messages: stable leading prefix →
  `systemInstruction`; any later system message is an error.
- Roles: assistant → `model`; user → `user`; `MessageRole::Tool` →
  `user` content with a `functionResponse` part whose `name` is looked
  up from the matching `ToolCall` block earlier in the conversation and
  whose `id` is the provider call id when one was captured.
- Tools: each `ToolSpec` → one entry in a single
  `{"functionDeclarations":[…]}` tool; native tools append
  `{"googleSearch":{}}` / `{"codeExecution":{}}` objects to the same
  `tools` array. Explicit `gemini.*` settings are honored without
  model-flag gating (the linked-surface convention shared with the
  Anthropic leaf) — the per-model flags drive capability advertising
  and the thinking default, never a veto. No client-side compatibility
  matrix: if a model rejects the combination, the HTTP error propagates
  as `gemini_http_error`.
- `generationConfig.maxOutputTokens =
  draft.limits.max_output_tokens.min(model.max_output_tokens)`.
- Structured output: `OutputSpec::JsonSchema` →
  `responseMimeType: "application/json"` +
  `responseJsonSchema: <schema>`; capability `Native`; the
  `finstack.internal.submit_final_output` tool must NOT be present
  (mirror of the OpenAI leaf's rule).
- Continuation replay: parse the envelope, reject provider/version
  mismatch, and splice `replay_contents` verbatim (signatures intact)
  over ONLY the assistant turn it replays — the last one. All earlier
  and later conversation turns still map from the kernel transcript:
  `generateContent` is stateless, so dropping pre-assistant user turns
  would erase the request the model is answering. Outbound
  `ContentBlock::Opaque` is dropped (all leaves do this — replay comes
  from continuation state, not from content blocks).
- Media: image/audio/file blocks gated per-model flags;
  `ResolvedMedia::Url` → `fileData`, `Bytes` → base64 `inlineData`.

## 7. Stream assembly

`GeminiGenerateContentAssembly` consumes unnamed SSE `data:` payloads:

- `parts[].text` with `thought:false` → `TextDelta`; with
  `thought:true` → `ReasoningDelta` (thought text stays transient;
  signatures never appear in deltas).
- `parts[].functionCall` → `ToolCallDelta` then a final
  `ModelToolCall { name, arguments, provider_call_id: id? }`.
- `parts[].thoughtSignature` → captured into the continuation
  accumulator only.
- `groundingMetadata` → one `ContentBlock::Opaque` with media type
  `application/vnd.finstack.gemini.grounding-metadata`, payload
  `Json(raw)`; `executableCode` / `codeExecutionResult` parts → Opaque
  blocks `…gemini.executable-code` / `…gemini.code-execution-result`.
- `usageMetadata` (cumulative) → `UsageDelta`; final usage maps
  `promptTokenCount` to input tokens and derives output tokens as
  `candidatesTokenCount + thoughtsTokenCount`, with total tokens
  computed as input + output — the wire `totalTokenCount` is
  deliberately not trusted, since Gemini's `candidatesTokenCount`
  excludes thoughts and using the wire total would violate the
  kernel's input + output == total invariant. `thoughtsTokenCount`/
  `cachedContentTokenCount` are also recorded to extension counters
  (zeros suppressed).
- Chunk with `finishReason: STOP|MAX_TOKENS` → assemble
  `Completed(ModelResponse)` with `continuation_state` envelope built
  from the accumulated model-turn contents; `SAFETY`/`RECITATION`/other
  → `gemini_response_invalid` (non-retryable); EOF without a
  `finishReason` → `gemini_stream_invalid`. `promptFeedback.blockReason`
  with no candidates → `gemini_response_invalid`.
- `candidates[1..]` are ignored (the request always pins
  `candidateCount: 1`).

## 8. Wiring

- Workspace: root `Cargo.toml` member + path dep; `finstack-ai`'s
  `native-tokio` feature adds `dep:finstack-ai-provider-gemini` with
  `vendored-tls`; crate added to `FORBIDDEN_WASM`.
- `gemini_inner` in `crates/finstack-ai/src/agent/linked.rs` (ids
  `python.agent.gemini` / `python.bundle.gemini` /
  `python.model.gemini`), public `Agent::gemini`; wasm-host mirror
  returns `agent_run_unsupported_plan`; `gateway_provider` gains the
  `"gemini_generate_content"` arm (ApiKey-shaped auth by default).
- Python/WASM bindings: argument-mapping facades only (ADR-045);
  public-api baselines regenerated.
- Fixtures under `fixtures/compatibility/providers/v1/gemini/`
  (`valid--text.sse`, `valid--tool.sse`, `valid--structured.sse`,
  `valid--thinking.sse`, `valid--grounding.sse`, `error--http-429.json`)
  with fabricated signatures.
- Docs: README, `docs/site/provider.md` section, ADR + register row.

## 9. Out of scope (v1)

- `cachedContents` create/update/delete API (host concern; revisit if a
  host needs leaf-mediated cache management).
- Gemini Files API upload helper (hosts implement it as a
  `MediaResolver` that returns `ResolvedMedia::Url` file URIs).
- Live API / bidiGenerateContent, image generation, embeddings, TTS.
- OAuth token minting/refresh for Vertex.
- Non-streaming `generateContent` (the leaf always streams, like every
  other leaf).
- Multi-candidate responses (`candidateCount` pinned to 1).

## 10. Security notes

- Thought signatures and the continuation envelope are secret-bearing
  provider state (SEC-INV-005, TM-04): excluded from `Debug`, logs,
  observer payloads, and generated fixtures.
- Credential redaction canary tests (the `CANARY` pattern from the
  Anthropic config tests) cover `GeminiConfig`, `SecretHeader`, and
  both endpoint variants.
- `validate_base_url` rules identical to the Anthropic leaf: http(s)
  only, no userinfo/query/fragment; HTTPS mandatory with credentials.
