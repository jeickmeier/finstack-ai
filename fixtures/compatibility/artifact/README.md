# artifact fixtures

Historical `finstack-ai-store-artifact` object encodings. The first
versioned tree is `v2/`, matching the `artifacts/v2/{reference-digest}` key
scheme and the `FSAIART2` envelope magic.

```text
fixtures/compatibility/artifact/v2/envelope/
```

`envelope/valid--pinned-owner.bin` is an envelope written before the
ownership sidecar existed: its JSON header still carries `owners` and
`unreferenced_since`. The store must read it unchanged, seed the
`artifacts/owners/v1/` sidecar from that header on first pin, and never
rewrite the envelope bytes. It is generated deterministically by
`artifact_tests::fixture_envelope` and regenerated with
`FINSTACK_WRITE_ARTIFACT_FIXTURES=1 cargo test -p finstack-ai-store-artifact`.
