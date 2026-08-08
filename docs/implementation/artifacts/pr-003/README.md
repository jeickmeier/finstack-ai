# PR-003 validation artifacts

Bound to commit `92217776d215c5658910b60397d0e22960466218` on branch
`pr-003-ci-release-matrix`. Local command evidence was captured on Darwin arm64
before hosted CI attachment. Hosted Linux/macOS/Windows runs are
blocked by the absence of a Git remote (`PR-003-B-no-remote-ede93913b2ea`).

| Artifact | Purpose |
| --- | --- |
| `environment.txt` | Host/toolchain snapshot |
| `doctor.txt` | `mise run doctor` |
| `format.txt` / `clippy.txt` / `test.txt` / `docs.txt` | Rust merge gates |
| `check-minimal.txt` / `architecture.txt` | Feature/architecture gates |
| `lint-workflows.txt` | `actionlint` |
| `supply-chain.txt` | cargo-deny (Eng §9 / TM-18) |
| `secret-scan.txt` | Clean repository gitleaks scan |
| `secret-scan-canary.txt` | Temporary examples/fixtures negatives |
| `release-smoke.txt` | Private facade-dependent release binary (Darwin) |
| `release-SHA256SUMS` / `build-metadata.json` | Release artifact digests/metadata |
| `test-ci.txt` | CI/security helper unit tests |
| `build-python.txt` | Current Python package smoke |
| `security-review.txt` | Threat Model §18 build/release review |
| `SHA256SUMS` | Digests of the files above |

Validation limits:
- Rust crate unit tests currently pass vacuously (placeholder crates).
- `release-reproducible` is Linux-only and was skipped on Darwin.
- Hosted workflow run URLs are unavailable until a remote/PR exists.
