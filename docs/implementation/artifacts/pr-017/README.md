# PR-017 local candidate evidence

PR-017 implements the native per-run event hub, bounded subscriber delivery,
ordered batching, filtering, lag policies, incremental model/tool progress, and
owned shutdown semantics. The immutable implementation is commit
`1bfc653c36312187f34c6ad7ba787a360145612d` with tree
`961b423876e92a9e5ae013d3ff49ea729b9fe800` on
`codex/pr-017-event-hub`.

This directory retains local validation and the required TM-17 review. The
exact command/result inventory is in
[`candidate-validation.txt`](candidate-validation.txt), and the security
disposition is in [`security-review.txt`](security-review.txt).
[`hosted-validation.txt`](hosted-validation.txt) records the truthful hosted
and independent-review boundary.

## Local acceptance map

| Acceptance | Immutable candidate proof | Disposition |
| --- | --- | --- |
| A01 | Every hub, subscriber, batch, model-driver, and tool-driver queue is configured and bounded; invalid zero bounds and excess subscriptions fail; a 100,000-event direct stream terminates without an unbounded queue | Passed at implementation commit |
| A02 | Paused-time count, byte, timer, oversized-event, durable-boundary, and terminal tests flatten to the original event identities and sequence order | Passed at implementation commit |
| A03 | The observer route never waits on source fan-out; a stalled observer is isolated from an interactive subscriber and semantic execution; TM-17 review passes for PR-017 scope | Passed at implementation commit |
| A04 | Transient lag drops are reported per batch and cumulatively; durable delivery either succeeds within its bound or closes explicitly as `MissedDurable`; the journal recovery test proves a missed terminal remains authoritative | Passed at implementation commit |

`CommitOutcome.events` and the public model/tool `assemble()` behavior remain
intact. Runtime progress EventIds use an isolated UUIDv7 entropy stream so
one, ten, one hundred, and one thousand chunks preserve identical durable
outcomes while progress is observable before terminal settlement.

PR-017 acceptance A01-A04 is closed against the immutable implementation
commit, but the logical PR remains `In progress` until integration. No hosted
run, actual pull request, independent review, merge, or G2 decision is claimed.
PR-018 observer attachment, PR-020 performance baselines, PR-057 redacted
observer projections, and later SDK/binding result handles remain open.
