# finstack-ai-provider-openrouter

OpenRouter Responses provider for finstack-ai. See Task 10 for full docs.

## Media input (images, audio, files)

Attach a host-supplied `MediaResolver` via `OpenRouterConfig::with_media_resolver`
to enable image/audio/file content blocks in user messages, then advertise
per-model support with `OpenRouterModelConfig::with_input_images`,
`with_input_audio`, and `with_input_files` (or via the catalog's
`architecture.input_modalities`). Without a configured resolver, any
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
