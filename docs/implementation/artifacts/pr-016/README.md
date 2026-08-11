# PR-016 local candidate evidence

PR-016 implements the executable Toolset port, compile-once Draft 2020-12
validation, fail-closed policy planning, bounded native execution, stream
normalization, panic containment, and kernel-owned source-order settlement on
`codex/pr-016-toolset-scheduler`.

This directory retains immutable local candidate validation, the mandatory
security and release review, and the truthful hosted-validation boundary. The
validated implementation is commit `aab7b81b8cb36eca82860ffa38ded7ba82a14247`
with tree `7895ef08523dfbb4d98b645ebd91b47cc2626006`. It has not yet been
merged, pushed, or submitted as an actual pull request. Artifact digests are
retained in [`SHA256SUMS`](SHA256SUMS).

The exact command/result inventory is in
[`candidate-validation.txt`](candidate-validation.txt). The TM-01/TM-02/
TM-14/TM-16/TM-18 and SEC-INV disposition is in
[`security-review.txt`](security-review.txt). Release panic-policy and smoke
results are in [`release-validation.txt`](release-validation.txt).
[`hosted-validation.txt`](hosted-validation.txt) records that no hosted or
independent-review evidence exists.

## Local acceptance map

| Acceptance | Local candidate proof | Disposition |
| --- | --- | --- |
| A01 | Instrumented gates and active counters prove global/per-tool limits, bounded parallel waves, queued cancellation, and sequential/barrier exclusivity | Passed at implementation commit |
| A02 | Release-mode native panic fixture maps one call to fixed `tool_panicked`, preserves its sibling and worker, omits the payload from errors/durable state, and cleans the active registry | Passed at implementation commit |
| A03 | Reverse parallel completion settles by arrival while durable tool messages, events, replay, and final state remain in model source order; progress advances only the confidential transient sequence | Passed at implementation commit |
| A04 | Compile-once input/output validators, one planning boundary, approval floor/stricter override, hostile-metadata non-authority, offline references, and output bounds | Passed at implementation commit |
| A05 | Default and independent fixture validators produce identical sorted issues, feedback, synthetic closures, and model-visible retry plans | Passed at implementation commit |

Filesystem/shell tools, real approval interactions, event-hub delivery,
bindings, WIT/Wasmtime isolation, persistent reconciliation/recovery, and the
general timer/retry system remain deferred to their planned work.

PR-016 remains `In progress` pending local integration and post-merge
validation. A01–A05 are passed at the immutable implementation commit. No
hosted run or independent review is claimed, and G2 remains `Not ready`.
