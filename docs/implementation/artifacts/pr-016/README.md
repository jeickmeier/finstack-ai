# PR-016 local candidate evidence

PR-016 implements the executable Toolset port, compile-once Draft 2020-12
validation, fail-closed policy planning, bounded native execution, stream
normalization, panic containment, and kernel-owned source-order settlement on
`codex/pr-016-toolset-scheduler`.

This directory retains local working-tree validation, the mandatory security
and release review, and the truthful hosted-validation boundary. The candidate
is based on `b8af2ae1d15307371a1413ad007328df50282198`; it has not been committed,
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
| A01 | Instrumented gates and active counters prove global/per-tool limits, bounded parallel waves, queued cancellation, and sequential/barrier exclusivity | Local test passed; register Pending |
| A02 | Release-mode native panic fixture maps one call to fixed `tool_panicked`, preserves its sibling and worker, omits the payload from errors/durable state, and cleans the active registry | Local test passed; register Pending |
| A03 | Reverse parallel completion settles by arrival while durable tool messages, events, replay, and final state remain in model source order; progress advances only the confidential transient sequence | Local test passed; register Pending |
| A04 | Compile-once input/output validators, one planning boundary, approval floor/stricter override, hostile-metadata non-authority, offline references, and output bounds | Local test passed; register Pending |
| A05 | Default and independent fixture validators produce identical sorted issues, feedback, synthetic closures, and model-visible retry plans | Local test passed; register Pending |

Filesystem/shell tools, real approval interactions, event-hub delivery,
bindings, WIT/Wasmtime isolation, persistent reconciliation/recovery, and the
general timer/retry system remain deferred to their planned work.

PR-016 and A01–A05 remain `In progress`/`Pending` because there is no immutable
implementation revision, integration, hosted run, or independent review. G2
remains `Not ready`.
