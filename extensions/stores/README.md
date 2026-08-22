# Store leaves

| Crate | Role |
| --- | --- |
| `finstack-ai-store-common` | Shared journal-store semantics used by the journal backends |
| `finstack-ai-store-memory` | Explicitly non-durable in-memory journal |
| `finstack-ai-store-sqlite` | Durable local SQLite journal; WAL + `synchronous=FULL` |
| `finstack-ai-store-postgres` | Durable multi-writer PostgreSQL journal with rustls and bounded operations |

Applications inject the journal crates. They are not the Agent, Python, or
WASM default. `finstack-ai-store-common` is a shared helper, not an injectable
store. Artifact storage lives under `extensions/artifacts/`.
