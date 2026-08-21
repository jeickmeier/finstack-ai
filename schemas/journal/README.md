# journal

Owner: `me@jeickmeier.com`  
Compatibility profile: durable ignorable optionals / fatal state-bearing unknowns  
Fixtures: `fixtures/compatibility/journal/`

 activates the family with the ADR-015 canonical-CBOR profile and
JournalStore v1 identity/load rules. Diagnostic JSON/JSONL is a lossless
projection only; it is not the journal encoding. Typed `CostAmount.micros`
stay canonical decimal strings.

```text
schemas/journal/v1/fixture.schema.json
```
