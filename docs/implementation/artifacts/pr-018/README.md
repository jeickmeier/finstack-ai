# PR-018 local delivery evidence

PR-018 completes the six-port composition surface with bounded context providers,
the seven-stage middleware ABI, deterministic ordering, durable invocation guards,
compaction integrity contracts, and immutable observers. The immutable implementation
candidate is commit `aac573b593dd3950a4143672aeadc3765fa88a29` with tree
`622f2eb60a78246bbebf7d780ab0699026c9ec68` on
`codex/pr-018-context-middleware`.

The exact local command inventory is in
[`candidate-validation.txt`](candidate-validation.txt), the required security and
threat-model disposition is in [`security-review.txt`](security-review.txt), and
[`hosted-validation.txt`](hosted-validation.txt) records the external-evidence
boundary. Integration evidence is intentionally absent until the candidate and this
evidence-only update are merged locally into `main` and revalidated.

## Local acceptance map

| Acceptance | Immutable candidate proof | Disposition |
| --- | --- | --- |
| A01 | The default-feature-free `extension-port-leaf` crate implements `ContextProvider`, `Middleware`, and `Observer` through public runtime exports only and compiles natively and for `wasm32-unknown-unknown`; the kernel has no PR-018 source change | Passed at candidate |
| A02 | Resolver tests reject missing named requirements and ordering cycles before invocation | Passed at candidate |
| A03 | `Observer` receives only shared immutable `Arc<[RunEvent]>` batches; its view owns no kernel, journal, reducer input, dispatcher, or mutable execution handle | Passed at candidate |
| A04 | Context assembly is independent of provider completion order, rejects cursor gaps, applies a deterministic whole-item reject/truncate policy, and emits per-provider truncation diagnostics | Passed at candidate |
| A05 | The fixed outcome matrix permits typed approval interaction at `before_finalize` but rejects replacement there; observers cannot return a behavior-changing outcome or access committed terminal state | Passed at candidate |
| A06 | Compaction validates a source-ordered model projection, byte-identical protected content, active user retention, tool call/result atomicity, hard budgets, untrusted derived summaries, inherited sensitivity, and leaves canonical source entries unchanged | Passed at candidate |
| A07 | Resolution rejects duplicate compaction owners and context mutation after compaction; checkpoint compatibility locks component/strategy/version/config/model profile/source prefix/summary/sensitivity | Passed at candidate |
| A08 | Exact committed-record guards prevent context/middleware I/O before durable intent; recorded completion reuse, recompute/reconcile/non-repeatable recovery actions, and exact related child-model validation cover every exposed invocation boundary | Passed at candidate |

The candidate introduces no provider, first-party compaction strategy, semantic-memory
system, telemetry exporter, hosted pull request, push, publication, or external
service. PR-018 remains `In progress` until local integration evidence is created.
