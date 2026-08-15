# PR-048 artifacts

Crash-prefix matrix, optional journal prune, sqlite leaf ops, binding
restart traces, and the Phase 6 / G5 readiness pack.

| File | Owns |
| --- | --- |
| `plan.md` | Execution envelope, exclusions, and acceptance mapping |
| `candidate-validation.txt` | Local workspace, A01–A11, and graph proofs at `8342a6b56fc7832f9e861c6c034c0c1601e58f7a` |
| `security-review.txt` | TM-12 / Threat Model §18 review at the same candidate |
| `phase6-exit-review.txt` | Four Phase 6 exit bullets evidenced at the candidate |
| `phase6-exit-closeout.txt` | Named Phase 6 exit 4/4 closeout at merge `b0641b0be338918c1951339657ce8c04c4ccff59` |
| `g5-readiness-review.txt` | `READY FOR NAMED DECISION` at candidate `8342a6b56fc7`; superseded as a decision by `g5-decision.txt` |
| `g5-decision.txt` | Named `G5-D-durable-beta-a9568bd869b5` at current `main` tip `0a4b477c1eb9` |
| `ops-guidance.txt` | Backup/restore, prune/horizon, corruption classes, IndexedDB label |
| `staging/` | Unpublished `@finstack/ai` 0.0.3 tarball, checksums, and SBOM |
| `integration-validation.txt` | Local `main` merge `b0641b0be338918c1951339657ce8c04c4ccff59` |

G5 passed via `G5-D-durable-beta-a9568bd869b5` against current `main`
tip `0a4b477c1eb9b04d2e86294a9c01b36855ddd74c`. Phase 6 remains `Done`
at merge `b0641b0be338918c1951339657ce8c04c4ccff59`. No hosted pull
request, npm/pypi publish, or tag is stored here.
