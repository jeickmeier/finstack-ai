# public-rust-api fixtures

Compatibility corpus for public Rust kernel value types (PR-006/PR-007).

Layout:

```text
v1/<kind>/(valid|invalid|roundtrip)--<slug>.json
```

Kinds:

- `typed-id`
- `raw-json`
- `metadata`
- `timestamp`
- `duration`
- `error-descriptor`
- `digest-known-answer`
- `blob-ref`
- `content-block`
- `message`

Boundary fixtures use materialization recipes (`json_string`, `scientific_array`,
`object_members`, …). The Rust runner in `finstack-ai-test` expands recipes,
measures source/canonical sizes, and asserts exact and one-over ceilings.
Python/JavaScript/CBOR projection fields are binding-neutral expected vectors
executed by Rust until live adapters exist.
