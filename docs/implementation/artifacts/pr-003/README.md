# PR-003 validation artifacts

Local command evidence plus hosted GitHub Actions evidence for logical PR-003.

Hosted A01/A03 evidence comes from actual PR-003a:
https://github.com/jeickmeier/finstack-ai/pull/1
Head `c2f4e79498c17dcea334d0e67b41289474cd2acd`.

| Artifact | Purpose |
| --- | --- |
| `environment.txt` | Local host/toolchain snapshot |
| `supply-chain.txt` | Local cargo-deny with all-features |
| `secret-scan.txt` / `secret-scan-canary.txt` | Local gitleaks + canaries |
| `release-smoke.txt` / `build-metadata.json` / `release-SHA256SUMS` | Local Darwin release smoke |
| `hosted-ci.txt` | Hosted PR runs for `ci` / `security` / `nightly` (A01) |
| `hosted-release-smoke.txt` | Hosted Linux/macOS/Windows release-smoke (A03) |
| `hosted/release-smoke-*/` | Retained build-metadata + SHA256SUMS per OS |
| `security-review.txt` | Threat Model §18 build/release review |
| `SHA256SUMS` | Digests of the checked-in files above |

Binaries are not retained in git; they remain on the Actions run artifacts (30-day retention) and are identified by the recorded SHA-256 digests.
