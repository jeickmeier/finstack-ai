# PR-017 local delivery evidence

PR-017 implements the native per-run event hub, bounded subscriber delivery,
ordered batching, filtering, lag policies, incremental model/tool progress, and
owned shutdown semantics. The immutable implementation is commit
`1bfc653c36312187f34c6ad7ba787a360145612d` with tree
`961b423876e92a9e5ae013d3ff49ea729b9fe800` on
`codex/pr-017-event-hub`.

The branch was integrated locally into `main` by merge
`966f047fd28972d35c835ddf0441f8ba67348b25`. Its tree
`8e65c8c53a3e0ee9f84a6f769879f2548b892479` exactly equals the validated
branch-tip tree at evidence commit
`2e4740d8dc88460496601f97710a8a9449c47165`.

This directory retains local validation and the required TM-17 review. The
exact command/result inventory is in
[`candidate-validation.txt`](candidate-validation.txt), and the security
disposition is in [`security-review.txt`](security-review.txt).
[`hosted-validation.txt`](hosted-validation.txt) records the truthful hosted
and independent-review boundary. Post-merge identity and validation are in
[`integration-validation.txt`](integration-validation.txt).

## Local acceptance map

| Acceptance | Immutable candidate proof | Disposition |
| --- | --- | --- |
| A01 | Every hub, subscriber, batch, model-driver, and tool-driver queue is configured and bounded; invalid zero bounds and excess subscriptions fail; a 100,000-event direct stream terminates without an unbounded queue | Passed at implementation and local merge |
| A02 | Paused-time count, byte, timer, oversized-event, durable-boundary, and terminal tests flatten to the original event identities and sequence order | Passed at implementation and local merge |
| A03 | The observer route never waits on source fan-out; a stalled observer is isolated from an interactive subscriber and semantic execution; TM-17 review passes for PR-017 scope | Passed at implementation and local merge |
| A04 | Transient lag drops are reported per batch and cumulatively; durable delivery either succeeds within its bound or closes explicitly as `MissedDurable`; the journal recovery test proves a missed terminal remains authoritative | Passed at implementation and local merge |

`CommitOutcome.events` and the public model/tool `assemble()` behavior remain
intact. Runtime progress EventIds use an isolated UUIDv7 entropy stream so
one, ten, one hundred, and one thousand chunks preserve identical durable
outcomes while progress is observable before terminal settlement.

PR-017 is `Done` at the immutable local merge with A01-A04 passed and
post-merge `test-events` and aggregate CI complete. No hosted run, actual pull
request, branch push, or independent review is claimed, and G2 remains `Not
ready`. PR-018 observer attachment, PR-020 performance baselines, PR-057
redacted observer projections, and later SDK/binding result handles remain
open.
