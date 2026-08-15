# PR-066 artifacts

Local `1.0.0` general-availability closeout. Candidate admitted on
`codex/pr-066-1.0.0-ga` from `04962c743d2f03d59b74feb9c71f872cccfcc003`.
G8 passed via `G8-D-general-availability-a889a29a3f54` against
`6e9ec39fae89a70f696ee740de2d2094670cba3e`. Local tag `v1.0.0`
points at that commit.

| File | Owns |
| --- | --- |
| `plan.md` | Historical execution envelope, exclusions, and acceptance mapping |
| `soak-note.md` | External-adopter soak gap (do not fabricate) |
| `soak-acceptance.txt` | Owner acceptance of the §21.12 gap as a G8 residual |
| `g8-readiness-review.txt` | Historical `READY FOR NAMED DECISION` pack |
| `g8-decision.txt` | Named G8 decision |
| `phase9-exit-closeout.txt` | Phase 9 exit 4/4 |
| `run-a.SHA256SUMS` / `run-b.SHA256SUMS` | Two-run staged `1.0.0` identity |
| `provenance.json` | Staging provenance (`staged_not_published`) |
| `security-review.txt` | TM-18 review for GA publish/credentials/announce |
| `candidate-validation.txt` | Local candidate commands and A02/A03 pass |
| `integration-validation.txt` | Local `main` merge tree proof |

No hosted pull request, npm/pypi/crates.io publish, tag push,
GitHub Release, or announce is stored here. Phase 9 entrance stays
`Passed` (2/2) via `PH9-E-entrance-tag-b610b0ba93b5` and
`PH9-E-entrance-feedback-ee6999c59a12` and is not re-recorded.
No marketplace, channel catalog, or product UI.
