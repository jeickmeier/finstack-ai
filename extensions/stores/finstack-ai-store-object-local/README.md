# finstack-ai-store-object-local

Local-filesystem `ObjectStore` backend for finstack-ai.

Stores each object as one self-describing envelope beneath the full scope
digest. A domain-separated hash of the logical key names the file; the
envelope retains the original key, scope, content digest, length, media type,
and payload. Replacements sync a complete temporary envelope before an atomic
publish and directory sync, so concurrent readers observe a complete old or
new object. Verified downloads use a temporary destination and never
overwrite the caller's file on integrity failure.

The previous truncated-scope/two-file layout is not read or migrated; local
development roots created by older builds must be recreated.

Does not support `presign_get` (returns `ObjectError::Unsupported`); intended
for local development and tests, not production object storage.
