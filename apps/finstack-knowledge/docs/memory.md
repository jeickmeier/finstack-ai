# Memory and unified recall

The memory extension stores scoped facts, exposes remember/correct/forget tools,
and captures facts from committed runs. Standalone users can also register its
memory search tool and recall provider. Correction, forgetting and expiry invalidate
derived vectors; unavailable or incomplete embeddings remain visible in coverage.

The knowledge application shares this store across sessions and uses one global
`search` tool plus global recall across memory, documents and authorized committed
journals. It disables memory's separate read tool and recall provider while keeping
write/manage tools and capture. Retrieved content is untrusted data, never authority.

Lexical retrieval is the offline default. An explicitly configured embedder enables
exact vector retrieval for memory and documents. A configured graph vocabulary adds
evidence-backed entity/relationship queries and opt-in expansion. Host maintenance
runs between settled turns and reports failures and remaining work. Python and CLI
share persisted sources when configured with the same directory and tenant.
