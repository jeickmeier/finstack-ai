# PR-064 artifacts

Reliability, extended fuzz, crash/recovery catalog, and
independent security review (TM §13.3). Admitted 2026-08-15 on
`codex/pr-064-reliability-fuzz-security-review` from local `main`
`587d01da4f900291926b290c76f1eafa487fde0b`.

| File | Owns |
| --- | --- |
| `plan.md` | Execution envelope, exclusions, and acceptance mapping |
| `findings.md` | Independent-review findings (`FIND-064-*`) |
| `independent-review.md` | Named first-party independent review (TM §13.3) |
| `candidate-validation.txt` | Local Darwin A01–A04 proofs at `363d52661eeb726c2f8e2d8ee103c7dc11ba4505` |
| `security-review.txt` | TM §13.3 / FIND-064-* closeout |
| `integration-validation.txt` | Local `main` merge `299888d06671d0bd6873dc3033fbaf4465ccf3c9` |
| `SHA256SUMS` | Artifact digests |
| [threat-model-g8-review.md](../../threat-model-g8-review.md) | TM-01–TM-21 and §13.3 matrix (not `G8-D-*`) |

No G8 decision, hosted pull request, npm/pypi/crates.io publish,
tag, or `1.0.0` cut is stored here. Phase 8 is `Done` and G7 is
`Passed`. Phase 9 entrance is `Passed` (2/2) via
`PH9-E-entrance-tag-b610b0ba93b5` and
`PH9-E-entrance-feedback-ee6999c59a12` and is not re-recorded.
PR-055–PR-063 are `Done`. Do not start PR-065+. Do not write
`G8-D-*`.
