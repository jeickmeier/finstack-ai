# finstack-ai-embeddings

Shared embedding machinery for finstack-ai: the validated `EmbeddingVector`
type, the batch `TextEmbedder` contract, and the deterministic `HashEmbedder`
reference implementation for tests and offline golden questions.

This crate implements no ports and takes no native or optional dependencies;
it builds for `wasm32-unknown-unknown`. Network-backed embedders live in
sibling crates of `extensions/embeddings/` and depend on this crate, never
the reverse.

Embedder identity discipline: a `TextEmbedderDescriptor::embedder_id` names a
model, revision, and dimensionality. A changed model revision is a new
`embedder_id` — that is, a new embedding space.
