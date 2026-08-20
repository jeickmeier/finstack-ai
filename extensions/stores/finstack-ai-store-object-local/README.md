# finstack-ai-store-object-local

Local-filesystem `ObjectStore` backend for finstack-ai.

Stores each object's content at `{root}/{physical_object_key(None, scope, key)}`
and a JSON sidecar carrying scope digest, content digest, length, and media
type at the same path plus `.meta.json`. Writes land via
`tempfile::NamedTempFile` and are published by renaming the sidecar and then
the content into place, so a crash mid-write leaves either nothing or an
orphan sidecar (treated as `NotFound`) — never a partially-written object.

Does not support `presign_get` (returns `ObjectError::Unsupported`); intended
for local development and tests, not production object storage.
