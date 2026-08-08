# PR-005 artifacts

Validation evidence for golden-trace, conformance, and benchmark harnesses, plus Phase 0 / G0 closure.

Implementation commit (local A01–A04): `e97ac9b543cb8e3b1465b1d1ae76c8fdede51009`
PR head (hosted CI): `9b183e9baa574e30972568a6a8b26510630bb10b`
Merge commit: `c1108d207389a947d16e9b0dd7a76026108c01eb` ([#3](https://github.com/jeickmeier/finstack-ai/pull/3))

## Acceptance mapping

| Criterion | Proof |
| --- | --- |
| A01 | `conformance.txt` (noop byte-for-byte load/normalize/compare) |
| A02 | `conformance.txt` (durable vs transient fixture) |
| A03 | `benchmark-metadata.json` + `benchmark-compile.txt` / `mise run benchmark` |
| A04 | `conformance.txt` + schema fixtures + `schema-governance.txt` |
| A05 | `g0-decision.txt` / `G0-D-foundation-ready-bcf021e4873a` after Phase 0 exit evidence |

## Phase 0 / G0

| Record | Artifact |
| --- | --- |
| Hosted CI | `hosted-ci.txt` (runs 31275172068 / 31275172072 / 31275172064) |
| Entrance / exit / gate | `g0-decision.txt` |

## Commands

See individual `*.txt` logs and root `SHA256SUMS`.
