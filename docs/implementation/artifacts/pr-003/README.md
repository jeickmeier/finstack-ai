# PR-003 validation artifacts

Local command evidence for branch `pr-003-ci-release-matrix` on Darwin arm64.
Hosted Linux/macOS/Windows runs remain blocked by the absence of a Git remote
(`PR-003-B-no-remote-ede93913b2ea`).

Review-fix refresh covers secret-scan lockfile coverage, cargo-deny
`all-features`, pinned Python build backend, release metadata derivation, hosted
`lint-workflows`, reservation cadences, and removal of the green reserved-fuzz
placeholder job.

| Artifact | Purpose |
| --- | --- |
| `environment.txt` | Host/toolchain snapshot |
| `supply-chain.txt` | cargo-deny with all-features (Eng §9 / TM-18) |
| `secret-scan.txt` | Clean repository gitleaks scan |
| `secret-scan-canary.txt` | Temporary examples/fixtures/lockfile negatives |
| `release-smoke.txt` | Private facade-dependent release binary (Darwin) |
| `release-SHA256SUMS` / `build-metadata.json` | Release artifact digests/metadata |
| `test-ci.txt` | CI/security helper unit tests |
| `lint-workflows.txt` | `actionlint` |
| `build-python.txt` | Python package smoke with hashed setuptools pin |
| `security-review.txt` | Threat Model §18 build/release review |
| `SHA256SUMS` | Digests of the files above |

Validation limits:
- Rust crate unit tests currently pass vacuously (placeholder crates).
- `release-reproducible` is Linux-only and was skipped on Darwin.
- Hosted workflow run URLs are unavailable until a remote/PR exists.
