# PR-008 artifacts

Validation evidence for runtime events, journal drafts/envelopes, effect and
interaction envelopes, shared refs/limits/Usage, and public-rust-api fixtures.

Branch: `pr-008-contract-amendment`
GitHub PR: [#5](https://github.com/jeickmeier/finstack-ai/pull/5)
Implementation tip: `7b5ededc2054b0b7dde8373efbe0ec95ba02b71b`

## Acceptance mapping

| Criterion | Proof |
| --- | --- |
| A01 | `test-kernel.txt` + `conformance.txt` class-safe `RunEvent` constructors; durable/transient fixtures |
| A02 | `test-kernel.txt` + `effect-requested` fixtures (`EffectId` + `input_digest`) |
| A03 | Format/kind versions on drafts/events; payload-digest/CBOR deferred to PR-039 |
| A04 | Derived-event ordinal mapping + `derived_event_ids` cardinality fixtures |
| A05 | `run-accepted` root/child lineage fixtures; interaction schema (no approval records) |
| A06 | Child attenuation fixtures (`construct_child`) for security/deadline/limits |
| A07 | Deterministic constructors under injected time/ids (kernel unit + fixture runner) |
| A08 | Schema fixture diffs + exact/one-over append-batch ceiling fixtures |

## Security / supply chain

| Record | Artifact |
| --- | --- |
| TM-15 / TM-16 / §18 review | `security-review.txt` |
| Dependency inventory | `dependency-inventory.txt` |
| cargo-deny | `supply-chain.txt` |

## Commands

See individual `*.txt` logs and root `SHA256SUMS`.
