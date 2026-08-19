# finstack-ai-provider-openrouter

OpenRouter provider for finstack-ai, speaking OpenRouter's stateless,
OpenAI-compatible Responses API (`POST /api/v1/responses`, SSE streaming).
The request never carries `store` or `previous_response_id`; full context is
sent on every call and reasoning replay uses the framework's
continuation-state envelope.

## Construction

```rust
use finstack_ai_provider_openrouter::{
    Authentication, OpenRouterConfig, OpenRouterModelConfig, OpenRouterProvider, SecretString,
};

let config = OpenRouterConfig::try_new("https://openrouter.ai")
    .expect("config")
    .with_authentication(Authentication::Bearer(
        SecretString::try_new("sk-or-example").expect("secret"),
    ))
    .with_attribution(Some("https://example.app"), Some("Example App"))
    .expect("attribution");
let model = OpenRouterModelConfig::try_new(
    "openai/gpt-5",
    1_000_000,
    400_000,
    128_000,
    128_000,
    64,
)
.expect("model");
let provider = OpenRouterProvider::try_new(config, vec![model]).expect("provider");
```

## Provider routing

OpenRouter's routing controls pass through model settings untouched — set
`provider` (e.g. `{"order": ["openai"], "sort": "throughput"}`) or a
`models` fallback array in `ModelSettings`, or use `:nitro` / `:floor`
model-name suffixes.

## Model catalog

`model_configs_from_catalog_json` maps a `GET /api/v1/models` body onto
conservative `OpenRouterModelConfig` entries;
`OpenRouterProvider::fetch_model_catalog(hard_input_bytes)` performs the GET.
Applying the result stays explicit via `replace_model_catalog`.

## Media input (images, audio, files)

Attach a host-supplied `MediaResolver` via `OpenRouterConfig::with_media_resolver`
to enable image/audio/file content blocks in user messages, then advertise
per-model support with `OpenRouterModelConfig::with_input_images`,
`with_input_audio`, and `with_input_files` (or via the catalog's
`architecture.input_modalities`, applied automatically by
`model_configs_from_catalog_json`). Without a configured resolver, any
media-bearing user message fails closed with `openrouter_request_invalid`.

**Audio caveat**: `OpenRouter` documents audio input only for
`/api/v1/chat/completions` (base64 `input_audio`, not URLs). This crate maps
`ContentBlock::Audio` onto the Responses endpoint's `input_audio` item type by
analogy, but whether `/api/v1/responses` actually accepts `input_audio` is
**unverified** — it has not been confirmed against a live OpenRouter response.
Hosts should enable `with_input_audio` only after confirming the target model
accepts audio input on the Responses endpoint; audio content resolved to a
URL (rather than inline bytes) is always rejected, since `OpenRouter`'s
documented audio input is base64-only.
