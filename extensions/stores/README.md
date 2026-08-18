# Store leaves

| Crate | Role |
| --- | --- |
| `finstack-ai-store-memory` | Explicitly non-durable in-memory journal |
| `finstack-ai-store-sqlite` | Durable local SQLite journal; WAL + `synchronous=FULL` |

Applications inject these crates. They are not the Agent, Python, or
WASM default.
