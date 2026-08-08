# PR-006 artifacts

Validation evidence for typed identifiers, RawJson/Metadata, digests, time
primitives, stable errors, injectable UUIDv7 generation, and public-rust-api
fixtures.

Implementation tip (PR head): `2d1307cb2426b3dc6f81cbeb8d6a65d76f9edaa4`
Merge commit: `56d7777956df145213b03d2b0b5c1922db42b346` ([#4](https://github.com/jeickmeier/finstack-ai/pull/4)).

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
