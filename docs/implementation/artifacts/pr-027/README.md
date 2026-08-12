# PR-027 Python package and wheel-pipeline evidence

PR-027 creates the first-class `finstack-ai` Python distribution as one typed
mixed Rust/Python package. The immutable local source candidate is
`81bc1aa769e8e8f5eefb8b36273862ff14f411ed`. Local acceptance A02–A05 passes;
A01 remains pending until the approved 20-row hosted wheel matrix and source
distribution job complete against the evidence revision.

## Candidate acceptance map

| Acceptance | Candidate evidence | Disposition |
| --- | --- | --- |
| A01 | The pinned workflow defines four approved targets across CPython 3.11, 3.12, 3.13, 3.14, and 3.14t, plus one sdist job; actionlint passes | Pending hosted execution |
| A02 | Editable and installed-wheel subprocess tests import the module, call `health()`, and observe no new Python thread or socket audit event | Passed locally |
| A03 | Cargo, Python project, wheel metadata, runtime metadata, and checker all report `0.0.1`; 3.14t has an explicit GIL-disabled assertion | Passed locally |
| A04 | The binding graph includes PyO3 and the OpenAI-compatible provider; kernel/runtime production graphs exclude PyO3 and the binding crate | Passed locally |
| A05 | Two clean builds produce identical wheel/sdist bytes; archives pass path, license, typing, provider-linkage, metadata, and 10 MiB budget checks | Passed locally |

Exact commands, revision identity, and validation scope are recorded in
[`candidate-validation.txt`](candidate-validation.txt), artifact hashes in
[`reproducibility.txt`](reproducibility.txt), and the TM-18 review in
[`security-review.txt`](security-review.txt). Hosted run and job identities will
be retained in [`hosted-validation.txt`](hosted-validation.txt); two failed
attempts are retained as diagnostic evidence, and the corrected replacement
attempt remains pending.

No package publication, hosted pull request, hosted merge, live provider call,
or independent review is claimed.
