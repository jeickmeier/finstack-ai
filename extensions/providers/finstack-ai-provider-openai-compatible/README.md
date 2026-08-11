# finstack-ai-provider-openai-compatible

Trusted native OpenAI-compatible Chat Completions provider for the public
`finstack-ai-runtime` `Model` port. It supports bounded SSE streaming, text and
tool-call deltas, structured JSON output, cumulative usage, cancellation,
timeouts, and stable secret-safe HTTP failures.

The scripted model in `finstack-ai-test` remains the semantic conformance
reference. This crate is a leaf battery: neither the kernel nor runtime depends
on it.

Ordinary tests are deterministic and keyless. The ignored live smoke test
requires an explicitly configured endpoint, model, and credential and is never
part of the default test suite.
