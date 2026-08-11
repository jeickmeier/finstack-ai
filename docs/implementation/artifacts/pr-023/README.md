# PR-023 local delivery evidence

PR-023 publishes the public `finstack-ai-test` support surface: deterministic
fixtures, conformance functions for the six primary ports, language-neutral
golden scenarios, public leaf-crate examples, and adversarial compaction
conformance. The immutable implementation candidate is commit
`740bc89131bda2919a166f5c796e1d3138a64ac6` with tree
`41704450f4c03a90d8bd111e421d4100be303abb` on
`codex/pr-023-test-kit`.

The exact candidate command inventory is in
[`candidate-validation.txt`](candidate-validation.txt), and the security and
threat-model disposition is in [`security-review.txt`](security-review.txt).
The reviewed evidence commit `f19558431f9bd39d581f57cf759207ccc896f043`
was merged locally to `main` at
`949f34010a2fe121b62681560f00eac1f58dfe76`; the merge tree exactly matches the
reviewed evidence tree. Post-merge focused and aggregate proof is in
[`integration-validation.txt`](integration-validation.txt).

## Local acceptance map

| Acceptance | Immutable candidate proof | Disposition |
| --- | --- | --- |
| A01 | Public-only Model and Toolset leaf crates execute `check_model_conformance` and `check_toolset_conformance` successfully | Passed on candidate |
| A02 | Architecture checks and normal dependency trees prove neither kernel nor runtime production dependencies include `finstack-ai-test` | Passed on candidate |
| A03 | Negative helper tests assert the exact port and stable violated-contract label | Passed on candidate |
| A04 | Strict checked-in scenarios load through the public test-kit API, and a real golden trace is driven through public SDK/test-kit/reducer APIs | Passed on candidate |
| A05 | Adversarial compaction conformance proves canonical history immutability, protected retention, tool-pair atomicity, checkpoint invalidation, hard-budget rejection, and byte-identical shared projections | Passed on candidate |

The implementation adds no kernel port, private runtime hook, certification
surface, production dependency on the test crate, provider network call, or
binding implementation. No hosted run, actual pull request, push, publication,
or independent review is claimed.
