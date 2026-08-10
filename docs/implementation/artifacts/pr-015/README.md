# PR-015 candidate evidence

PR-015 implements the provider-neutral Model port and scripted stream driver on
`codex/pr-015-model-port`.

This directory retains working-tree candidate validation and the mandatory
security review. The candidate is based on `main` commit
`08f0e6395d6514eaf06e87da7ac3b837335790be`; it is not an immutable
implementation revision and therefore does not close PR-015 A01–A04.
Candidate artifact digests are retained in [`SHA256SUMS`](SHA256SUMS).

The exact local command/result inventory is in
[`candidate-validation.txt`](candidate-validation.txt). The TM-02/TM-04/
TM-14/TM-16/TM-18 and SEC-INV disposition is in
[`security-review.txt`](security-review.txt). Hosted and immutable-review status
is recorded in [`hosted-validation.txt`](hosted-validation.txt).

## Candidate acceptance map

| Acceptance | Candidate proof | Final disposition |
| --- | --- | --- |
| A01 | `mise run test-model`; `model_port` byte-identical 1/10/100/1,000-chunk assembler and full durable-runtime projections | Pending immutable reviewed revision |
| A02 | Deterministic cancellation, consumer-drop, normal-shutdown, and grace-deadline forced-abort tests with zero active scripted streams | Pending immutable reviewed revision |
| A03 | Exact terminal/tool/usage/limit/response error tables plus durable malformed-stream failure with no assistant message or completion identity | Pending immutable reviewed revision |
| A04 | Default-feature-free leaf `Model` compiled natively and for `wasm32-unknown-unknown`; minimal/WASM repository checks | Pending hosted and immutable reviewed revision |

No provider, router, network client, executable toolset, binding surface, event
subscription API, persistent recovery path, or new kernel record/event kind is
included.
