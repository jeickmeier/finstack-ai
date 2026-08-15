# Provider authoring

Trusted native model leaves live under `extensions/providers/`. They implement
the public `finstack-ai-runtime` `Model` port only. They do not add a seventh
port, reshape the trait, or publish wire DTOs as runtime contracts.

There are three implementations:

1. `ScriptedModel` in `finstack-ai-test` — semantic reference. No HTTP. Use it
   for reducer-visible text, tool, usage, cancellation, and error shapes.
2. `OpenAiCompatibleProvider` — Chat Completions plus a versioned quirks table.
   Ollama/local is `OpenAiCompatibleConfig::ollama_local`, not a third crate.
   TDD §2 has no `provider-ollama` directory.
3. `AnthropicProvider` — Anthropic Messages (`POST /v1/messages`) leaf. Named
   SSE (`event:` + `data:`), `x-api-key`, and `anthropic-version`.

Required rules:

- Keep wire DTOs crate-private.
- Redact `SecretString`, API keys, and secret headers from `Debug`.
- Fail closed on unknown stream events, oversized frames, and malformed JSON.
- Record keyless loopback fixtures. Live smokes stay `#[ignore]`.
- Advertise capability flags from explicit model config. Do not claim every
  endpoint is identical.
- Do not add a provider router or a WASM/JS Anthropic key path.

See the crate READMEs for endpoint-specific mapping and fixture paths.
