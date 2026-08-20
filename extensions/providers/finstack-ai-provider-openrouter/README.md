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
    .expect("authentication")
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

`with_authentication` returns `Result`. It fails when the reserved
`default` credential name is rejected.

`Agent::openrouter` and the Python/WASM linked factories construct this
provider without a `MediaResolver`. Vision, file, and audio **input**
require a host-built provider with `with_media_resolver`. ADR-049
rejected FFI resolvers on those factories.

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

Image and file input follow `architecture.input_modalities`. OpenRouter
Responses `input_audio` is unverified and opt-in: catalog fetch never
enables audio, and the default stays off. Hosts opt in only with
`OpenRouterModelConfig::with_input_audio(true)` after confirming the
target model.

## Media input (images, audio, files)

Attach a host-supplied `MediaResolver` via `OpenRouterConfig::with_media_resolver`
to enable image/audio/file content blocks in user messages, then advertise
and enforce per-model support with `OpenRouterModelConfig::with_input_images`,
`with_input_audio`, and `with_input_files`. Catalog fetch can set the
image and file flags from `architecture.input_modalities`; it does not
enable audio. These flags are gated at draft translation: a media block
whose modality flag is off is rejected with `openrouter_request_invalid`,
even after it resolved successfully. Without a configured resolver, any
media-bearing user message also fails closed with
`openrouter_request_invalid`.

**Audio caveat**: OpenRouter Responses `input_audio` is **unverified and
opt-in**. It is off by default and is never enabled by catalog fetch.
`OpenRouter` documents audio input only for `/api/v1/chat/completions`
(base64 `input_audio`, not URLs). This crate maps `ContentBlock::Audio`
onto the Responses `input_audio` item type by analogy; whether
`POST /api/v1/responses` actually accepts that item has not been confirmed
against a live OpenRouter response. Hosts enable it only with
`OpenRouterModelConfig::with_input_audio(true)` after confirming the
target model. Audio resolved to a URL (rather than inline bytes) is
always rejected, since documented OpenRouter audio input is base64-only.

## Media generation

For outbound image/speech/transcription/video generation tools (as opposed
to the media *input* above), see the `finstack-ai-tools-openrouter-media`
and `finstack-ai-tools-openai-media` toolset crates, registrable from any
linked constructor including `Agent::openrouter`.
