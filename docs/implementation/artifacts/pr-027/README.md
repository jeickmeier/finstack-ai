# PR-027 Python package and wheel-pipeline evidence

PR-027 creates the first-class `finstack-ai` Python distribution as one typed
mixed Rust/Python package. The immutable local source candidate is
`6ef57c2bdc95d1f1722f35c2a42a9249e3b17ce2`. The exact security workflow,
clean local aggregate, replacement 20-row hosted wheel matrix, and source
distribution pass; exact aggregate CI also passes.

## Candidate acceptance map

| Acceptance | Candidate evidence | Disposition |
| --- | --- | --- |
| A01 | The pinned workflow executes all 20 approved target/interpreter rows across CPython 3.11, 3.12, 3.13, 3.14, and 3.14t, plus one sdist job | Passed hosted |
| A02 | Editable and installed-wheel subprocess tests import the module, call `health()`, and observe no new Python thread or socket audit event | Passed locally |
| A03 | Cargo, Python project, wheel metadata, runtime metadata, and checker all report `0.0.1`; 3.14t has an explicit GIL-disabled assertion | Passed locally |
| A04 | The binding graph includes PyO3 and the OpenAI-compatible provider; kernel/runtime production graphs exclude PyO3 and the binding crate | Passed locally |
| A05 | Two clean release-staging builds produce identical wheel/sdist bytes on macOS; single-build Windows artifacts and all archives pass path, license, typing, provider-linkage, metadata, and 10 MiB budget checks | Passed locally |

Exact commands, revision identity, and validation scope are recorded in
[`candidate-validation.txt`](candidate-validation.txt) and
[`final-candidate-validation.txt`](final-candidate-validation.txt), artifact hashes in
[`reproducibility.txt`](reproducibility.txt), and the TM-18 review in
[`security-review.txt`](security-review.txt). Hosted run and job identities are
retained in [`hosted-validation.txt`](hosted-validation.txt); nine failed or
superseded evidence sets remain diagnostic only. All final-candidate acceptance
evidence is complete. The exact local merge and post-merge checks pass under
[`integration-validation.txt`](integration-validation.txt); PR-027 is `Done`.

No package publication, hosted pull request, hosted merge, live provider call,
or independent review is claimed.
