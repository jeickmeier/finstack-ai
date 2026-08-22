# finstack-ai-store-artifact

`ArtifactStore` backed by S3-compatible or local blob storage.

Artifact storage is the public storage concept. This crate owns the artifact
algorithm — the `artifacts/v2/{reference-digest}` key scheme, the versioned
envelope that preserves the complete `ArtifactRef` beside the bytes, and the
pin / orphan-collection protocol — written once over a private blob driver.

Enable the `s3` feature for `S3ArtifactStore` and the `local` feature for
`LocalArtifactStore`. Neither feature is on by default. The default artifact
ceiling is `DEFAULT_MAX_ARTIFACT_BYTES` (64 MiB).

Applications inject this crate. It is not the Agent, Python, or WASM default.

```rust
// finstack-ai-store-artifact with feature "local"
use std::path::PathBuf;

use finstack_ai_store_artifact::LocalArtifactStore;

let store = LocalArtifactStore::try_new(PathBuf::from("/tmp/artifacts"))
    .expect("local artifact store");
```

```rust
// finstack-ai-store-artifact with feature "s3"
use finstack_ai_store_artifact::{S3ArtifactStore, S3ObjectStoreConfig};

let config = S3ObjectStoreConfig::try_new("http://127.0.0.1:9000", "artifacts", "garage")
    .expect("s3 config");
let store = S3ArtifactStore::try_new(config).expect("s3 artifact store");
```
