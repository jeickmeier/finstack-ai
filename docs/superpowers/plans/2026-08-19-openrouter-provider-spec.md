# OpenRouter Provider Extension — Spec

Date: 2026-08-19 (multimodal scope added same day). Investigation of the
August 2026 OpenRouter API and of this repository's provider architecture
(see `docs/site/provider.md`, ADR-040, ADR-045, ADR-047, ADR-048).

## Goal

Integrate OpenRouter across modalities in three phases:

- **Phase 1**: `finstack-ai-provider-openrouter`, a T1 in-process provider
  crate implementing the `Model` port (text, JSON, tool calls, reasoning),
  plus a model-catalog fetch helper (`GET /api/v1/models`), a first-class SDK
  constructor (`Agent::openrouter`), and Python/WASM binding parity.
- **Phase 2**: `finstack-ai-tools-openrouter-media`, a T1 Toolset crate
  exposing OpenRouter's media **generation** endpoints (`/api/v1/images`,
  `/api/v1/videos`, `/api/v1/audio/speech`, `/api/v1/audio/transcriptions`)
  as agent tools, mirroring the E2B toolset pattern.
- **Phase 3**: media **input** (vision/audio/file prompts) through the
  provider, enabled by a new host-supplied `MediaResolver` contract in
  `finstack-ai-runtime` introduced via ADR.

## Modality coverage

| Modality | Direction | Phase | Mechanism |
|---|---|---|---|
| Text / JSON / tools / reasoning | in + out | 1 | `Model` port over `/api/v1/responses` |
| Image, audio, file prompts | in | 3 | `Model` port + `MediaResolver` (`ContentBlock::Image/Audio/File(MediaRef)` → `input_image`/`input_audio`/`input_file` wire items) |
| Image generation | out | 2 | `openrouter_generate_image` → `POST /api/v1/images`; native: `openai_generate_image` → OpenAI `/v1/images/generations` |
| Video generation | out | 2 | `openrouter_generate_video` → `POST /api/v1/videos` (async job) + `openrouter_get_video` → `GET /api/v1/videos/{id}` poll (no native equivalent at any of the other three vendors) |
| Speech synthesis | out | 2 | `openrouter_generate_speech` → `POST /api/v1/audio/speech`; native: `openai_generate_speech` → OpenAI `/v1/audio/speech` |
| Audio transcription | in→text | 2 | `openrouter_transcribe_audio` → `POST /api/v1/audio/transcriptions`; native: `openai_transcribe_audio` → OpenAI `/v1/audio/transcriptions` |

Media generation is deliberately **not** forced through the `Model` port: the
port's contract is a streaming conversation (`ModelStreamItem` carries
text/reasoning/tool-call deltas only), and OpenRouter serves generation from
separate endpoints. Tools are the architecture's fit (precedent: the T4 E2B
sandbox toolset).

### Cross-provider adoption

Both phases extend to the existing providers:

- **Generation (Phase 2)** — the OpenRouter media toolset is
  provider-agnostic by construction: it is an ordinary `Toolset` registered
  alongside *any* model, and OpenRouter's generation endpoints already
  route to OpenAI, Gemini, and other vendors' models. Every linked
  constructor (`openai`, `anthropic`, `ollama`, `openrouter`) gains
  convenience wiring to register it with its own OpenRouter API key.
  **Native generation** exists for exactly one of the other vendors:
  OpenAI (`/v1/images/generations`, `/v1/audio/speech`,
  `/v1/audio/transcriptions`) — covered by a second toolset crate,
  `finstack-ai-tools-openai-media`, billed to the host's OpenAI key.
  Anthropic and Ollama expose **no media-generation APIs upstream**; for
  agents on those providers, generation is available only through the
  OpenRouter toolset. The prefixed tool names (`openai_generate_image` vs
  `openrouter_generate_image`) let one agent register both backends
  side by side.
- **Media input (Phase 3)** — `MediaResolver` lives in the runtime precisely
  so every provider can adopt it. Per-provider modality support:

| Provider | Images in | Files/PDF in | Audio in | Wire mapping |
|---|---|---|---|---|
| openrouter | ✓ | ✓ | ✓* | Responses `input_image` / `input_file` / `input_audio` |
| openai | ✓ | ✓ | ✓ | Responses `input_image` / `input_file` / `input_audio` (same shapes) |
| anthropic | ✓ | ✓ (PDF `document`) | ✗ (API has no audio input) | Messages `image` / `document` source blocks (base64 or URL) |
| ollama | ✓ (base64 bytes only) | ✗ | ✗ | `images: [<b64>, …]` on the chat message; URL resolutions rejected |

Unsupported modalities keep failing closed with each provider's
`*_request_invalid` code.

\* OpenRouter documents audio input only for `/api/v1/chat/completions`
(base64 only, no URLs); whether its Responses endpoint accepts
`input_audio` is verified at implementation time — if it does not, the
openrouter provider rejects `Audio` blocks fail-closed
(`openrouter_request_invalid`) until it does. Audio always requires
`ResolvedMedia::Bytes` on every provider (URL resolutions are rejected).

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
  (incl. `max_completion_tokens`), `supported_parameters`, and
  `architecture` (incl. `input_modalities: ["file","image","text"]` and
  `output_modalities`). A per-model endpoint
  `GET /api/v1/model/{author}/{slug}` and an `output_modalities` query
  filter also exist.
- **Image generation** `POST /api/v1/images`: request `{model, prompt}` plus
  optional `n`, `resolution`, `aspect_ratio`, `quality`, `output_format`,
  `stream`, `input_references`, `provider`. Response is **base64 only**:
  `{"data": [{"b64_json", "media_type"}], "usage": {"cost"}}` — no hosted
  URLs.
- **Video generation is asynchronous**: `POST /api/v1/videos`
  (`{model, prompt}` + optional `duration`, `resolution`, `aspect_ratio`,
  `frame_images`, `input_references`, `generate_audio`, `callback_url`)
  returns 202 `{"id", "polling_url", "status": "pending"}`;
  `GET /api/v1/videos/{id}` reports `pending → in_progress → completed |
  failed` and, when completed, `unsigned_urls: [...]` pointing at
  `GET /api/v1/videos/{id}/content`. `GET /api/v1/videos/models` lists video
  models.
- **Speech** `POST /api/v1/audio/speech` (OpenAI/Google/Mistral voices; MP3
  or PCM binary out). **Transcription** `POST /api/v1/audio/transcriptions`
  (base64-JSON body or OpenAI-style multipart file ≤ 25 MB; **no audio
  URLs**; 60-second upstream timeout; JSON `{"text", ...}` out).
- **Audio input to chat models** is documented for `/api/v1/chat/completions`
  via `{"type": "input_audio", "input_audio": {"data": <b64>, "format"}}` —
  base64 only, URLs unsupported. Whether the Responses endpoint accepts
  `input_audio` is not documented and must be verified at implementation.

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
   `scripts/wasm_package/check.py` `FORBIDDEN_WASM`.

## Decisions — Phase 2 (media-generation toolset)

10. Crate `extensions/toolsets/finstack-ai-tools-openrouter-media`, modeled
    line-for-line on `finstack-ai-sandbox-e2b` (explicit API key, no env
    reads, HTTPS off loopback, redacted `Debug`, bounded response reads,
    cancellation/deadline `select!`, `verify_authority`). **Five**
    `ToolSpec`s in one `Toolset`: `openrouter_generate_image`,
    `openrouter_generate_video` (submits the async job),
    `openrouter_get_video` (checks job status and returns `unsigned_urls`
    when completed; an optional `wait_seconds` argument makes it poll
    server-side inside the one call — bounded, cancellation- and
    deadline-aware — so the model does not burn a turn per poll; there is
    deliberately **no separate wait tool**, since it would be a strict
    superset of the status check), `openrouter_generate_speech`,
    `openrouter_transcribe_audio`.
11. Tool results are JSON and **bounded** by a configurable
    `max_result_bytes` (default 256 KiB, hard cap 8 MiB — hosts raise it to
    accept inline images/audio). Per the verified API shapes: image results
    are base64 (`b64_json` + `media_type` — the API offers no hosted image
    URLs); speech results are base64 audio; video tools return job ids and
    `unsigned_urls` (never bytes); transcription returns text. The
    transcription tool takes an HTTPS `audio_url`, downloads it itself
    (bounded to the documented 25 MB limit), and submits base64 JSON —
    OpenRouter accepts no audio URLs. Payloads exceeding the cap fail
    closed with `openrouter_media_limit_exceeded`. Raw media bytes never
    enter the journal.
12. Stable tool error codes: `openrouter_media_credential_required`,
    `openrouter_media_endpoint_invalid`, `openrouter_media_invalid_arguments`,
    `openrouter_media_transport_failed`, `openrouter_media_limit_exceeded`,
    `openrouter_media_timeout`.
13. Side-effect metadata: the four generating tools are
    `SideEffectClass::NonIdempotentWrite` (paid API calls),
    `RetrySafety::AtMostOnce`, `ApprovalRequirement::Policy`;
    `openrouter_get_video` is a read-only status poll
    (`SideEffectClass::ReadOnly`, `ApprovalRequirement::NotRequired`).
14. SDK wiring: `OpenRouterAgentSpec` gains `media_tools: bool`
    (default false); when true, `openrouter_inner` registers the toolset
    with the same API key and attribution. `OpenAiAgentSpec`,
    `AnthropicAgentSpec`, and `OllamaAgentSpec` gain
    `openrouter_media: Option<OpenRouterMediaToolsSpec>` (an
    `{ api_key, referer, title }` struct) — the toolset needs an OpenRouter
    key even when the chat model is served elsewhere. The crate joins the
    wasm forbidden list; the wasm/python bindings surface the knobs.

    **Naming rationale**: the `openrouter_` prefix is kept on the field and
    on the model-facing tool names even when registered with other
    providers' agents, because it names the real data flow — prompts leave
    for OpenRouter's API and are billed to an OpenRouter credential, a
    second vendor relative to the chat provider. A vendor-neutral name
    would obscure that; prefixed tool names also cannot collide with other
    registered tools (precedent: `e2b_run`) — which is what lets
    `openai_generate_image` and `openrouter_generate_image` coexist on one
    agent. The `media_tools: bool` / `openrouter_media: Option<..>`
    asymmetry is intentional: on the OpenRouter constructor the key and
    attribution already exist in the spec and the vendor is unambiguous.

14b. **Native OpenAI media toolset**: crate
    `extensions/toolsets/finstack-ai-tools-openai-media`, same skeleton and
    guarantees as the OpenRouter media toolset (explicit key, HTTPS off
    loopback, bounded JSON results, `AtMostOnce`/`Policy` metadata). Three
    tools against `https://api.openai.com`: `openai_generate_image`
    (`POST /v1/images/generations`), `openai_generate_speech`
    (`POST /v1/audio/speech`), `openai_transcribe_audio`
    (`POST /v1/audio/transcriptions`, `multipart/form-data` built from a
    bounded HTTPS download of the caller's `audio_url`). Because current
    OpenAI image models return base64 only (no hosted URLs), the config
    carries `max_result_bytes` (default 256 KiB, cap 8 MiB) so hosts can
    opt into inline image payloads; oversized results fail closed with
    `openai_media_limit_exceeded`. Error codes mirror the OpenRouter set
    under the `openai_media_` prefix. SDK wiring: `OpenAiAgentSpec` gains
    `media_tools: bool` reusing the spec's own OpenAI key (same shape as
    OpenRouter's flag); other constructors do not get it — pairing an
    OpenAI credential with a non-OpenAI agent is possible via explicit
    toolset registration but not a convenience path. No video: OpenAI has
    no video-generation endpoint on this API surface as of August 2026;
    re-verify at implementation time.
15. The exact wire DTOs for `/images`, `/videos`, `/audio/*` are verified
    against the live OpenRouter API reference at implementation time
    (Task 11 has an explicit verification step); the plan encodes the
    documented August-2026 shapes.

## Decisions — Phase 3 (media input via MediaResolver)

16. The kernel already represents media (`ContentBlock::Image/Audio/File`
    wrapping `MediaRef`/`BlobRef`), but `BlobRef` carries only labels — no
    bytes, no URL — and the runtime has no resolution machinery. A new
    **`MediaResolver`** contract is added to
    `finstack-ai-runtime` `ports/model/provider_util/` via ADR (next free
    number under `docs/implementation/adrs/`), following ADR-048's
    host-supplied-object precedent (like `CredentialStore`): it is *not* a
    seventh registered port; hosts hand an `Arc<dyn MediaResolver>` to
    provider configs.

    ```rust
    pub trait MediaResolver: PortObject {
        fn resolve(&self, blob: &BlobRef) -> PortFuture<Result<ResolvedMedia, MediaResolveError>>;
    }

    pub enum ResolvedMedia {
        Url(Arc<str>),
        Bytes { media_type: Arc<str>, bytes: Arc<[u8]> },
    }
    ```

17. Providers resolve media **inside `Model::request`'s async block, before
    building the wire request** (draft translation stays sync by taking a
    pre-resolved map keyed by blob id). Missing resolver + media content =
    `openrouter_request_invalid`, fail closed.
18. Wire mapping (Responses API): `Image` → `{"type":"input_image",
    "image_url": <url-or-data-URI>}`; `File` → `{"type":"input_file", ...}`;
    `Audio` → `{"type":"input_audio", "input_audio": {"data": <b64>,
    "format": ...}}` — exact shapes re-verified against the OpenRouter
    reference at implementation time.
19. `OpenRouterModelConfig` gains `with_input_images/with_input_audio/
    with_input_files` toggles feeding `InputCapabilities`; the catalog
    parser maps the models API's `architecture.input_modalities` onto them.
20. **Video input** rides the `File` path: the kernel has no `Video` content
    block — a video prompt is a `ContentBlock::File(MediaRef)` with a
    `video/*` media type, forwarded as a file/URL wire item to models that
    accept it. Adding a first-class `Video` block would be a kernel change
    and is out of scope.
21. **Cross-provider adoption**: `openai`, `anthropic`, and `ollama` each
    gain `with_media_resolver` on their configs, per-model input toggles on
    their model configs, and their own wire mapping (see the adoption matrix
    above). Wire mapping stays crate-local per ADR-047's leaves-own-their-
    protocol philosophy — the shared runtime piece is only the resolver
    contract and `ResolvedMedia`. Modalities a provider's API cannot accept
    keep failing closed (`anthropic` rejects `Audio` blocks; `ollama`
    rejects `File`/`Audio` blocks and URL resolutions).
22. Linked SDK constructors do **not** expose media resolvers in this
    effort: a resolver is a host-authored Rust object; bridging one from
    Python/JS callbacks is future work. Rust hosts construct providers
    directly with `with_media_resolver`.

## Non-goals

- No Chat Completions support (`openai_chat` stays a configuration error).
- No gateway `wire_protocol` arm: `Agent::gateway` with
  `endpoint = "https://openrouter.ai/api/v1/responses"` and
  `wire_protocol = "openai_responses"` already reaches OpenRouter today.
- No `/embeddings`, `/generation`, or plugins support.
- No image/media **output from the Model port** (`ModelStreamItem` has no
  media variant; generation goes through the Phase 2 tools).
- No blob **storage** service: `MediaResolver` resolves references the host
  already owns; persisting generated media is the host's concern.
- No `reconcile` implementation beyond `Unknown` (matches OpenAI).
