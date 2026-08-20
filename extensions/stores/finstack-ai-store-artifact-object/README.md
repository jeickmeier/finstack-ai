# finstack-ai-store-artifact-object

`ArtifactStore` adapter over a host-supplied `ObjectStore`.

Bridges the application-level `ArtifactStore` contract onto the
infrastructure-level `ObjectStore` port so hosts can back artifacts with the
same object storage backend (local filesystem, S3, ...) used for other
unstructured content, instead of the small in-process default.

Artifacts are stored at the logical key `artifacts/{content-digest-hex}`
within an `ObjectScope` derived from the caller's `ArtifactScope`
(`session_id` is always `Some`). The artifact `kind` and its
non-authoritative `attributes` are stored verbatim in the object's
`ObjectMetadata::attributes`.

Default artifact byte ceiling is 64 MiB, clamped to the backing object
store's own `ObjectStoreLimits::max_object_bytes`; override with
`ObjectArtifactStore::with_max_artifact_bytes`.

`get` recomputes the object key from the artifact's content digest and
re-verifies the returned bytes hash to that digest before returning them.

Object errors map onto `ArtifactError` as follows:

| `ObjectError`                   | `ArtifactError`  |
| -------------------------------- | ----------------- |
| `NotFound`                       | `NotFound`         |
| `TooLarge`                       | `TooLarge`         |
| `ScopeMismatch`                  | `ScopeMismatch`    |
| `Integrity`                      | `Integrity`        |
| `InvalidKey` / `InvalidMetadata` | `InvalidMetadata`  |
| `Unavailable` / `Io` / `Unsupported` | `Unavailable`  |
