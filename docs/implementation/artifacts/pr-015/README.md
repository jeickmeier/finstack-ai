# PR-015 evidence

PR-015 implements the provider-neutral Model port and scripted stream driver on
`codex/pr-015-model-port`.

This directory retains candidate validation, the mandatory security review,
and immutable local integration evidence. Implementation commit
`f3a9ffb891a7dd7d5ed0afe2971d35aa266d3c3a` was merged locally on `main` as
`16f3a865d122aac34d1df5b3dc1d7a0b44c4aa82`; both commits have tree
`13e1a236f7a2ce63bc20a9586ba9e8c7bb54592f`. Artifact digests are retained in
[`SHA256SUMS`](SHA256SUMS).

The exact local command/result inventory is in
[`candidate-validation.txt`](candidate-validation.txt). The TM-02/TM-04/
TM-14/TM-16/TM-18 and SEC-INV disposition is in
[`security-review.txt`](security-review.txt). Merge-tree identity and post-merge
checks are in [`integration-validation.txt`](integration-validation.txt).
[`hosted-validation.txt`](hosted-validation.txt) records truthfully that no
actual pull request or hosted run exists.

## Candidate acceptance map

| Acceptance | Candidate proof | Final disposition |
| --- | --- | --- |
| A01 | `mise run test-model`; `model_port` byte-identical 1/10/100/1,000-chunk assembler and full durable-runtime projections | Passed at local merge |
| A02 | Deterministic cancellation, consumer-drop, normal-shutdown, and grace-deadline forced-abort tests with zero active scripted streams | Passed at local merge |
| A03 | Exact terminal/tool/usage/limit/response error tables plus durable malformed-stream failure with no assistant message or completion identity | Passed at local merge |
| A04 | Default-feature-free leaf `Model` compiled natively and for `wasm32-unknown-unknown`; minimal/WASM repository checks | Passed at local merge |

No provider, router, network client, executable toolset, binding surface, event
subscription API, persistent recovery path, or new kernel record/event kind is
included.

PR-015 is `Done` by local integration. G2 remains `Not ready`, and no hosted or
independent-review evidence is claimed.
