# PR-020 local delivery evidence

PR-020 stabilizes the native runtime before the SDK phase. The immutable
implementation candidate is commit
`91a9faec7d58e4ea594e5459bc48f6f977975dbb` with tree
`6a286bc63fc2df6a8cd20d7d68b255b355bf88db` on
`codex/pr-020-native-runtime-gate`.

The exact local command inventory is in
[`candidate-validation.txt`](candidate-validation.txt), the security and
threat-model disposition is in [`security-review.txt`](security-review.txt),
host-specific measurements are in
[`benchmark-baselines.txt`](benchmark-baselines.txt), and
[`hosted-validation.txt`](hosted-validation.txt) records the external-evidence
boundary. The branch was integrated locally into `main` by merge
`f101b260dd6c9ca6ffa82d805824669c5d974ca0`; its tree exactly equals the
evidence-commit tree at `283b676ebd92b82fae43a9c8189688f13a19f784`.
Post-merge identity and validation are in
[`integration-validation.txt`](integration-validation.txt), and the separate
passing G2 review is in [`g2-decision.txt`](g2-decision.txt).

## Local acceptance map

| Acceptance | Immutable candidate proof | Disposition |
| --- | --- | --- |
| A01 | The coordinator crash-prefix matrix proves a failed append never reaches a provider, every effect pauses only after verified append/apply/recheck, recovery observes the committed pending effect while dispatch count remains zero, and one explicit permit releases exactly the observed action | Passed at candidate |
| A02 | Kernel/runtime source is unsafe-free; the checked-in pinned-Miri task passes representative raw-JSON, reducer-apply, and deterministic-fingerprint paths; normal, minimal, WASM, Clippy, Rustdoc, and aggregate suites pass | Passed at candidate |
| A03 | 128 repeated idle owners join without abort; 24 full scripted model runs settle with zero live streams; tool shutdown, cancellation, backpressure, slow-observer isolation, queue bounds, and host-specific reducer/model/tool/idle-memory baselines pass | Passed at candidate |
| A04 | Immutable local integration passed, all four Phase 2 exits passed, and the separate G2 decision reviewed commit-before-effect, bounded queues, ownership, cancellation, slow consumers, leaks, faults, security, and residual scope | Passed at G2 |

The implementation adds no provider, network client, database, workflow
engine, plugin host, public SDK, serialized kernel contract, or production
dependency. No hosted pull request, push, publication, hosted run, or
independent review is claimed. PR-020 and Phase 2 are `Done`; G2 is `Passed`.
