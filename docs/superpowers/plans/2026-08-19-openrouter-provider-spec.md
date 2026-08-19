# OpenRouter Provider Extension — Spec

Date: 2026-08-19. Investigation of the August 2026 OpenRouter API and of this
repository's provider architecture (see `docs/site/provider.md`, ADR-040,
ADR-045, ADR-047, ADR-048).

## Goal

Add `finstack-ai-provider-openrouter`, a T1 in-process provider crate
implementing the `Model` port against OpenRouter, plus a model-catalog fetch
helper (`GET /api/v1/models`), a first-class SDK constructor
(`Agent::openrouter`), and Python/WASM binding parity.

## OpenRouter API facts (verified August 2026)

- Base URL `https://openrouter.ai/api/v1`, auth `Authorization: Bearer <key>`.
- **Responses API** `POST /api/v1/responses` — GA since 2026-07-25,
  OpenAI-Responses-compatible, SSE streaming via `stream: true`, and
  **stateless**: requests with `store: true` or `previous_response_id` are
  rejected with HTTP 400. Supports reasoning (effort levels), parallel tool
  calling, and `text.format` structured output.
- Optional attribution headers: `HTTP-Referer`, `X-Title` (non-secret).
- Provider routing: a `provider` request object (`order`, `only`, `ignore`,
  `allow_fallbacks`, `sort`, `max_price`, `require_parameters`,
  `data_collection`, `zdr`, `quantizations`, …) and a `models` fallback
  array; model-name suffixes `:nitro` / `:floor`.
- `GET /api/v1/models` returns `{"data": [Model, …]}` where each `Model` has
  `id`, `name`, `context_length`, `pricing`, `top_provider`
  (incl. `max_completion_tokens`), and `supported_parameters`.

## Decisions

1. **Wire protocol: OpenAI Responses**, not Chat Completions. Reuses the
   shared `OpenAiResponsesAssembly` in
   `crates/finstack-ai-runtime/src/ports/model/provider_util/`. Chat
   Completions support was deliberately removed by ADR-040/ADR-047; no new
   ADR is required for this crate because it introduces no shared protocol
   machinery.
2. The wire request **omits `store` entirely** (OpenRouter rejects
   `store: true`; the field is unnecessary on a stateless API) and never
   sends `previous_response_id`. Both stay reserved settings.
3. The continuation-state envelope keeps the provider tag
   `"openai.responses"` because the shared assembly generates it; it
   identifies the wire protocol, not the vendor (same as the gateway path).
4. Attribution headers are **plain config fields** (`with_attribution`), not
   `SecretHeader`s; they do not force HTTPS on their own. Credentials and
   secret headers require HTTPS exactly as in the OpenAI crate.
5. Provider routing (`provider` object, `models` array, `:nitro`/`:floor`
   suffixes) flows through `ModelSettings` untouched — none of those keys is
   reserved, and model names are opaque.
6. Catalog follow-on: a pure parser
   (`model_configs_from_catalog_json`) plus
   `OpenRouterProvider::fetch_model_catalog(hard_input_bytes)` doing
   `GET {base}/api/v1/models`. Mutation stays explicit: callers apply the
   result via the existing `replace_model_catalog` pattern.
7. Frozen identity: provider string `"openrouter"`, estimator
   `"openrouter.utf8-byte-upper-bound"` v1, native capability sentinel
   `"openrouter.responses"`, error codes `openrouter_config_invalid`,
   `openrouter_request_invalid`, `openrouter_http_error`,
   `openrouter_transport_error`, `openrouter_timeout`,
   `openrouter_cancelled`, `openrouter_stream_invalid`,
   `openrouter_stream_limit_exceeded`, `openrouter_response_invalid`.
8. SDK constructor defaults: base `https://openrouter.ai`, 2-minute timeout,
   the shared linked context-window constants, reasoning settings identical
   to `Agent::openai`.
9. Bindings: Python gains `Agent.openrouter(...)` and an availability probe;
   `linked_providers()` widens from a 3-tuple to a 4-tuple. WASM gains a
   fail-closed `openrouter` method, and the crate is added to
   `tools/wasm_package/check.py` `FORBIDDEN_WASM`.

## Non-goals

- No Chat Completions support (`openai_chat` stays a configuration error).
- No gateway `wire_protocol` arm: `Agent::gateway` with
  `endpoint = "https://openrouter.ai/api/v1/responses"` and
  `wire_protocol = "openai_responses"` already reaches OpenRouter today.
- No use of OpenRouter's multimodal endpoints, `/generation`, or plugins.
- No `reconcile` implementation beyond `Unknown` (matches OpenAI).
