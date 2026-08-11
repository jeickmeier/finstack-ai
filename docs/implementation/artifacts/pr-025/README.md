# PR-025 local delivery evidence

PR-025 implements the trusted native calculator and capability-scoped
filesystem toolsets. The immutable implementation candidate is commit
`faac2e30f8a31e319b6af68abf4db5dd450f421b` with tree
`6e64a3d17a5edfbb4084f921009b47063705c6b4`; its evidence commit is
`3e6157a2116cc58620257f1eb4cd01eb5fc6e1d0`. The local `main` merge is
`ee76c71354c2ca504283478a3208e1020c90cad6`, whose tree exactly matches
the evidence tree `9619754f442061e1e34870488749194266196d8a`.

The exact candidate command inventory is in
[`candidate-validation.txt`](candidate-validation.txt), and the TM-03 control
review is in [`security-review.txt`](security-review.txt). Local merge identity
and post-merge gates are recorded in
[`integration-validation.txt`](integration-validation.txt).

## Local acceptance map

| Acceptance | Immutable candidate proof | Disposition |
| --- | --- | --- |
| A01 | Traversal, final/intermediate symlink escape, protected path, write-through-symlink, oversized-result, and no-store fixtures pass; large success output is staged, never spilled | Passed on candidate |
| A02 | Deterministic barrier fixtures swap before final open and rename/replace after final open; outside canary bytes are never read or written | Passed on candidate for Unix implementation |
| A03 | Calculator and filesystem tests prove repeated `tools()` calls retain the same `Arc<[ToolSpec]>` allocation | Passed on candidate |
| A04 | Every call validates principal/locator scope; artifact fixtures preserve exact tenant/session/run scope and record the committed effect identity as non-authoritative metadata | Passed on candidate |
| A05 | Optimized Criterion `calculator_add_256_operands` benchmark compiles in focused/full gates and executes successfully | Passed on candidate |

The package is intentionally native. macOS/Unix safe primitives and adversarial
fixtures ran locally; the non-Unix constructor returns
`filesystem_unsupported`, but no Windows cross-target was installed locally.
No shell, Git integration, browser automation, OS sandbox, hosted run, actual
pull request, push, publication, or independent review is claimed.
