# PR-065 artifacts

Ecosystem conformance suites, compatibility-badge process, and
release engineering (checksum recreation, starter RC tests,
rollback/hotfix rehearsal, support windows). Admitted
2026-08-15 on `codex/pr-065-ecosystem-conformance-release-eng`
from local `main` `c2159c79a4aedea82bb50b19f387d88b8c271052`.

| File | Owns |
| --- | --- |
| `plan.md` | Execution envelope, exclusions, and acceptance mapping |
| `run-a.SHA256SUMS` / `run-b.SHA256SUMS` | A01 two-run recreation (written by `mise run recreate-release`) |
| `SHA256SUMS-B` / `SHA256SUMS-H` | A04 hotfix rehearsal pair |

No G8 decision, hosted pull request, npm/pypi/crates.io publish,
yank, tag, or `1.0.0` cut is stored here. Phase 8 is `Done` and
G7 is `Passed`. Phase 9 entrance is `Passed` (2/2) via
`PH9-E-entrance-tag-b610b0ba93b5` and
`PH9-E-entrance-feedback-ee6999c59a12` and is not re-recorded.
Do not start PR-066. No plugin marketplace.
