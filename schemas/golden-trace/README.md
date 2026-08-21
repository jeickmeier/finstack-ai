# golden-trace

Owner: `me@jeickmeier.com`  
Compatibility profile: strict reject-unknown (contract section 28.4)  
Fixtures: `fixtures/compatibility/golden-trace/`

JSON Schema 2020-12 documents for scripted inputs and golden traces:

```text
schemas/golden-trace/v1/scripted-input.schema.json
schemas/golden-trace/v1/trace.schema.json
schemas/golden-trace/v1/test-kit.schema.json
```

Payload declaration ceilings mirror Technical Design §6.5. Format v1 retains
the original scripted fields and adds strict model-only completion, deferral,
external-completion (required `text`, including empty text), and
`before_finalize` continuation steps.
The native reducer adapter obtains semantic state hashes from
`KernelState::state_hash`; fixture normalization itself remains sorted-key JSON
comparison and does not implement semantic JCS.
