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

## Ownership sidecar and blob index

Artifact lifecycle state lives beside the envelope, not inside it. Each
envelope has an owner sidecar at `artifacts/owners/v1/{reference-digest}`
(canonical JSON: `schema`, `owners`, `unreferenced_since` in Unix
milliseconds or `null`, the envelope's `envelope_digest`, and the GC
`collecting_since` mark) and a blob index entry at
`artifacts/by-blob/v1/{digest}` keyed by the digest of the canonical
`BlobRef`, so `get_by_blob` is one small read plus one envelope read. Pin
and unpin are read-modify-write on the small sidecar; envelope bytes are
never rewritten, so a 64 MiB artifact is no longer downloaded and uploaded
to change its owners.

Migration is lazy and needs no tooling: an envelope written before the
sidecar existed keeps its header `owners` and `unreferenced_since`, and the
sidecar is seeded from that header the first time the artifact is pinned,
unpinned, or examined by GC. Once a sidecar exists it is authoritative and
the header copy is stale but harmless. Pre-index envelopes are found by the
bounded listing scan, which writes the index entry on a hit. Both files sit
outside the `artifacts/v2/` prefix, so GC never lists them, and an entry
left dangling by a crash is cleaned up by the next lookup, pin, or pass.

Garbage collection commits to a deletion by marking the sidecar before it
removes the envelope, then the sidecar, then the index entry. A pin racing
that decision either wins the digest precondition or observes the mark and
fails with `artifact_not_found`; a mark left by an interrupted pass is lifted
after one orphan grace and cleared immediately by a fresh `stage_put`.

Garbage collection saves a scope-local cursor at `artifacts/gc-cursor/v1`,
outside the artifact listing prefix. Bounded calls continue through pinned
prefixes, including after reopening the adapter. The cursor is a scan hint;
reference validation, conditional deletion, and the orphan grace remain authoritative.

The local backend uses OS-held exclusive locks on permanent `.lock` files.
Process termination releases ownership automatically; the file's presence does
not mean it is locked. Do not remove these files while any store is running.
Stop older store processes before upgrading because the former lock-file
presence protocol cannot coordinate with the new advisory locks. A mutation
that finds the lock held reports `artifact_unavailable` with message
`artifact_busy`; it is retryable and says nothing about whether the artifact
exists. Local listings page in physical-filename (key-hash) order and only
open the envelopes on the returned page; a GC cursor saved by an earlier
version restarts the scan from the beginning once.

The S3 store clamps the driver's single-object ceiling to the artifact
ceiling plus 1 MiB of header slack, so a GET buffers at most one artifact
rather than the 5 GiB protocol maximum. Supported backends honour
`If-None-Match: *` and `If-Match` with `412`; constructors never probe the
network, so an endpoint that ignores preconditions surfaces as conflicts at
use.
