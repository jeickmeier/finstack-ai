# PR-016 local delivery evidence

PR-016 implements the executable Toolset port, compile-once Draft 2020-12
validation, fail-closed policy planning, bounded native execution, stream
normalization, panic containment, and kernel-owned source-order settlement. The
work was integrated locally into `main` by merge
`b2f678693fcd3eb2fe09aafca55c4e06c17371ec`.

This directory retains immutable local candidate validation, the mandatory
security and release review, the truthful hosted-validation boundary, and the
post-merge integration proof. The validated implementation is commit
`aab7b81b8cb36eca82860ffa38ded7ba82a14247` with tree
`7895ef08523dfbb4d98b645ebd91b47cc2626006`. Candidate evidence was bound by
`9f7c42d77c966d71ac67b5c44f1d7a64ffa242b6`; the merge tree
`fe657d9eed055c9424471854854816de37f6f890` exactly equals that candidate tip.
No push or actual pull request is claimed. Artifact digests are retained in
[`SHA256SUMS`](SHA256SUMS).

The exact command/result inventory is in
[`candidate-validation.txt`](candidate-validation.txt). The TM-01/TM-02/
TM-14/TM-16/TM-18 and SEC-INV disposition is in
[`security-review.txt`](security-review.txt). Release panic-policy and smoke
results are in [`release-validation.txt`](release-validation.txt). Exact merge
identity and post-merge checks are in
[`integration-validation.txt`](integration-validation.txt).
[`hosted-validation.txt`](hosted-validation.txt) records that no hosted or
independent-review evidence exists.

## Local acceptance map

| Acceptance | Local candidate proof | Disposition |
| --- | --- | --- |
| A01 | Instrumented gates and active counters prove global/per-tool limits, bounded parallel waves, queued cancellation, and sequential/barrier exclusivity | Passed at implementation and local merge |
| A02 | Release-mode native panic fixture maps one call to fixed `tool_panicked`, preserves its sibling and worker, omits the payload from errors/durable state, and cleans the active registry | Passed at implementation and local merge |
| A03 | Reverse parallel completion settles by arrival while durable tool messages, events, replay, and final state remain in model source order; progress advances only the confidential transient sequence | Passed at implementation and local merge |
| A04 | Compile-once input/output validators, one planning boundary, approval floor/stricter override, hostile-metadata non-authority, offline references, and output bounds | Passed at implementation and local merge |
| A05 | Default and independent fixture validators produce identical sorted issues, feedback, synthetic closures, and model-visible retry plans | Passed at implementation and local merge |

Filesystem/shell tools, real approval interactions, event-hub delivery,
bindings, WIT/Wasmtime isolation, persistent reconciliation/recovery, and the
general timer/retry system remain deferred to their planned work.

PR-016 is `Done` at the immutable local merge, with A01–A05 passed and
post-merge Toolset and aggregate validation complete. No hosted run or
independent review is claimed, and G2 remains `Not ready`.
