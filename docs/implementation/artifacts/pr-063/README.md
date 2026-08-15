# PR-063 artifacts

Performance, memory, and startup budget ratification: NFR-PERF-001
through NFR-PERF-007, size targets, idle/active session profiles.
Admitted 2026-08-15 on `codex/pr-063-perf-memory-startup-budgets`
from local `main` at `dd3557d8742805d003023f9fd8bf628672f3e33f`.

| File | Owns |
| --- | --- |
| `plan.md` | Execution envelope, exclusions, and acceptance mapping |
| `perf-budgets.md` (register) | Operational NFR-PERF fail/warning table |
| `session-profiles.json` | Idle 1,000 / idle 128 / active 100 RSS |
| `kernel-micro.txt` | NFR-PERF-001/002/004/006 Criterion notes |
| `python-fast-path-report.json` | NFR-PERF-003 Python 10% |
| `wasm-js-crossing.json` | NFR-PERF-003 WASM 15% |
| `startup-report.json` | Warm full-example CLI + size |
| `size-budget-report.json` | A03 fail-gate results |
| `a01-comparison.md` | Versus `v0.1.0` method-plus-number corpus |
| `tdd-33-coverage.md` | TDD §33 harness map |
| `flamegraph-notes.md` | Secret-free profile stand-in |
| `optimize-none.md` | No justified hot-path change |
| `SHA256SUMS` | Artifact digests |
| `candidate-validation.txt` | Local Darwin A01–A04 proofs at `0a84c8196624bdc4eeff8ec229a304fe6d69866d` |
| `security-review.txt` | TM-04 / queue-bound review |

No G8 decision, hosted pull request, npm/pypi/crates.io publish,
tag, or `1.0.0` cut is stored here. Phase 8 is `Done` and G7 is
`Passed`. Phase 9 entrance is `Passed` (2/2) via
`PH9-E-entrance-tag-b610b0ba93b5` and
`PH9-E-entrance-feedback-ee6999c59a12` and is not re-recorded.
PR-062 is `Done`. Do not start PR-064+.
