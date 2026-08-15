# finstack-ai-provider-anthropic

Trusted native Anthropic Messages provider for the public
`finstack-ai-runtime` `Model` port. It supports bounded named SSE streaming,
text and `tool_use` deltas, prompted structured JSON output, thinking
normalized to `ReasoningDelta`, cache counters on `Usage.extension_counters`,
cancellation, timeouts, and stable secret-safe HTTP failures.

The scripted model in `finstack-ai-test` remains the semantic conformance
reference. This crate is a leaf battery: neither the kernel nor runtime depends
on it. Wire DTOs stay crate-private.

Ordinary tests are deterministic and keyless. The ignored live smoke test
requires an explicitly configured endpoint, model, and credential and is never
part of the default test suite.

Authentication uses `x-api-key` and `anthropic-version`. Credentials and secret
headers require HTTPS. Keyless loopback HTTP is allowed for recorded fixtures.
Redirects are disabled. `Debug` redacts secrets.
