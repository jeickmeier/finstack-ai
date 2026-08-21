# finstack-ai-store-artifact-object

`ArtifactStore` adapter over a host-supplied `ObjectStore`.

Bridges the application-level `ArtifactStore` contract onto the
infrastructure-level `ObjectStore` port so hosts can back artifacts with the
same object storage backend (local filesystem, S3, ...) used for other
unstructured content, instead of the small in-process default.

Artifacts are stored at `artifacts/v2/{reference-digest}`
within an `ObjectScope` derived from the caller's `ArtifactScope`
(`session_id` is always `Some`). A versioned envelope preserves the complete
`ArtifactRef`, owners, lifecycle metadata, and content. Conditional object
operations prevent concurrent pin/unpin/collection updates from losing state.

Default artifact byte ceiling is 64 MiB, clamped to the backing object
store's own `ObjectStoreLimits::max_object_bytes`; override with
`ObjectArtifactStore::with_max_artifact_bytes`.

`get` validates scope before I/O, verifies the stored reference and content,
and rejects a backing `ObjectRef` whose key, scope, digest, length, or media
type was rewritten.

Object errors map onto `ArtifactError` as follows:

| `ObjectError`                   | `ArtifactError`  |
| -------------------------------- | ----------------- |
| `NotFound`                       | `NotFound`         |
| `TooLarge`                       | `TooLarge`         |
| `ScopeMismatch`                  | `ScopeMismatch`    |
| `Integrity`                      | `Integrity`        |
| `InvalidKey` / `InvalidMetadata` | `InvalidMetadata`  |
| `Unavailable` / `Io` / `Unsupported` | `Unavailable`  |
