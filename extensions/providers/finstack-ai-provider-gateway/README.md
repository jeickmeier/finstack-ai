# finstack-ai-provider-gateway

Config-driven `finstack_ai_runtime::Model` adapter over a named wire protocol:

`openai_responses` | `openai_chat` | `anthropic_messages` | `ollama_chat`

A route fails at construction if any required model field is missing. Context
windows are never inferred. `route.auth` is a credential reference; the
factory never reads environment variables. An unresolved reference fails the
request. Non-loopback endpoints require HTTPS. Secrets pass only through the
redacted `Authentication` wrapper.

This crate is a T1 native adapter. It is not isolated. Official OpenAI,
Anthropic, and Ollama crates remain the reference implementations.
