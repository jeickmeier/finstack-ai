# PR-007 artifacts

Validation evidence for content blocks, messages, blob references, public-rust-api
fixtures, and pack v0.14 message-contract amendment.

Branch: `pr-007-content-messages-final`
Implementation tip: `4a0a77f668adcfa2679dc161779c4456d721c0c2`

## Acceptance mapping

| Criterion | Proof |
| --- | --- |
| A01 | `test-kernel.txt` + `conformance.txt` 38-fixture corpus with frozen `serialized_json`; `check-wasm.txt` type-checks `finstack-ai-kernel` for `wasm32-unknown-unknown` |
| A02 | `test-kernel.txt` + `conformance.txt` invalid tool-association / role-block fixtures |
| A03 | `conformance.txt` blob-ref large-reference-only fixture (`max_serialized_bytes`) and media/file message round-trips |
| A04 | `docs.txt` rustdoc examples; `schema-governance.txt` + public-rust-api fixtures |

## Security / supply chain

| Record | Artifact |
| --- | --- |
| TM-16 / TM-20 / §18 review | `security-review.txt` |
| Dependency inventory | `dependency-inventory.txt` |
| cargo-deny | `supply-chain.txt` |

## Commands

See individual `*.txt` logs and root `SHA256SUMS`.
