# finstack-ai-context-memory

Reference in-process memory/retrieval `ContextProvider`. Writes stage bytes
through the public `ArtifactStore` / `BlobRef` boundary. Reads return
budgeted, provenance-bearing items matched by exact id or keyword.

This is a pattern, not a vector-database product. There is no embedding
model, ANN index, or network fetch.
