# PR-042 execution plan

Date: 2026-08-14
Owner: me@jeickmeier.com
Branch: `codex/pr-042-model-reconciliation` (proposed; create only after a new envelope)
Baseline: local `main` after PR-041 closeout `34bf153e90682941d02f69717437c833ac8b6233`
Plan baseline: documentation pack v0.20 / PLAN-0.18

This file is the execution contract for PR-042. It does **not** authorize
coding, branch creation, commit, merge, or hosted actions. The closed
PR-041 envelope is not reused. `continue` does not start this PR.

## Admission

- Phase 6 entrance is `Passed` (3/3). Do not re-record `PH6-E-entrance-*`.
- PR-039 is `Done` at `64c54e767f53faac240ab92c19a8447264e82ff8`.
- PR-040 is `Done` at `dbd10d35b223288666b2fdc0e13d03f48b5b97c3`.
- PR-041 is `Done` at `8a84293264291dbe158fae1b4e5dedcc30c74061` (closeout `34bf153e90682941d02f69717437c833ac8b6233`).
- ADR-025 is Accepted / Partial (runtime lifecycle remains). ADR-033 is
  Accepted / Not started. Implement both; do not reinterpret. ADR-034
  stays PR-018/PR-048.
- PR-042 is the only active logical PR once admitted. PR-043+ stays excluded.
- No Implementation Plan section 6.3 ADR trigger applies if the work stays
  inside at-least-once plus idempotency/reconciliation (no stronger
  exactly-once claim).
- Threat Model section 18 is triggered (effect recovery semantics; TM-10,
  also TM-14). Complete the review before merge.
- Dependencies: PR-039–PR-041 and the Model port. Both are present.

## Acceptance mapping

- PR-042-A01: in-process crash fixtures at every model-effect boundary
  restore to a documented `ModelResumeAction` (see crash matrix).
- PR-042-A02: after a committed model `EffectCompleted` / settlement
  index hit, spawn/recover never calls `Model::request` again for that
  `EffectId`.
- PR-042-A03: `NotStarted` / `RetrySafe` / `Unknown`+retry-allowed
  re-dispatch uses the original `EffectId`, `ModelRequestId`, canonical
  draft bytes, and `input_digest`.
- PR-042-A04: `Unknown` or `NonRepeatable` when retry is not allowed
  suspends with `model_reconciliation_unsupported` and does not
  `request()` or fabricate `EffectCompleted`.
- PR-042-A05: equal external completion is idempotent; a conflicting
  completion fails closed and appends `RecordExternalCommandRejected`.

## Execution envelope

Authorized 2026-08-14 by `implement the plan` after the named example
envelope:

`mode=integrated; target=main; local branch/commit/merge authorized; external actions=feature-branch push plus hosted Linux CI/security; hosted PR/merge, npm publish, tag, and G5 inference prohibited`

Baseline: local `main` at `34bf153e90682941d02f69717437c833ac8b6233`.
Branch: `codex/pr-042-model-reconciliation`. This envelope is new and
does not reuse the closed PR-041 envelope.

## Locked design

Do not invent `DeferredHandle`, `SuspendedOperation`, a generic
`ReconcileResult`/`EffectOutput`, a seventh port, or new journal fields.

1. TDD §23.2 `StillRunning(DeferredHandle)` is the existing
   `ModelReconcileResult::StillRunning(ModelDeferral)`.
   `Deferred(ModelDeferral)` is the same recovery action. Both require a
   committed `EffectDeferred` before wait.
2. TDD §23.1 `SuspendedOperation` is `PendingModelEffect` on
   `KernelState` plus a pure `ModelResumeAction`. No public DTO.
3. Architecture §10.3 is the durable classifier. The five PR-042
   principal states are crash-injection rows. Unstarted, in-flight, and
   completed-but-uncommitted are journal-identical (`AwaitingModel` +
   pending, no deferred) and are §10.3 *possibly started*. Reconcile
   distinguishes them. §10.3 *completed* is A02.
4. `Completed(EffectOutput)` is `Completed(ModelResponse)`. Reuse
   `build_settlement`. `AwaitingModel` → `ModelSettled`.
   `AwaitingExternal` → `ExternalEffectCompleted` with
   `assistant_message = Some`.
5. Ignore `resumable_stream`. Never continue a provider stream after
   crash.
6. Settlement indexes in snapshots stay PR-041/PR-048. PR-042 only
   reads `model_settlements` / `completion_identities`.
7. Cancel-during-suspended-model is PR-045. Recheck cancellation and
   deadline before settlement; do not add that matrix.
8. `CommitCoordinator::recover` stays replay-only (timer analog).
   `Model::reconcile` runs on `RunTaskOwner::spawn_with_model*` after
   warmup, before the handle is published. Tests call
   `model_resume_action(recovered.state())` for the documented action.
9. Do not reuse `context_resume_action` (model `component` is usually
   `None`). Mirror the pattern only.
10. Agent does not grow a public resume API. Proofs use `recover` +
    `spawn_with_model`, same as the persisted-timer restart test.
11. OpenAI-compatible `reconcile` stays `Unknown`
    (`idempotent_requests=false`). That is ADR-033 provider evidence,
    not a retrieve/background client.
12. Python/WASM must not grow a reconcile surface. Ingress still
    rejects model-assistant decode (`model_response_decoder_unavailable`).

### Classifier

Add in `crates/finstack-ai-runtime/src/model.rs` and export:

```rust
pub enum ModelResumeAction {
    NoOutstanding,
    UseRecorded,
    Reconcile,
    Retry,            // post-reconcile only
    WaitExternal,
    SuspendUncertain,
}

pub fn model_resume_action(state: &KernelState) -> ModelResumeAction;
```

First-pass (journal only) never returns `Retry`. `AwaitingModel` +
pending + no settlement → `Reconcile`. `AwaitingExternal` +
`CallbackOnly` → `WaitExternal`. `AwaitingExternal` + `Poll` /
`CallbackOrPoll` → `Reconcile`. Already settled → `UseRecorded` /
`NoOutstanding`.

```text
retry_allowed =
    requested.retry_safety() ∈ {SafeToRetry, IdempotentWithKey}
    AND model.capabilities(draft.model).idempotent_requests
```

`RetrySafety::AtMostOnce` / `Unknown` ⇒ `retry_allowed = false`.
`NonRepeatable` overrides `retry_allowed`.

### Reconcile result → documented action

| Result | Condition | Action | Durable submit |
| --- | --- | --- | --- |
| no pending / settlement hit | any | UseRecorded / NoOutstanding | none |
| `Completed(response)` | AwaitingModel | SettleDirect | `ModelSettled(Completed)` |
| `Completed(response)` | AwaitingExternal | SettleExternal | `ExternalEffectCompleted` |
| `Completed` equal indexed | settled | UseRecorded | none |
| `Completed` unequal indexed | settled | FailClosed | `RecordExternalCommandRejected` |
| `Deferred` / `StillRunning` | AwaitingModel | EnsureDeferred then WaitExternal | `ModelSettled(Deferred)` first |
| `Deferred` / `StillRunning` | AwaitingExternal, same handle | WaitExternal | none |
| `Deferred` / `StillRunning` | AwaitingExternal, other handle | FailClosed | `RecordExternalCommandRejected` |
| `NotStarted` / `RetrySafe` | AwaitingModel, no deferred | Retry | re-`request()` same identity |
| `Unknown` | retry_allowed | Retry (at-least-once, not exactly-once) | same |
| `Unknown` | !retry_allowed | SuspendUncertain (A04) | none; no `request()` |
| `NonRepeatable` | any | SuspendUncertain | none |
| `NotStarted` / `RetrySafe` | AwaitingExternal | SuspendUncertain | do not re-`request()` |

A04 code string: `model_reconciliation_unsupported`. Do not emit a new
`RunSuspended` record (no kernel input). Phase stays
`AwaitingModel` / `AwaitingExternal`; spawn must not `request()`.

### Crash matrix (in-process only)

Not OS kill. Not PR-040 sqlite process-kill. Not PR-048 full prefix
matrix. Tools: `enable_manual_drive`, `ScriptedModel` `Block` +
`drop(owner)`, `CommitCoordinator::recover`, respawn
`RunTaskOwner::spawn_with_model`. Store: `MemoryJournalStore`.

| Row | Crash | After recover | First-pass | Scripted reconcile | After spawn |
| --- | --- | --- | --- | --- | --- |
| 1 Unstarted | manual_drive pause on Execute; drop | AwaitingModel, `request_count==0` | Reconcile | `NotStarted` | Retry; one `request()` |
| 2 In-flight | Block; drop | AwaitingModel | Reconcile | `StillRunning` | EnsureDeferred; no second `request()` |
| 2b In-flight retryable | same | AwaitingModel | Reconcile | `Unknown` + retry_allowed | Retry same `EffectId` (A03) |
| 3 Completed-uncommitted | journal = EffectRequested only | AwaitingModel | Reconcile | `Completed` | SettleDirect; no `request()` |
| 4 Deferred | live Deferred committed; drop | AwaitingExternal | Reconcile or WaitExternal | same handle or `Completed` | wait or SettleExternal |
| 5 Non-resumable | AtMostOnce or !idempotent | AwaitingModel | Reconcile | `Unknown` / `NonRepeatable` | SuspendUncertain; no `request()` |
| 6 A02 | ModelSettled committed; drop | pending None | NoOutstanding | must not run | never `request()` |
| 7 A05 dup | deferred; equal completion twice | indexed | UseRecorded | n/a | idempotent |
| 8 A05 conflict | deferred; unequal completion | unchanged | n/a | n/a | fail-closed + rejection record |

Row 3 is reconcile-reported completion, not a live-stream race.

## Files

Create later (implementation / candidate, not this planning step):

- `docs/implementation/artifacts/pr-042/README.md`
- `docs/implementation/artifacts/pr-042/candidate-validation.txt`
- `docs/implementation/artifacts/pr-042/security-review.txt`

Modify:

- `crates/finstack-ai-runtime/src/model.rs` — `ModelResumeAction`,
  `model_resume_action`, `map_model_reconcile_result`,
  `MODEL_RECONCILIATION_UNSUPPORTED`
- `crates/finstack-ai-runtime/src/lib.rs` — exports
- `crates/finstack-ai-runtime/src/settlement.rs` —
  `resume_pending_model_effect`
- `crates/finstack-ai-runtime/src/model_runtime.rs` —
  `ModelDispatcher::resume_request` (empty `active` map after crash)
- `crates/finstack-ai-runtime/src/task.rs` — call resume after timer
  resume on both spawn paths
- `crates/finstack-ai-runtime/src/host_task.rs` — same helper
- `crates/finstack-ai-test/src/scripted_model.rs` — optional reconcile
  queue + `reconcile_count`; default stays `Unknown`
- `crates/finstack-ai-test/tests/model_port.rs` — A01–A05 fixtures
- `extensions/providers/finstack-ai-provider-openai-compatible` —
  optional explicit `Unknown` + one offline test

Do not modify: kernel reducer/records, ingress model-assistant decoder,
Python/WASM adapters, sqlite store, snapshot DTO, `docs/planning/`.

## Proposed tasks

Create ledger rows only at admit. Summaries are implementation-specific.

1. Tracking — confirm deps, ADRs, TM-10/TM-14, exclusions, branch.
2. Classifier — `model_resume_action` table tests on synthetic
   `KernelState`.
3. Reconcile wiring — spawn calls `Model::reconcile`; map
   `Completed` / `Deferred` / `StillRunning` through `build_settlement`.
4. Retry / suspend policies — same-identity re-dispatch vs
   `SuspendUncertain`; never re-`request()` a deferred effect.
5. Crash matrix — A01 rows 1–5 in `model_port.rs`.
6. Completions — A02 + A05; ingress stays decoder-unavailable.
7. Provider + scripted evidence — ScriptedModel queue; OpenAI
   `Unknown`.
8. Validation — A01–A05, TM-10/TM-14 review, graph checks; stop
   before G5.

## Validation (when authorized)

Focused:

```text
cargo test -p finstack-ai-runtime --features native-tokio --offline --locked model_resume
cargo test -p finstack-ai-test --test model_port --offline --locked
cargo test -p finstack-ai-provider-openai-compatible --offline --locked
cargo test -p finstack-ai-kernel --test model_only_reducer --offline --locked settlements
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
No universal provider stream resumption. No tool-effect reconciliation
(PR-043). No interactions/approval (PR-044). No durable
cancellation/retry/timer restore matrix (PR-045). No conversation tree
or multi-lane APIs (PR-046/047). No full crash-prefix, migrators,
IndexedDB v1, settlement-index rebuild, `0.0.3`, or G5 (PR-048). No
OpenAI background retrieve. No Python/WASM reconcile API. No silent
exactly-once.
