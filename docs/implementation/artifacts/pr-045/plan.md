# PR-045 execution plan

Date: 2026-08-14
Owner: me@jeickmeier.com
Branch (when admitted): `codex/pr-045-durable-cancel-timers`
Baseline (when admitted): PR-044 review head `a98e230e75dc3e621a55746f19b662cee9a7f2c9`
  (implementation candidate `e56d6f1d0638986d1201f2b901f372d64d01d062`)
Plan baseline: documentation pack v0.20 / PLAN-0.18 / Implementation Plan SHA-256
`555a150fa9eaa2de39342eabdfd3d050b19628d735adbf498a9d75fcbc1102a4`

This file is the execution contract for PR-045. It does **not** authorize
coding, branch creation, commit, merge, or hosted actions. The open
PR-043 and PR-044 envelopes are not reused. `continue` does not start
this PR. Planning does not admit PR-045.

## Admission

- Phase 6 entrance is `Passed` (3/3). Do not re-record `PH6-E-entrance-*`.
- PR-039 is `Done` at `64c54e767f53faac240ab92c19a8447264e82ff8`.
- PR-040 is `Done` at `dbd10d35b223288666b2fdc0e13d03f48b5b97c3`.
- PR-041 is `Done` at `8a84293264291dbe158fae1b4e5dedcc30c74061`.
- PR-042 is `Done` at local merge `4d627711632c771d733b689e1325b2d9679ee317`.
- PR-043 is `In review` on `codex/pr-043-tool-reconciliation` (candidate
  `86f71c8fd47c0c6d721f9ae36df5c0ba90b22025`).
- PR-044 is `In review` on `codex/pr-044-typed-interactions` (candidate
  `e56d6f1d0638986d1201f2b901f372d64d01d062`, evidence
  `a98e230e75dc3e621a55746f19b662cee9a7f2c9`). PR-043 and PR-044 are
  the **active** logical PRs. Admitted 2026-08-14 under the authorized
  stacked envelope; PR-043 and PR-044 stay `In review`.
- PR-011 is `Done` at `01380ead5c5ca7b9e7d28d681d84719c9bf0279e`. It
  owns the kernel cancel/retry/timer **vocabulary** and the §22.5
  race table. Do not redefine those records.
- PR-019 is `Done` at `bf748d5aa1ee7ffa11cf9ac3ab2dea399034dd0b`. It
  owns the in-process cancellation-token tree, `Clock` /
  `MonotonicDeadline`, and owner-shutdown retry-timer fixtures. It
  explicitly excluded a durable database or workflow-engine clock.
- ADR-004 stays Accepted / Partial. Timer/cancel commit-before-effect
  on restore is additional Partial evidence. Do **not** mark
  Implemented (PR-048 / G5 remain).
- ADR-013 stays Accepted / Not started / Missing. It is not in this
  PR’s mapped delivery. Do not silently mark Implemented or Partial.
- ADR-025 stays Accepted / Partial. Cancel-during-deferred restore
  continues the generic-effect path. Do **not** mark Verified.
  PR-048 remains.
- ADR-033 stays Implemented / Verified for **model** interruption.
  Do not reopen it.
- ADR-027 stays Partial (PR-044 candidate). This PR only adds
  run-level cancel while `AwaitingInteraction`. Do not mark
  Implemented.
- No Implementation Plan section 6.3 ADR trigger applies if the work
  stays inside the existing six ports, existing PR-008/PR-011 record
  families, and at-least-once plus idempotency. Do not add a seventh
  port. Do not add `TimerId` / `TimerRequested` / `CancellationAcknowledged`.
  Do not claim exactly-once timer firing or cancellation.
- Do **not** add kernel-state v7. Cancel/retry/timer projections
  already live on v3 (`cancellation`, `retry`). Interaction fields
  stay v6. Histories with no new control records keep their existing
  `state_version` / `state_hash`.
- Threat Model section 18 is triggered (cancellation/deadline
  semantics; primary **TM-14**; SEC-INV-002/003/004/007/008;
  secondary TM-10). Complete the review before merge. A threat-model
  review is not an ADR and does not stop coding after admit.
- Dependencies: PR-039–PR-044. PR-044 must not still be the sole
  active PR at admit.
- Phase 6 exit “pending effects reconcile deterministically” is
  **partial evidence** from this PR (timer/cancel restore). Do not
  infer G5. G5 still waits for PR-048.

## Acceptance mapping

- PR-045-A01: a retry scheduled before shutdown fires once after
  restart according to policy. Pattern: park on `RunPhase::Sleeping`
  with a committed `RetryScheduled` + timer `EffectRequested`, abort
  the driving task, drop the owner, `CommitCoordinator::recover`,
  respawn. The same `timer_effect_id` / `due_at` / attempt fires
  exactly one `TimerFired` and resumes at `PreparingContext` without
  resetting `RetryState.attempts`. A second respawn is idempotent
  (`UseRecorded` / equal `TimerFired`). `max_retries` still binds.
  PR-019’s owner-replacement fixture is the floor, not this
  acceptance.
- PR-045-A02: cancellation during a suspended interaction or
  deferred model/tool effect closes predictably. “Application
  effect” in the Implementation Plan maps to host-deferred
  model/tool (`AwaitingExternal`), not a new `EffectKind::Application`.
  Run-level `CancelRequested` while `AwaitingInteraction`,
  `AwaitingExternal` (model or tool), or `Sleeping` writes
  `CancellationRequested` first, then reconciles. Framework-authored
  cancelled closures carry no tool output or success claim.
  Uncertainty suspends (`cancellation_uncertain`). Duplicate equal
  cancel is idempotent; a late conflicting privileged completion
  fails closed and is audited.
- PR-045-A03: overdue timer replay is deterministic and bounded.
  Restart recomputes the monotonic wait from the persisted wall
  `due_at` and the original schedule timestamp. An already-due timer
  fires once through `KernelInput::TimerFired` and increments
  `TimerDiagnostics.already_due`. Backward wall jumps clamp; they
  never extend an active wait. No polling loop. No unbounded
  `JoinSet` growth.
- PR-045-A04: timer identities and schedule records pass
  compatibility fixtures. Timer identity is the existing
  `EffectId` on `RetryScheduled.timer_effect_id` /
  `TimerFired.effect_id`. Add public-rust-api `kernel-input`
  round-trips for `TimerFired` and `CancellationReconciled`. Do not
  invent `TimerId`. Do not rewrite v1–v6 journal or kernel-state
  hashes.

## Execution envelope

Authorized 2026-08-14 by `implement the plan`.

Local branch and commit are authorized. Merge to `main`, feature-branch
push, hosted PR/merge, npm publish, tag, and G5 inference are not named
and remain prohibited. The open PR-043 and PR-044 envelopes are not
reused.

`mode=stacked` (local implementation on the PR-044 review head);
`target` unset; `external actions=none`.

PR-043 and PR-044 remain `In review` and are not marked `Done`. This
PR is the active implementation slice; it does not close or merge
PR-043 or PR-044.

Baseline: `a98e230e75dc3e621a55746f19b662cee9a7f2c9`.
Branch: `codex/pr-045-durable-cancel-timers`.

## Locked design

Do not invent a seventh port, a `Timer` port trait, `TimerId`,
`TimerRequested`, `CancellationAcknowledged`,
`ExternalEffectOutcome::Cancelled`, `EffectKind::Application`, a
new `RunPhase`, new `RecordBody` families, or kernel-state v7.
Activate the existing PR-011/PR-019/PR-044 surface.

### 1. Outstanding effects must include interaction and retry timer

Today `outstanding_requested_effects` and
`apply.rs` `CancellationRequested` only collect
`pending_model_effect` and requested tool-batch calls
(`decide.rs` ~1704, `apply.rs` ~1129). That omits:

- `pending_interaction.request.effect_id()` while
  `AwaitingInteraction`
- `retry.pending.timer_effect_id` while `Sleeping`

TDD §22.6.3: PR-011 accepts cancellation for an already represented
outstanding interaction effect; PR-044 now represents it. A cancel
from `Sleeping` with an empty outstanding set can `RunCancelled`
while the timer task is still sleeping — that is the bug this PR
closes.

Keep one helper used by both decide and apply. Sort + dedup
`EffectId`s. `CancelRequested` stays one `CancellationRequested`
record plus `PostCommitAction::CancelEffect` for each outstanding
id. Do not `ExecuteEffect` the interaction effect.

### 2. CancelRequested is valid from AwaitingInteraction and Sleeping

`decide_cancel` already admits any non-terminal run. Extend the
PR-011 phase matrix (`matrix.rs`) with `AwaitingInteraction` and
`Sleeping`. Do not add a new input.

After `CancellationRequested`, `reject_busy` continues to reject
`InteractionSettled` (`interaction.rs` ~317). External resolve /
expire after cancel won is a late privileged input: fail closed
(`invalid_phase_input` or `RecordExternalCommandRejected` when the
locator is known). Do not expire-if-due on spawn when
`state.cancellation.is_some()`.

### 3. Reconciliation emits the interaction cancel pair

TDD §12.4: a cancel batch pairs `InteractionCancelled` with
`EffectCancelled`. `decide_reconciliation` today auto-emits
`EffectCancelled` only for `pending_model_effect` (~1581). When
`newly_cancelled` contains the pending interaction effect, emit
`InteractionCancelled` then `EffectCancelled` in that order, then
`CancellationReconciled`, then `RunCancelled` / `RunSuspended`.

`apply_interaction_effect_terminal` currently restores
`prior_phase`. That violates v3 validation if
`cancellation.is_some()` and phase becomes `BeforeToolBatch`
(`state/mod.rs` ~692). When `state.cancellation.is_some()`, clear
`pending_interaction` and **keep** `RunPhase::Cancelling` (or
`Suspended` / `Cancelled` if those records are in the same batch).
Do not restore `prior_phase` under an active cancellation.

Runtime `allocate_for_runtime_input` must count the extra
interaction records/events when the next reconcile will close an
interaction effect. Reuse the existing interaction id; do not
allocate a new one.

### 4. TimerFired loses to CancellationRequested

`decide_timer_fired` does not consult `state.cancellation`. A late
fire after cancel would write `TimerFired` and move phase to
`PreparingContext` while `cancellation` is still set — apply would
fail validation.

If `state.cancellation.is_some()`, reject `TimerFired` with
`invalid_phase_input` (or treat it only as reconciliation
evidence; do **not** start a fresh cycle). The runtime then
classifies the timer effect as `cancelled_effects` (wait aborted)
or `completed_effects` only when an equal `TimerFired` is already
committed. `fired_at < due_at` stays `ConflictingSettlement`.
Equal duplicate `TimerFired` stays idempotent.

### 5. Spawn rechecks cancel and deadline before resume

PR-042/043/044 recheck on the active settlement path and
explicitly deferred this matrix. On `spawn_with_model*` /
`spawn_with_model_and_tools`, after recover and before
`Model::reconcile` / `Toolset::reconcile` / expire-if-due /
`resume_request` / `call()`:

1. If `state.cancellation.is_some()`, do not dispatch. Drive
   `CancelEffect` / `reconcile_cancelled_effect` for each
   outstanding id, including interaction and retry timer.
2. If the run deadline is due and no cancellation is in flight,
   keep the existing `fail_closed_on_run_deadline` path.
3. Expire-if-due runs only when `cancellation.is_none()`.

In-flight `CancelEffect` for a timer cancels the
`TimerDispatcher` child. For an interaction there is no running
task — skip dispatch and submit the reconciliation chunk.

Deferred model/tool with `Unknown` / `NonRepeatable` when retry
is not allowed: `uncertain_effects` → `RunSuspended
{ reason_code: "cancellation_uncertain" }`. Do not fabricate
`RunCancelled`. Do not fabricate a successful tool result.

### 6. Clock adapter — not a port

Keep `Clock` as an injected wall source (`id_generation.rs`).
Native default remains `SystemClock`. PR-045 adds a public
`ExternalClock` in `finstack-ai-runtime` that implements `Clock`:

- `now()` is the only required method
- tests call `jump(delta)` / `set(timestamp)` for forward and
  backward wall fixtures
- a durable workflow host constructs `ExternalClock` from its
  own clock and passes it into the runtime
- the kernel still never reads ambient time
- monotonic waits still use `MonotonicDeadline::from_persisted`
  and are never serialized
- this is not a seventh port and not a `Timer` trait

Do not persist a clock table. Do not add a retrieve client. Do
not wire `retry_backoff_with_jitter` into the kernel;
`due_at` stays `env.now.checked_add(backoff)` (TDD §22.6.1).
Jitter, if used, stays runtime-side when building a
`RetryDirective`.

### 7. Cross-run routing stays PR-046

TDD §22.6.3: “Actual cross-run routing remains PR-045/PR-046.”
This PR owns **same-run** cancel of deferred/interaction/timer
effects. Parent→child fan-out, lane APIs, and descendant
propagation services are PR-046/PR-047. Do not add a child-run
router.

## Cancel / timer restore matrix (in-process only)

Not OS kill. Not PR-040 sqlite process-kill. Not PR-048
crash-prefix. Store: `MemoryJournalStore`. Pattern: park, abort
driving tasks, `drop(permit)`, `CommitCoordinator::recover`,
respawn or submit cancel/reconcile.

| Row | Scenario | After recover / settle |
| --- | --- | --- |
| 1 Retry before shutdown | `Sleeping` + committed `RetryScheduled`; drop; respawn after `due_at` | one `TimerFired`; same `timer_effect_id` / attempt; `PreparingContext` (A01) |
| 2 Retry idempotent | second respawn after row 1 | no second fire; `timer_firings` hit (A01) |
| 3 Cancel while AwaitingInteraction | `CancelRequested` after approval request | `CancellationRequested`; interaction pair; `RunCancelled`; no protected `call()` (A02) |
| 4 Cancel while deferred model | `AwaitingExternal` + `CancelRequested` | original `EffectId` cancelled or `cancellation_uncertain`; no second `Model::request` (A02) |
| 5 Cancel while deferred tool | deferred tool + `CancelRequested` | synthetic cancelled closure; no `call()`; source order preserved (A02) |
| 6 Cancel while Sleeping | `CancelRequested` before timer due | timer in outstanding; `CancelEffect`; no `TimerFired`; `RunCancelled` (A02) |
| 7 Late TimerFired after cancel | fire in flight after `CancellationRequested` | rejected / not a fresh cycle; timer classified cancelled (A02) |
| 8 Late privileged completion | conflicting model/tool completion after cancel | fail closed + rejection/audit (A02) |
| 9 Duplicate cancel | equal initiator + reason twice | `Idempotent`; no second apply (A02) |
| 10 Overdue restart | respawn with `now >= due_at` | one fire; `already_due >= 1`; bounded (A03) |
| 11 Backward clock | wall jumps backward across restart | wait clamped; does not extend (A03) |
| 12 Uncertain deferred | `Unknown` without retry_allowed during cancel | `RunSuspended`; no fabricated success (A02) |

PR-044 `cancelled_while_waiting_never_dispatches` (`InteractionSettled::Cancelled`) stays as a floor. A02 requires the **run-level** `CancelRequested` row (row 3).

## Implementation pitfalls

1. Crash tests must `abort()` the driving task, await it, then
   `drop(permit)`.
2. Spawn `event_task.run()` before any resume/cancel `submit`.
3. Do not consume `stage_settlements` on cancel or timer fire.
4. Do not `ExecuteEffect` the interaction `effect_id`.
5. `apply_interaction_effect_terminal` must keep `Cancelling`
   when `cancellation.is_some()`.
6. `outstanding_requested_effects` and `CancellationRequested`
   apply must stay identical.
7. `decide_timer_fired` must not start a cycle after cancel won.
8. Expire-if-due must not run while cancelling.
9. Do not invent `TimerId`. Compatibility fixtures use `EffectId`.
10. Do not require `mise run test-lifecycle` — that task is gone.
    Use the cargo filters below.
11. Workspace sqlite `concurrent_readers_never_observe_a_torn_batch`
    can flake under parallel load; isolate and record honestly.

## Files

Create later (implementation / candidate, not this planning step):

- `docs/implementation/artifacts/pr-045/README.md`
- `docs/implementation/artifacts/pr-045/candidate-validation.txt`
- `docs/implementation/artifacts/pr-045/security-review.txt`

Modify when implementing:

- `crates/finstack-ai-kernel/src/reducer/decide.rs` —
  `outstanding_requested_effects`, `decide_reconciliation`
  interaction pair, `decide_timer_fired` vs cancel
- `crates/finstack-ai-kernel/src/reducer/apply.rs` —
  `CancellationRequested` outstanding set;
  `apply_interaction_effect_terminal` under cancel
- `crates/finstack-ai-kernel/tests/model_only_reducer/` —
  AwaitingInteraction/Sleeping cancel matrix; timer-vs-cancel
- `crates/finstack-ai-kernel/tests/model_only_reducer/matrix.rs` —
  add the two missing phases
- `crates/finstack-ai-runtime/src/timer_runtime.rs`,
  `time.rs`, `id_generation.rs` — injectable clock; overdue /
  backward-clock diagnostics
- `crates/finstack-ai-runtime/src/settlement.rs`,
  `task.rs`, `host_task.rs`, `coordinator.rs` — spawn recheck;
  CancelEffect for interaction/timer
- `crates/finstack-ai-runtime/src/lib.rs` — export the injectable
  clock if it becomes public
- `crates/finstack-ai-test/tests/model_port.rs` — A01 / A03
  recover+respawn (keep PR-019 floor)
- `crates/finstack-ai-test/tests/tool_port.rs` — A02 deferred-tool
  cancel
- `crates/finstack-ai-test/tests/interaction.rs` — A02 run-level
  cancel while waiting
- `fixtures/compatibility/public-rust-api/v1/kernel-input/` —
  `roundtrip--timer-fired.json`,
  `roundtrip--cancellation-reconciled.json`
- `crates/finstack-ai-test/tests/public_rust_api.rs` — corpus count

Do not modify: `docs/planning/`, sqlite store, snapshot DTO
rebuild, WASM/JS durable restart, WIT, UI, cron/scheduler,
PR-046/047 lane APIs, Python/WASM reconcile.

Optional thin Python: one `AgentRun.cancel()` while an approval
is pending, if it stays a handle test. Not required to close A02.

## Proposed tasks

Create ledger rows only at admit. Summaries are
implementation-specific.

1. Tracking — confirm PR-039–PR-044, PR-011/PR-019 boundaries,
   ADR-004/013/025/033, TM-14, exclusions, no branch until
   PR-044 is no longer the sole active PR.
2. Kernel outstanding set — failing tests first: cancel from
   `AwaitingInteraction` and `Sleeping`; interaction cancel pair;
   `TimerFired` rejected after cancel; apply keeps `Cancelling`.
3. Runtime spawn recheck — cancel/deadline before
   reconcile/expire/dispatch; `CancelEffect` for interaction and
   retry timer; skip expire-if-due while cancelling.
4. Injectable clock — public/test workflow clock; forward and
   backward wall-jump fixtures; no seventh port.
5. A01 retry-before-shutdown — recover + respawn; one fire;
   attempt preserved; second respawn idempotent.
6. A02 cancel-during-suspended matrix — rows 3–9 and 12;
   MemoryJournalStore; in-process only.
7. A03 overdue / clamp — rows 10–11; `already_due`; bounded
   dispatcher.
8. Validation — A01–A04 fixtures, TM-14 review, graph checks;
   stop before G5.

## Validation (when authorized)

Focused:

```text
cargo test -p finstack-ai-kernel --offline --locked cancel
cargo test -p finstack-ai-kernel --offline --locked timer
cargo test -p finstack-ai-kernel --offline --locked retry
cargo test -p finstack-ai-runtime --features native-tokio --offline --locked timer
cargo test -p finstack-ai-runtime --features native-tokio --offline --locked cancel
cargo test -p finstack-ai-test --test model_port --offline --locked
cargo test -p finstack-ai-test --test tool_port --offline --locked
cargo test -p finstack-ai-test --test interaction --offline --locked
cargo fmt --all --check
cargo clippy --workspace --all-targets --offline --locked -- -D warnings
```

Candidate:

```text
cargo test --workspace --offline --locked
cargo test -p finstack-ai-test --test journal_v1 --offline --locked
cargo test -p finstack-ai-test --test public_rust_api --offline --locked
cargo tree -e normal --locked --offline -p finstack-ai-runtime -p finstack-ai -p finstack-ai-kernel
uv run --no-project python tools/wasm_package/check.py graph
```

Do not require `mise run ci`, the full pytest suite, sqlite kill,
or browser tests to close A01–A04. Do not infer G5.

## Explicit exclusions

No seventh port. No `Timer` port. No `TimerId` / `TimerRequested`.
No `ExternalEffectOutcome::Cancelled`. No `EffectKind::Application`.
No kernel-state v7. No new `RecordBody` families. No cron or
workflow scheduler. No PR-046/047 lanes or child-run fan-out.
No PR-048 crash-prefix, G5, WASM durable restart, or snapshot
rebuild. No silent ADR-013/004 Implemented. No exactly-once
claim. No fabricated cancelled-success or tool-produced results
on uncertainty. Do not reuse the PR-043 or PR-044 envelope. Do
not admit while PR-044 is the sole active logical PR.
