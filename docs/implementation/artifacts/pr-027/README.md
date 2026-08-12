# PR-027 Python package and wheel-pipeline evidence

PR-027 creates the first-class `finstack-ai` Python distribution as one typed
mixed Rust/Python package. The immutable local source candidate is
`1dd60c5ec96464f07057d027dfda3d8a1d151855`. Local acceptance A02–A05 and the
approved 20-row hosted wheel matrix plus source distribution pass; final
aggregate hosted CI at the same revision is still running.

## Candidate acceptance map

| Acceptance | Candidate evidence | Disposition |
| --- | --- | --- |
| A01 | The pinned workflow executes all 20 approved target/interpreter rows across CPython 3.11, 3.12, 3.13, 3.14, and 3.14t, plus one sdist job | Passed hosted |
| A02 | Editable and installed-wheel subprocess tests import the module, call `health()`, and observe no new Python thread or socket audit event | Passed locally |
| A03 | Cargo, Python project, wheel metadata, runtime metadata, and checker all report `0.0.1`; 3.14t has an explicit GIL-disabled assertion | Passed locally |
| A04 | The binding graph includes PyO3 and the OpenAI-compatible provider; kernel/runtime production graphs exclude PyO3 and the binding crate | Passed locally |
| A05 | Two clean builds produce identical wheel/sdist bytes; archives pass path, license, typing, provider-linkage, metadata, and 10 MiB budget checks | Passed locally |

Exact commands, revision identity, and validation scope are recorded in
[`candidate-validation.txt`](candidate-validation.txt) and
[`final-candidate-validation.txt`](final-candidate-validation.txt), artifact hashes in
[`reproducibility.txt`](reproducibility.txt), and the TM-18 review in
[`security-review.txt`](security-review.txt). Hosted run and job identities are
retained in [`hosted-validation.txt`](hosted-validation.txt); six failed or
superseded evidence sets remain diagnostic only, and the final exact candidate
is awaiting aggregate CI completion.

No package publication, hosted pull request, hosted merge, live provider call,
or independent review is claimed.
