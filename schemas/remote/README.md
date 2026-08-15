# remote (candidate-v1)

Owner: `me@jeickmeier.com`  
Compatibility profile: strict inbound / negotiated outbound  
Fixtures: `fixtures/compatibility/remote/v1/`

Inbound remote commands and authentication messages reject unknown fields
and unknown variants before state lookup (TDD §28.4). Framing is shared
with the `process` family (ADR-021); message vocabularies remain distinct
(ADR-014). Source types live in `crates/finstack-ai-protocol/src/remote.rs`.
