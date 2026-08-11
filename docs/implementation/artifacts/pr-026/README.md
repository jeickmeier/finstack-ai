# PR-026 native developer-preview evidence

PR-026 implements the Rust native developer-preview candidate initially at
`b1bddac82041d1780651d9cee4d2378dba379ee5`, locally merged to `main` as
`93b7959b1edbe1e541c2449dcbcc78723bf4504b`. Hosted portability remediation is
complete at `af6b93d3f10362ab0399c5de18affe4563740da7`; the exact-revision CI and
security workflows pass, all four Phase 3 exits pass, and the separate G3
decision is recorded. PR-026 and Phase 3 are `Done`; G3 is `Passed`.

## Candidate acceptance map

| Acceptance | Candidate evidence | Disposition |
| --- | --- | --- |
| A01 | Isolated `fixtures/ci/native-user` project resolves the SDK, provider, and calculator packages and completes the offline tool loop with result `five` | Passed |
| A02 | Exact Linux, macOS, and Windows Rust jobs complete the Native developer preview step and the full CI workflow passes at `af6b93d3f10362ab0399c5de18affe4563740da7` | Passed |
| A03 | Four versioned warning-only native thresholds pass; exact measurements are retained in `performance-report.json` | Passed |
| A04 | Seven verified Cargo `.crate` archives, binary, docs, reports, file lists, release notes, and checksums are staged twice with identical package hashes | Passed |
| A05 | All four Phase 3 exits pass and delegated decision `G3-D-native-preview-14a386c7db24` records G3 as Passed | Passed |

Exact commands and scope limits are recorded in
[`candidate-validation.txt`](candidate-validation.txt), security controls in
[`security-review.txt`](security-review.txt), release hashes in
[`reproducibility.txt`](reproducibility.txt), and the gate readiness result in
[`g3-readiness-review.txt`](g3-readiness-review.txt). The local merge identity
and post-merge checks are recorded in
[`integration-validation.txt`](integration-validation.txt). Exact hosted job
identities and the remediation trail are in
[`hosted-validation.txt`](hosted-validation.txt), Phase 3 exit dispositions in
[`phase3-exit-review.txt`](phase3-exit-review.txt), and the passing gate review
in [`g3-decision.txt`](g3-decision.txt).

The integrated `main` revision was pushed and hosted CI/security ran. No actual
pull request or hosted merge, package publication, live provider call, or
independent review is claimed.
