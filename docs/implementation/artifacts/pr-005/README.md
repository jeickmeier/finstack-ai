# PR-005 artifacts

Local validation evidence for golden-trace, conformance, and benchmark harnesses.

Branch: `pr-005-harnesses`
Implementation commit: `e97ac9b543cb8e3b1465b1d1ae76c8fdede51009`
Local command logs were captured immediately before that commit and rebound to it for A01–A04.

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
