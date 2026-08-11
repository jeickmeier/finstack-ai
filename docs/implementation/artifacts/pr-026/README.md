# PR-026 candidate and integration evidence

PR-026 implements the Rust native developer-preview candidate at immutable
commit `b1bddac82041d1780651d9cee4d2378dba379ee5` and tree
`ce5156cdd5f77a58585d2f3421fec2d5af8155b8`. The evidence commit
`08578a26240c4161ebb3da98285a632bab92f4af` was locally merged to `main` as
`93b7959b1edbe1e541c2449dcbcc78723bf4504b`; focused and aggregate validation
passed on that merge. The required Linux and Windows executions of
`mise run test-pr026` have not run, so PR-026 remains `Blocked` and G3 remains
`Not ready`.

## Candidate acceptance map

| Acceptance | Candidate evidence | Disposition |
| --- | --- | --- |
| A01 | Isolated `fixtures/ci/native-user` project resolves the SDK, provider, and calculator packages and completes the offline tool loop with result `five` | Passed |
| A02 | Model-only and tool-loop examples pass locally on macOS; the checked workflow invokes the same task on Linux/macOS/Windows, but Linux and Windows executions are absent | Pending |
| A03 | Four versioned warning-only native thresholds pass; exact measurements are retained in `performance-report.json` | Passed |
| A04 | Seven verified Cargo `.crate` archives, binary, docs, reports, file lists, release notes, and checksums are staged twice with identical package hashes | Passed |
| A05 | G3 readiness review found A02 and integration/phase-exit evidence incomplete; the execution envelope prohibits minting the named decision before those prerequisites pass | Pending |

Exact commands and scope limits are recorded in
[`candidate-validation.txt`](candidate-validation.txt), security controls in
[`security-review.txt`](security-review.txt), release hashes in
[`reproducibility.txt`](reproducibility.txt), and the gate readiness result in
[`g3-readiness-review.txt`](g3-readiness-review.txt). The local merge identity
and post-merge checks are recorded in
[`integration-validation.txt`](integration-validation.txt).

No push, hosted run, actual pull request, publication, live provider call,
Linux/Windows execution, or independent review is claimed.
