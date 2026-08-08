# PR-002 validation artifacts

Bound to merge commit `ee9754fe2d0f015181dcefa97e715392aadd28ed` on `main`
(local merge of `pr-002-architecture-enforcement`; no GitHub remote/PR available).
Hosted CI attachment of `mise run architecture` is PR-003.

Compile-time six-port / native-vs-WASM port-bound fixtures were intentionally
omitted from PR-002; those proofs wait for production port traits in PR-014–PR-018.

| Artifact | Purpose |
| --- | --- |
| `environment.txt` | Tool versions and reviewed commit |
| `architecture.txt` | Full `mise run architecture` output + `/usr/bin/time -p` |
| `unit-tests.txt` | Architecture checker unit tests |
| `wasm-binding.txt` | Production `finstack-ai-wasm` target check |
| `dependency-direction.txt` | Workspace edge inventory |
| `kernel-deps.txt` | Kernel direct dependency inventory |
| `fixture-inventory.txt` | Checked-in fixture paths |
| `cargo-metadata.json` | Locked workspace metadata snapshot |
| `SHA256SUMS` | Digests of the files above |

Observed wall-clock for `mise run architecture`: **real 0.32s** (limit: < 60s).
