# PR-009 artifacts

Validation evidence for the model-only kernel reducer, PR-009 records/events,
golden-trace / public-rust-api fixtures, and pack v0.16 reducer contract amendment.

Integrated on `main`: `5843dce6d77498a75acdc15d816586cb26098456`

## Acceptance mapping

| Criterion | Proof |
| --- | --- |
| A01 | `test-kernel.log` / `merge-test-kernel.txt` + `conformance.log` / `merge-conformance.txt` model-only golden traces and apply-only replay with stable state hashes |
| A02 | `test-kernel.log` / `merge-test-kernel.txt` decide purity + `ExecuteEffect` sibling-request proofs |
| A03 | `test-kernel.log` / `merge-test-kernel.txt` phase/input matrix and stable `KernelError` codes without panic |
| A04 | `conformance.log` / `merge-conformance.txt` + reducer tests: model chunk splits do not change durable batches/state |
| A05 | `conformance.log` / `merge-conformance.txt` deferred model completion and `before_finalize` continuation replay identically |

Post-integration verification: `merge-test-kernel.txt` + `merge-conformance.txt` + `merge-check-wasm.txt`.

## Security / supply chain

| Record | Artifact |
| --- | --- |
| TM-15 / TM-16 / §18 / SEC-INV review | `security-review.txt` |
| Local CI aggregate (pre-integration candidate, then bound) | `ci.log` |
| Remediation command summary | `remediation-validation.txt` |

## Commands

See individual `*.log` / `*.txt` logs and root `SHA256SUMS`.
