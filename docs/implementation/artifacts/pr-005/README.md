# PR-005 artifacts

Local validation evidence for golden-trace, conformance, and benchmark harnesses.

Branch: `pr-005-harnesses`
Captured against worktree tip `f51671507a148666d5a42f29086d97b3d6877f93` (pre-merge; bind to immutable PR/merge heads when available).

## Acceptance mapping

| Criterion | Local proof |
| --- | --- |
| A01 | `conformance.txt` (noop byte-for-byte load/normalize/compare) |
| A02 | `conformance.txt` (durable vs transient fixture) |
| A03 | `benchmark-metadata.json` + `benchmark-compile.txt` / `mise run benchmark` |
| A04 | `conformance.txt` + schema fixtures + `schema-governance.txt` |
| A05 | deferred to post-merge named G0 decision |

## Commands

See individual `*.txt` logs and root `SHA256SUMS`.
