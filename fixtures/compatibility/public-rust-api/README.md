# public-rust-api fixtures

Compatibility corpus for public Rust kernel and runtime value types (–).

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
- `run-phase`, `kernel-input`, `committed-batch`, `kernel-state`
- `operation-locator`, `external-effect-completion-command`
- `interaction-resolution-command`, `external-command-rejected`
- `pr009-record`, `pr010-record`, `pr011-record`, `pr012-record`, `pr014-record`
- `corrupt-replay`
- `model-request-draft`, `model-context-profile`, `tool-spec`

Boundary fixtures use materialization recipes (`json_string`, `scientific_array`,
`object_members`, …). The Rust runner in `finstack-ai-test` expands recipes,
measures source/canonical sizes, and asserts exact and one-over ceilings.
Python/JavaScript/CBOR projection fields are binding-neutral expected vectors
executed by Rust until live adapters exist.

Corpus size and required subjects are asserted by executable tests. Reducer
coverage uses real strict deserializers and constructors for phase/input
vocabulary, batch bounds, state-hash known answers, record-derived events, and
atomic rejection of declared corrupt-replay mutations. The completed-state
known answer comes from the full state reached by the real deterministic reducer
trace, not from a hand-assembled terminal-only state.
