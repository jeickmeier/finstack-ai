# Store leaves

| Crate | Role |
| --- | --- |
| `finstack-ai-store-memory` | Explicitly non-durable in-memory journal |
| `finstack-ai-store-sqlite` | Durable local SQLite journal; WAL + `synchronous=FULL` |
| `finstack-ai-store-postgres` | Durable multi-writer PostgreSQL journal with rustls and bounded operations |
| `finstack-ai-store-object-s3` | `ObjectStore` backed by S3-compatible storage (AWS S3, MinIO, Garage); hand-rolled SigV4 signing |
| `finstack-ai-store-object-local` | `ObjectStore` backed by a local filesystem root; `presign_get` unsupported |
| `finstack-ai-store-artifact-object` | `ArtifactStore` adapter over any `Arc<dyn ObjectStore>`; 64 MiB default ceiling |

Applications inject these crates. They are not the Agent, Python, or
WASM default.
