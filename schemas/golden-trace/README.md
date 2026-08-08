# golden-trace

Owner: `me@jeickmeier.com`  
Compatibility profile: strict reject-unknown (TDD §28.4)  
Fixtures: `fixtures/compatibility/golden-trace/`

JSON Schema 2020-12 documents for scripted inputs and golden traces:

```text
schemas/golden-trace/v1/scripted-input.schema.json
schemas/golden-trace/v1/trace.schema.json
```

Payload declaration ceilings mirror Technical Design §6.5. The Phase 0
harness validates fixtures and compares opaque expected values; it does not
implement Phase 1 semantic hashing or ADR-016 JCS.
