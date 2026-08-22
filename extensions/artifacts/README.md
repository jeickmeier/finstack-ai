# Artifact leaves

| Crate | Role |
| --- | --- |
| `finstack-ai-store-artifact` | `ArtifactStore` over local or S3-compatible blob storage (`local` / `s3` features; 64 MiB default ceiling) |

Applications inject this crate. It is not the Agent, Python, or WASM default.
