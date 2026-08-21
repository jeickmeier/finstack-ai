# finstack-ai-store-object-s3

S3-compatible `ObjectStore` backend for finstack-ai.

`S3ObjectStore` implements put, conditional put/replace/delete, bounded get,
verified download-to-file, head, scoped paginated listing, delete, and
presigned GET without the AWS SDK. Physical keys contain the full scope
digest. File uploads are copied into a private bounded snapshot before
signing, so the bytes sent cannot diverge from the signed digest.

Listings request at most 1,000 entries, cap response XML at 8 MiB, parse it
structurally, and reject malformed pagination or keys outside the requested
scope. Prefix configuration is fallible through `try_with_key_prefix`.
Presign requests reject zero, fractional-second, or over-policy expiries.

## SigV4

`sigv4::sign_headers` produces the `authorization`, `x-amz-date`, and
`x-amz-content-sha256` headers (plus any passthrough headers) for a signed
request. `sigv4::presign_url` produces a complete presigned URL. Both are
pure functions with no I/O and are validated against AWS's published
known-answer test vectors.
