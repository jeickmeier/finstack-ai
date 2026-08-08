# PR-006 artifacts

Validation evidence for typed identifiers, RawJson/Metadata, digests, time
primitives, stable errors, injectable UUIDv7 generation, and public-rust-api
fixtures.

Implementation tip (local A01–A05 evidence): `6704134d1701a3271f6c1a351e409c650b8eb8bc`
on branch `pr-006-kernel-value-types`.
PR head (hosted CI): `abf44b7e8e02c700cbfd70343b8999b18e1af7ea` ([#4](https://github.com/jeickmeier/finstack-ai/pull/4)).
Merge commit fields remain post-merge.

## Acceptance mapping

| Criterion | Proof |
| --- | --- |
| A01 | `test-kernel.txt` + `conformance.txt` / public-rust-api typed-id + error fixtures |
| A02 | `conformance.txt` raw-json/metadata/digest fixtures (exact/one-over materialization) |
| A03 | `conformance.txt` timestamp/duration cross-language projection vectors (Rust-executed) |
| A04 | `conformance.txt` error-descriptor round-trip; runtime FrameworkError strips source |
| A05 | `architecture.txt` + `check-minimal.txt` + `check-wasm.txt` (kernel free of clocks/RNG/async) |

## Security / supply chain

| Record | Artifact |
| --- | --- |
| TM-16 / §18 review | `security-review.txt` |
| Dependency inventory | `dependency-inventory.txt` |
| cargo-deny | `supply-chain.txt` |

## Commands

See individual `*.txt` logs and root `SHA256SUMS`.
