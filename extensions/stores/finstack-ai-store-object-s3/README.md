# finstack-ai-store-object-s3

S3-compatible `ObjectStore` backend for finstack-ai.

This crate currently ships the configuration surface (`S3ObjectStoreConfig`,
`Addressing`) and a hand-rolled AWS SigV4 signer (`sigv4`) used to build
signed requests and presigned URLs without a full AWS SDK dependency. The
`S3ObjectStore` implementation of the `ObjectStore` trait lands in a later
change.

## SigV4

`sigv4::sign_headers` produces the `authorization`, `x-amz-date`, and
`x-amz-content-sha256` headers (plus any passthrough headers) for a signed
request. `sigv4::presign_url` produces a complete presigned URL. Both are
pure functions with no I/O and are validated against AWS's published
known-answer test vectors.
