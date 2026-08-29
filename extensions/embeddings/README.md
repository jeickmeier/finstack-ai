# Embedding leaves

Embedding primitives and embedder implementations. Trusted native code, the
same class as the rest of `extensions/`.

| Crate | Role |
| --- | --- |
| `finstack-ai-embeddings` | Shared embedding machinery: validated `EmbeddingVector`, the `TextEmbedder` contract, and the deterministic `HashEmbedder` reference implementation |

`finstack-ai-embeddings` stays dependency-free beyond the runtime port
bounds and builds for `wasm32-unknown-unknown`; it implements no ports.
Network-backed embedder implementations live in sibling crates of this
family and depend on it, never the reverse.
