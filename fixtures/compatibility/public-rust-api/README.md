# public-rust-api fixtures

Compatibility corpus for public Rust kernel value types (PR-006–PR-009).

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
- `run-accepted`, `effect-requested`, `record-draft`, `append-request`, `run-event`
- `run-phase`, `kernel-input`, `committed-batch`, `kernel-state`, `pr009-record`

Boundary fixtures use materialization recipes (`json_string`, `scientific_array`,
`object_members`, …). The Rust runner in `finstack-ai-test` expands recipes,
measures source/canonical sizes, and asserts exact and one-over ceilings.
Python/JavaScript/CBOR projection fields are binding-neutral expected vectors
executed by Rust until live adapters exist.

The v1 corpus contains exactly 65 fixtures. PR-009 coverage uses real strict
deserializers and constructors for phase/input vocabulary, batch bounds, state
hash known answers, and record-derived events. The completed-state known answer
comes from the full state reached by the real deterministic reducer trace, not
from a hand-assembled terminal-only state.
