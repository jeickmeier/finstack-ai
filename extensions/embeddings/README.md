# Embedding leaves

Embedding primitives and embedder implementations. Trusted native code, the
same class as the rest of `extensions/`.

| Crate | Role |
| --- | --- |
| `finstack-ai-embeddings` | Shared embedding machinery: validated `EmbeddingVector`, the `TextEmbedder` contract, and the deterministic `HashEmbedder` reference implementation |
| `finstack-ai-embedder-ollama` | Native `TextEmbedder` over Ollama `POST /api/embed` |

`finstack-ai-embeddings` stays dependency-free beyond the runtime port
bounds and builds for `wasm32-unknown-unknown`; it implements no ports.
Network-backed embedder implementations live in sibling crates of this
family and depend on it, never the reverse.

`finstack-ai-embedder-ollama` mirrors the Ollama provider's HTTP
conventions (own `reqwest` client, redirects disabled, `vendored-tls`
passthrough) instead of `finstack-ai-net-guard`, whose loopback/private-IP
vetting is the wrong default for an operator-configured localhost daemon.
Configuring it is an application's explicit egress decision; no embedder is
ever a default.
