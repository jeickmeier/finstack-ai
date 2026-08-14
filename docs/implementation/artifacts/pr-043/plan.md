# PR-043 execution plan

Date: 2026-08-14
Owner: me@jeickmeier.com
Branch: `codex/pr-043-tool-reconciliation`
Baseline: local `main` after PR-042 closeout `a3c542446dd16a9177772097cf51803ec06081d8`
Plan baseline: documentation pack v0.20 / PLAN-0.18 / Implementation Plan SHA-256 `555a150fa9eaa2de39342eabdfd3d050b19628d735adbf498a9d75fcbc1102a4`

This file is the execution contract for PR-043. It does **not** authorize
coding, branch creation, commit, merge, or hosted actions. The closed
PR-042 envelope is not reused. `continue` does not start this PR.

## Admission

- Phase 6 entrance is `Passed` (3/3). Do not re-record `PH6-E-entrance-*`.
- PR-039 is `Done` at `64c54e767f53faac240ab92c19a8447264e82ff8`.
- PR-040 is `Done` at `dbd10d35b223288666b2fdc0e13d03f48b5b97c3`.
- PR-041 is `Done` at `8a84293264291dbe158fae1b4e5dedcc30c74061`.
- PR-042 is `Done` at local merge `4d627711632c771d733b689e1325b2d9679ee317`
  (closeout `a3c542446dd16a9177772097cf51803ec06081d8`).
- ADR-025 is Accepted / Partial (tool and interaction runtime lifecycle
  remain). This PR continues the tool path only. Do not mark it
  Verified.
- ADR-033 stays Implemented / Verified for **model** effects. Do not
  reinterpret it as tool-only or reopen it.
- ADR-013 stays Accepted / Not started in the register (Missing
  evidence). The ADR file already records Partial PR-014 store
  evidence; later mapped work includes PR-046–PR-048. Do not silently
  mark ADR-013 Implemented.
- ADR-034 stays PR-018/PR-048.
- PR-043 is the only active logical PR once admitted. PR-044+ stays
  excluded.
- No Implementation Plan section 6.3 ADR trigger applies if the work
  stays inside at-least-once plus idempotency/reconciliation (no
  stronger exactly-once claim). Architecture §10.4 is the statement to
  preserve, not a new guarantee.
- Threat Model section 18 is triggered (effect recovery semantics;
  TM-10, also TM-14; SEC-INV-003/004/008). Complete the review before
  merge.
- Dependencies: PR-039–PR-041 and the Toolset port. Both are present.
  PR-042 is the locked analog, not a named Implementation Plan
  dependency.

## Acceptance mapping

- PR-043-A01: in-process crash fixtures at every tool-effect boundary
  (before start, during execution, after result, before result commit)
  restore to a documented `ToolResumeAction` (see crash matrix).
- PR-043-A02: idempotent tools (`SideEffectClass::ReadOnly` or
  `IdempotentWrite` plus `RetrySafety::SafeToRetry` or
  `IdempotentWithKey`) retry with the same `EffectId`, `ToolCallId`,
  `ToolBatchId`, and frozen `ValidatedToolCall`. After a committed
  `ToolCallSettled` / `tool_settlements` hit, spawn/recover never
  `call()` that `EffectId` again.
- PR-043-A03: `Unknown` or `NonRepeatable` when retry is not allowed
  (including `NonIdempotentWrite` / `AtMostOnce`) suspends with
  `tool_reconciliation_unsupported` and does not `call()` or fabricate
  a successful/tool-produced `ToolCallSettled`.
- PR-043-A04: after a restore that continues, recovered history has a
  source-ordered valid tool result for every accepted call in
  `ToolBatchOpened`, including the pre-crash completed subset.
  `SuspendUncertain` leaves outstanding calls unsettled and does not
  invent results.
- PR-043-A05: tool resume uses the existing deferred-effect /
  `ToolBatchSettled` / `ExternalEffectCompleted` protocol. No seventh
  port, no new journal fields, no parallel suspension or completion
  machine. Equal external tool completion stays idempotent; conflict
  fails closed.

## Execution envelope

Authorized 2026-08-14 by `implement the plan`.

Local branch and commit are authorized. Merge to `main`, feature-branch
push, hosted PR/merge, npm publish, tag, and G5 inference are not named
and remain prohibited. The closed PR-042 envelope is not reused.

`mode=stacked` (local implementation); `target` unset; `external
actions=none`.

Baseline: local `main` at `a3c542446dd16a9177772097cf51803ec06081d8`.
Branch: `codex/pr-043-tool-reconciliation`.

## Locked design

Do not invent `DeferredHandle`, `SuspendedOperation`, a generic
`ReconcileResult`/`EffectOutput`, a seventh port, or new journal
fields. TDD §15.1 still sketches `ReconcileResult`; the runtime
already specialized this to `ToolReconcileResult`. Keep that type.

1. TDD §23.2 `StillRunning(DeferredHandle)` is the existing
   `ToolReconcileResult::StillRunning(ToolDeferral)`.
   `Deferred(ToolDeferral)` is the same recovery action. Both require a
   committed `EffectDeferred` on **that call** before wait.
2. TDD §23.1 `SuspendedOperation` is `ActiveToolBatch` plus per-call
   `ActiveToolCallStatus` on `KernelState` plus a pure
   `ToolResumeAction`. No public DTO. No new `RunSuspended` record.
3. Architecture §10.3 is the durable classifier. Unstarted, in-flight,
   and completed-but-uncommitted journals are identical for one call
   (`AwaitingTools` + `Requested { deferred: None }`, no settlement)
   and are §10.3 *possibly started*. Reconcile distinguishes them.
   §10.3 *completed* is the A02 settlement-index hit. §10.4 remains
   exactly-once **record application** only.
4. `Completed(EffectOutput)` is `Completed(ToolResult)`. Reuse
   `build_tool_settlement` / `process_tool_result`. Direct
   `AwaitingTools` → `KernelInput::ToolBatchSettled`.
   `AwaitingExternal` on **that call** (`deferred.is_some()`) →
   `ExternalEffectCompleted` and reuse `allocate_tool_settlement` IDs
   (same assistant-message-mismatch class of bug as PR-042).
5. Ignore live tool streams after crash. Never continue a
   `ToolEventStream`.
6. Settlement indexes in snapshots stay PR-041/PR-048. PR-043 only
   reads `tool_settlements` / `completion_identities` and the restored
   `ActiveToolBatch` (source order + completed subset). That is the
   “persist source-order batch metadata and completed subset state”
   principal change: prove the existing kernel projection, do not add
   fields.
7. Cancel-during-suspended-tool is PR-045. Recheck cancellation and
   deadline before settlement; do not add that matrix. Existing
   synthetic cancellation/failure closures (`TOOL_CANCELLED`,
   `TOOL_DEADLINE_EXCEEDED`, unknown-tool, invalid-args) stay the
   framework-authored results. Do not invent a new synthetic family or
   fabricate success/tool-produced output.
8. `CommitCoordinator::recover` stays replay-only. `Toolset::reconcile`
   runs on `RunTaskOwner::spawn_with_model_and_tools` after timer and
   model resume, **after** the combined dispatcher is installed, and
   before the handle is published. Tests may call
   `tool_resume_action(recovered.state(), effect_id)` for the
   documented action.
9. Do not reuse `model_resume_action` or `context_resume_action`.
   Mirror the pattern only. Tool classification is **per call**, not
   per run phase: one deferred sibling must not force `WaitExternal`
   on a still-direct sibling.
10. Agent does not grow a public resume API. Proofs use `recover` +
    `spawn_with_model_and_tools`, same as the PR-042 persisted-model
    restart tests.
11. Calculator and filesystem `reconcile` stay default `Unknown`.
    Calculator is `ReadOnly` + `SafeToRetry` (retry-allowed).
    `filesystem_write` is `IdempotentWrite` + `IdempotentWithKey`
    (retry-allowed). `filesystem_edit` is `NonIdempotentWrite` +
    `AtMostOnce` (not retry-allowed). That is ADR-013 tool evidence,
    not a retrieve/reconcile client.
12. Python/WASM must not grow a reconcile surface.
13. `ToolFailurePolicy` is not an uncertainty escape hatch. It applies
    to live tool results. “Unless policy says otherwise” in the
    Implementation Plan is `retry_allowed` from committed
    `RetrySafety` plus catalog `SideEffectClass`.
14. Effect IDs are already on `ToolCallContext.run.effect_id` and
    `ReconcileContext.run.effect_id`. “Expose effect IDs to tools”
    means keep that identity on call, reconcile, and retry, and prove
    tools can use it as the application idempotency key (FR-TLS-004).
    Do not add `EffectId` to `ValidatedToolCall` or the journal.

### Classifier

Add in `crates/finstack-ai-runtime/src/tool.rs` and export:

```rust
pub enum ToolResumeAction {
    NoOutstanding,
    UseRecorded,
    Reconcile,
    Retry,            // post-reconcile only
    WaitExternal,
    SuspendUncertain,
}

pub fn tool_resume_action(state: &KernelState, effect_id: EffectId) -> ToolResumeAction;
pub fn tool_retry_allowed(
    requested: &EffectRequested,
    spec: &ToolSpec,
) -> bool;
pub fn map_tool_reconcile_result(
    state: &KernelState,
    effect_id: EffectId,
    result: &ToolReconcileResult,
    retry_allowed: bool,
) -> ToolResumeAction;
```

First-pass (journal only) never returns `Retry`.

Per-call first-pass:

| Journal shape | Action |
| --- | --- |
| no active batch / no matching call | NoOutstanding |
| `Settled` / `Buffered` / `tool_settlements` hit | UseRecorded |
| `SyntheticClosure` plan | UseRecorded (never `call` / `reconcile`) |
| `Undispatched` | NoOutstanding (not a resume target) |
| `Requested { deferred: None }` | Reconcile |
| `Requested { deferred: Some }` + `CallbackOnly` / `ExternalWorkflow` | WaitExternal |
| `Requested { deferred: Some }` + `Poll` / `CallbackOrPoll` | Reconcile |

`Undispatched` later-group calls get `EffectRequested` from a later
commit (after the current group settles). If that commit happens
during resume, the **already installed** dispatcher runs `call()`
once. Do not reconcile them.

```text
retry_allowed =
    requested.retry_safety() ∈ {SafeToRetry, IdempotentWithKey}
    AND spec.side_effect ∈ {ReadOnly, IdempotentWrite}
```

`RetrySafety::AtMostOnce` / `Unknown` ⇒ `retry_allowed = false`.
`SideEffectClass::NonIdempotentWrite` ⇒ `retry_allowed = false`.
Missing catalog entry ⇒ `SuspendUncertain`.
`NonRepeatable` overrides `retry_allowed`.

Do not invent a durable per-tool attempt counter. `ToolDispatchSeed.attempt`
stays `1` on first dispatch and on resume (existing
`tool_dispatch_seed`). `EffectId` is the idempotency key.
`pending_tool_seeds()` may use `state.accepted_at` for
`requested_at`, matching `pending_model_seed`. Recheck the absolute
deadline before settlement.

### Batch resume procedure

`resume_pending_tool_effects` walks `ActiveToolBatch.calls` in
**source order**. Classify every outstanding `Execute` + `Requested`
call first. Then:

1. `reconcile()` each first-pass `Reconcile` on the owning
   `ResolvedTool.toolset` (`catalog.by_id(&call.tool_id)`).
2. Map every result. If **any** call is `SuspendUncertain` or
   `reconcile()` errors, return that action, dispatch **no**
   `call()`, and submit **no** settlement.
3. Apply `Completed` / `Deferred` / `StillRunning` durable submits in
   source order through the existing tool settlement path.
4. Collect `Retry` seeds and return `Retry` so spawn can
   `ToolDispatcher::resume_call` / host `resume_call` (empty `active`
   map after crash).

Aggregate action for spawn:

- any `SuspendUncertain` → `SuspendUncertain` (wins)
- else any `Retry` → `Retry` (also fine if siblings `WaitExternal`)
- else any `WaitExternal` → `WaitExternal`
- else all recorded / none outstanding → `UseRecorded` / `NoOutstanding`

A03 code string: `tool_reconciliation_unsupported`. Spawn returns
`RunHandleError` with that code. Phase stays `AwaitingTools` /
`AwaitingExternal`; no new kernel input.

Install `RuntimeDispatcher::with_tools` **before** tool-resume
submits. Settling the last call of a group can emit `ExecuteEffect`
for the next group; without a dispatcher, `submit` faults
`effect_driver_unavailable`. Keep PR-042 model-resume order (model
resume may stay before install). Insert tool resume **after** install
and **before** the worker is spawned / the handle is published.

Host twin: same helper after `HostDispatcher` install.

### Reconcile result → documented action

`awaiting_external` is **that call’s** `deferred.is_some()`, not the
run phase.

| Result | Condition | Action | Durable submit |
| --- | --- | --- | --- |
| no matching call / settlement hit | any | UseRecorded / NoOutstanding | none |
| `Completed(result)` | direct Requested | settle direct | `ToolBatchSettled(Completed)` |
| `Completed(result)` | that call deferred | settle external | `ExternalEffectCompleted` |
| `Completed` equal indexed | settled | UseRecorded | none |
| `Completed` unequal indexed | settled | FailClosed | `RecordExternalCommandRejected` |
| `Deferred` / `StillRunning` | direct Requested | EnsureDeferred then WaitExternal | `ToolBatchSettled(Deferred)` first |
| `Deferred` / `StillRunning` | deferred, same handle | WaitExternal | none |
| `Deferred` / `StillRunning` | deferred, other handle | FailClosed | `RecordExternalCommandRejected` |
| `NotStarted` / `RetrySafe` | direct, no deferred | Retry | re-`call()` same identity |
| `Unknown` | retry_allowed | Retry (at-least-once, not exactly-once) | same |
| `Unknown` | !retry_allowed | SuspendUncertain (A03) | none; no `call()` |
| `NonRepeatable` | any | SuspendUncertain | none |
| `NotStarted` / `RetrySafe` | that call deferred | SuspendUncertain | do not re-`call()` |

`process_tool_result` must keep the existing early-return when the
call is already deferred. AwaitingExternal `Completed` uses the
external settle path.

### Crash matrix (in-process only)

Not OS kill. Not PR-040 sqlite process-kill. Not PR-048 full prefix
matrix. Tools: `enable_manual_drive`, `ScriptedToolset` `Block` +
drop owner, `CommitCoordinator::recover`, respawn
`RunTaskOwner::spawn_with_model_and_tools`. Store:
`MemoryJournalStore`.

| Row | Crash | After recover | First-pass | Scripted reconcile | After spawn |
| --- | --- | --- | --- | --- | --- |
| 1 Unstarted | manual_drive pause on Execute; drop | AwaitingTools, `call_count==0` | Reconcile | `NotStarted` | Retry; one `call()` same `EffectId` |
| 2 In-flight | Block; drop | AwaitingTools | Reconcile | `StillRunning` | EnsureDeferred; no second `call()` |
| 2b In-flight retryable | same | AwaitingTools | Reconcile | `Unknown` + retry_allowed | Retry same `EffectId` (A02) |
| 3 Completed-uncommitted | journal = EffectRequested only | AwaitingTools | Reconcile | `Completed` | settle direct; no `call()` |
| 4 Deferred | live Deferred committed; drop | AwaitingExternal | Reconcile or WaitExternal | same handle or `Completed` | wait or settle external |
| 5 Non-resumable | NonIdempotentWrite or AtMostOnce | AwaitingTools | Reconcile | `Unknown` / `NonRepeatable` | SuspendUncertain; no `call()` |
| 6 A02 recorded | ToolCallSettled committed; drop | that call Settled | UseRecorded | must not run for that id | never `call()` that `EffectId` |
| 7 Completed subset | two-call batch; call 0 settled, call 1 in-flight; drop | source order preserved; call 0 Settled | call 0 UseRecorded; call 1 Reconcile | call 1 `Unknown` + retry_allowed | never `call()` call 0; retry call 1; A04 source-ordered results |
| 8 A05 dup | deferred tool; equal completion twice | indexed | UseRecorded | n/a | idempotent |
| 9 A05 conflict | deferred tool; unequal completion | unchanged | n/a | n/a | fail-closed + rejection record |

Row 3 is reconcile-reported completion, not a live-stream race.
Row 7 is the batch-specific A01/A04 proof.

### Implementation pitfalls (from PR-042; apply tool analogs)

1. Crash tests must `abort()` the driving task, `await` it, then
   `drop(permit)`. Do not leave a task blocked on a oneshot when
   dropping the owner.
2. Native `EventHubHandle::publish` is oneshot-backed; spawn
   `event_task.run()` **before** any resume `submit`.
3. External `Completed` must reuse `allocate_tool_settlement` IDs when
   submitting `ExternalEffectCompleted`.
4. `process_tool_result` early-returns if the call is already
   deferred. AwaitingExternal `Completed` must use the external settle
   path.
5. Public spawn wrappers `Box::pin` inner fns for clippy
   `large_futures`.
6. Respawn clocks must stay **before** the fixture deadline (PR-042
   used `2_500`/`2_600`, not `20_000`).
7. Poisoned ScriptedToolset mutexes use
   `unwrap_or_else(PoisonError::into_inner)`.
8. Install the tool dispatcher before resume settlements that can
   emit the next group’s `ExecuteEffect`.

## Files

Create later (implementation / candidate, not this planning step):

- `docs/implementation/artifacts/pr-043/README.md`
- `docs/implementation/artifacts/pr-043/candidate-validation.txt`
- `docs/implementation/artifacts/pr-043/security-review.txt`

Modify:

- `crates/finstack-ai-runtime/src/tool.rs` — `ToolResumeAction`,
  `tool_resume_action`, `tool_retry_allowed`,
  `map_tool_reconcile_result`, `TOOL_RECONCILIATION_UNSUPPORTED`;
  rustdoc on `ToolCallContext` / `ReconcileContext` that `effect_id`
  is the application idempotency key
- `crates/finstack-ai-runtime/src/lib.rs` — exports
- `crates/finstack-ai-runtime/src/settlement.rs` —
  `resume_pending_tool_effects`
- `crates/finstack-ai-runtime/src/coordinator.rs` —
  `pending_tool_seeds()` (source-ordered `Requested { deferred: None }`
  Execute calls)
- `crates/finstack-ai-runtime/src/tool_runtime.rs` —
  `ToolDispatcher::resume_call` (empty `active` map after crash)
- `crates/finstack-ai-runtime/src/task.rs` — install dispatcher, then
  tool resume, on `spawn_with_model_and_tools`
- `crates/finstack-ai-runtime/src/host_task.rs` — same helper +
  `resume_call`
- `crates/finstack-ai-test/src/scripted_toolset.rs` — optional
  reconcile queue + `reconcile_count` + `call_count`; default stays
  `Unknown`
- `crates/finstack-ai-test/tests/tool_port.rs` — A01–A05 fixtures
- `extensions/toolsets/finstack-ai-tools-calculator` and
  `extensions/toolsets/finstack-ai-tools-filesystem` — optional
  explicit `Unknown` plus one offline policy test each (calculator
  retry-allowed; `filesystem_edit` not retry-allowed)

Do not modify: kernel reducer/records, ingress, Python/WASM adapters,
sqlite store, snapshot DTO, `docs/planning/`.

## Proposed tasks

Create ledger rows only at admit. Summaries are implementation-specific.

1. Tracking — confirm deps, ADRs, TM-10/TM-14, exclusions, branch.
2. Classifier — `tool_resume_action` table tests on synthetic
   `KernelState` (single call, completed subset, deferred sibling).
3. Reconcile wiring — spawn calls `Toolset::reconcile`; map
   `Completed` / `Deferred` / `StillRunning` through
   `build_tool_settlement`.
4. Retry / suspend policies — same-identity re-`call()` vs
   `SuspendUncertain`; never re-`call()` a deferred or settled
   effect; `SideEffectClass` gate.
5. Crash matrix — A01 rows 1–5 and row 7 in `tool_port.rs`.
6. Completions — A02 recorded + A04 source order + A05
   dup/conflict; same protocol as models.
7. Scripted + first-party evidence — ScriptedToolset queue;
   calculator/filesystem `Unknown` policy tests.
8. Validation — A01–A05, TM-10/TM-14 review, graph checks; stop
   before G5.

## Validation (when authorized)

Focused:

```text
cargo test -p finstack-ai-runtime --features native-tokio --offline --locked tool_resume
cargo test -p finstack-ai-test --test tool_port --offline --locked
cargo test -p finstack-ai-tools-calculator --offline --locked
cargo test -p finstack-ai-tools-filesystem --offline --locked
cargo test -p finstack-ai-kernel --offline --locked tool
```

Candidate:

```text
cargo fmt --all --check
cargo clippy --workspace --all-targets --offline --locked -- -D warnings
cargo test --workspace --offline --locked
cargo test -p finstack-ai-test --test journal_v1 --offline --locked
cargo tree -e normal --locked --offline -p finstack-ai-runtime -p finstack-ai -p finstack-ai-kernel
uv run --no-project python tools/wasm_package/check.py graph
```

Do not require `mise run ci`, pytest, sqlite kill, or browser tests to
close A01–A05. Do not infer G5.

## Explicit exclusions

No seventh port. Runtime and SDK stay protocol-free. No default
Agent/Python/WASM sqlite wiring. No `SnapshotWritten` from `decide`.
No claim of exactly-once external side effects. No
interactions/approval (PR-044). No cancel-during-suspended-tool
matrix (PR-045). No conversation tree or multi-lane APIs
(PR-046/047). No full crash-prefix, migrators, IndexedDB v1,
settlement-index rebuild, `0.0.3`, or G5 (PR-048). No Python/WASM
reconcile API. No generic `ReconcileResult`. No new journal fields.
No fabricated successful tool results. No silent ADR-013
Implemented. No reuse of the closed PR-042 envelope.
