# PR-002 local validation artifacts

Captured on branch `pr-002-architecture-enforcement` before merge. These prove
`mise run architecture` locally. Hosted CI attachment is PR-003. Treat digests as
provisional until bound to an immutable merge commit in the evidence register.

Compile-time six-port / native-vs-WASM port-bound fixtures were intentionally
omitted; those proofs wait for production port traits in PR-014–PR-018.

| Artifact | Purpose |
| --- | --- |
| `environment.txt` | Tool versions |
| `architecture.txt` | Full `mise run architecture` output + `/usr/bin/time -p` |
| `unit-tests.txt` | Architecture checker unit tests |
| `wasm-binding.txt` | Production `finstack-ai-wasm` target check |
| `dependency-direction.txt` | Workspace edge inventory |
| `kernel-deps.txt` | Kernel direct dependency inventory |
| `fixture-inventory.txt` | Checked-in fixture paths |
| `cargo-metadata.json` | Locked workspace metadata snapshot |
| `SHA256SUMS` | Digests of the files above |

Observed wall-clock for `mise run architecture` is recorded in `architecture.txt`
(limit: < 60s).
