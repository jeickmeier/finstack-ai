# PR-019 local delivery evidence

PR-019 connects durable kernel cancellation, deadlines, semantic retry timers,
and bounded task ownership to the native Tokio runtime. The immutable
implementation candidate is commit `941403749648d2ec3e6e85a8af1a0b73afd80866`
with tree `31f65eb7ca93cfccd126637ead5bb0d225913770` on
`codex/pr-019-cancellation-timers`.

The branch was integrated locally into `main` by merge
`bf748d5aa1ee7ffa11cf9ac3ab2dea399034dd0b`. Its tree
`352bf9bf417f9eee628c59f765ee880e58c4d3c7` exactly equals the PR-019
evidence-commit tree at `56e7f64f52e84b2e289863355a3b04740115111c`.

The exact local command inventory is in
[`candidate-validation.txt`](candidate-validation.txt), the required security and
threat-model disposition is in [`security-review.txt`](security-review.txt), and
[`hosted-validation.txt`](hosted-validation.txt) records the external-evidence
boundary. Post-merge identity and validation are in
[`integration-validation.txt`](integration-validation.txt).

## Local acceptance map

| Acceptance | Immutable candidate proof | Disposition |
| --- | --- | --- |
| A01 | A downward-only cancellation tree scopes run, model, tool-batch, individual-tool, and timer tasks; model and tool cancellation fixtures reach durable `RunPhase::Cancelled`; graceful and forced shutdown classifications are asserted | Passed at candidate |
| A02 | Persisted wall deadlines are converted once to Tokio monotonic instants; active wall-clock changes cannot extend them; overdue work is rejected before provider entry; waits use `sleep_until` rather than polling | Passed at candidate |
| A03 | A durable retry scheduled at attempt 1 survives owner shutdown, is recovered overdue by a replacement owner, fires once, clears the pending timer, and resumes at `PreparingContext` without resetting the attempt | Passed at candidate |
| A04 | The sole `RunTaskOwner` owns commit, model, tool, timer, and event tasks; explicit shutdown propagates cancellation, waits through a bounded grace period, aborts remaining tasks when required, and exposes safe diagnostics | Passed at candidate |

The implementation introduces no durable database, workflow-engine clock,
provider, hosted pull request, push, publication, or external service. PR-020
still owns the broad crash-prefix, race, stress, leak, and concurrency gate, so
G2 is not decided by this evidence bundle. PR-019 is `Done` at its immutable
local merge with A01–A04 passed.
