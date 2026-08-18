# finstack-ai-kernel remediation review

Status: numbered findings #1–#23 for Phase 13 / PR-080–PR-084. Checked in by PR-080.

Authority: this file is the in-repo authority for the numbered review. The companion
artifact URL is not an authority. This file does not amend `docs/planning/` by itself,
does not claim gate evidence, and does not assert ownership, approval, or completion
for any logical PR. Section references (§) are to
`docs/planning/03-finstack-ai-technical-design.md`.

Related: [kernel-conformance-review.md](kernel-conformance-review.md) records F1–F37 at
`cae7b91`. F-series dispositions for this program are appended there.

Findings below are transcribed from the Phase 13 program. Locations and mechanics that
the program names are kept. Details the program does not name are not invented.

## HEAD dispositions (do not re-implement)

| ID | Status | Notes |
| --- | --- | --- |
| F1, F2, F3 | Fixed at HEAD | CBOR tool-result / opaque decode and `state_version` `.max` promotion. |
| #14 | Fixed at HEAD | `RetryScheduled.prior_error` now participates in `ErrorDescriptor` re-validation. |
| #16 | Fixed at HEAD | `ToolBatchOpened.calls` is a `BoundedVec`. |
| #20 | Fixed at HEAD | Transient accessors for `ToolProgress` / `ProviderHeartbeat` exist. |
| #22 | Fixed at HEAD | `derived_event_kind` has direct tests. |
| Most of #8 / #19 | Fixed at HEAD | `AssigneeHint` Role/Queue deserialize and `InteractionRequest::try_new` already call `validated_label`. Residual work, if any, is PR-083. |
| F23 | Fixed at HEAD | `state_hash` rustdoc already covers versions 1–6. Do not revert. |
| F13 | Closed as #5 Option A | TDD 0.20: no fabricated observed charge; both reserve twins deleted. |
| F9 | Resolved by #1 / #10 | Ordering fact confirmed; #10 is the concrete redelivery re-count. |

`raw_json.rs:128-145` is the live `Metadata` member-limit path. Do not delete it.
`KernelInput` has **15** variants; **5** are already in `equal_committed_redelivery`.

## Still open

| Slice | Findings |
| --- | --- |
| PR-081 | #18; remaining #23 rustdoc (not F23); dead `buffered_prefix` machinery; fingerprint/projection twins; additive `RunEventKind::kind_name()` |
| PR-082 | #1, #2, #3, #4, #10, #15; #5 landed Option A / TDD 0.20 |
| PR-083 | #6, #7; residual #8 / #19 that is not the landed Role/Queue checks |
| PR-084 | #9, #11, #12, #13a landed in the local worktree; #17 landed BoundedMap; two `pub fn`s removed |

## Deferred

| ID | Why | Route |
| --- | --- | --- |
| #13b | PR-067 and Phase 10 exit 5 forbid reshaping `InvariantViolation`. | PR-067 / Phase 10 exit 5; not Phase 13. |
| Any PR-083 item that fails its unreachability proof | Must not ship on a hand-wave. | Phase 11 or an `EX-*` row. |

## Findings

### #1 — Decide-limit projection terminally fails a rejectable payload

Owner: PR-082 (lands together with #2).

`decide_limit` can emit a terminal failure for a payload that should be rejected.
`limit_shape` (`apply/shapes.rs`) accepts `[LimitReached, RunFailed]`. The severity is
a wrong terminal decision on a healthy run, not a journal wedge. Admissibility guards
in `decide/limit.rs` (`stage_boundary`, `compaction_boundary`,
`outstanding_model_settlement`, `outstanding_tool_settlement`,
`outstanding_external_completion`) may only fall through to the existing `_ => {}`.
They must not add or reorder a frozen error. A narrow hoist belongs in new
`decide/precheck.rs` between `equal_committed_redelivery` and `decide_limit`. Do not
hoist `assistant_message` presence above `ConflictingCompletionId`. Replace
digest-swallowing `let Ok(..) else { return Ok(None) }` with `?` (sites are in
`decide/mod.rs`, not `limit.rs`).

### #2 — `TimerFired` after `RunSuspended` is unappliable

Owner: PR-082 (lands together with #1).

`unknown_cost_policy` is evaluated outside every projection arm. From `Sleeping` it
can emit `RunSuspended`; the next `TimerFired` is then unappliable. Same wedge as #1
from the apply side. Required shape change at `apply/shapes.rs:116`:
`Some(Accepted) => false` and `Some(Suspended) => reconciliation_shape(...)`, plus a
phase guard in `decide_timer_fired`. Test reconciliation with **three** calls, not two.
`cancellation_followups` can emit `ToolBatchClosed` before a trailing `ToolCallSettled`.

### #3 — Four maps capped by `validate()` are missing from capacity preflight

Owner: PR-082. One commit across `preflight_batch` + `preflight_decision` +
`decide_timer_fired`.

Uncovered growth fields: `timer_firings`, `child_preparations`, `budget_reservations`,
`budget_charges`. `SEMANTIC_MAP_MAX_ENTRIES = 256`. `TimerFired` is produced by
`decide`, so preflighting it only in `preflight_batch` converts an unhashable state
into a journal wedge. Grep real journals before merge. Current goldens do not exceed
the cap.

### #4 — `apply_completed_usage` must not charge through structural or foreign bypass

Owner: PR-082. Land before #5.

Pin that `apply_completed_usage` does not charge through `structural_record_shape` /
`foreign_run_shape`. Those paths bypass the phase matrix. Enumerate
`RecordBody::is_structural()` before relying on `validate_batch_shape`.

### #5 — `AllowWithinReservedMaximum` Option A

Owner: PR-082. **Landed** (maintainer decided Option A). TDD 0.20.

On a costless completion under `AllowWithinReservedMaximum`: test remaining
headroom (`accrued >= maximum` → existing `control_failure_decision` / terminal
path). Exact observed equality does not terminate. Write nothing to `usage.cost`.
Both reserve twins (`reserve_remaining_cost`, `reserve_unknown_cost`) are
deleted so decide and apply agree by construction. TDD §22.6.2 now states that
“records no fabricated observed charge” is the whole policy behavior.

### #6 — Terminal-state validator tightening depends on #1

Owner: PR-083. Land after PR-082.

Asserts on terminal state that #1 reorders. Every new rejection needs a fixture that
the shape was unproducible by any committed reducer path; failures defer.

Watch: `state_hash_oracles.rs` minimal v2–v6, `successful.rs` v1 orphan `ToolResult`,
and `accepted_v1_after_prepare` / `src/state/tests.rs`.

### #7 — Remaining `KernelState::validate` gaps

Owner: PR-083.

Close remaining `KernelState::validate` gaps, or prove they are already enforced.
Same unreachability-fixture rule as #6. Failures defer.

### #8 — Residual `AssigneeHint` work beyond landed Role/Queue checks

Owner: PR-083 for residual only.

Most Role/Queue deserialize and constructor checks already landed at HEAD. Residual
work that is not those checks is closed or dispositioned as already landed. Input
tightening on a constructor; no control change.

### #9 — `provider_call_id` missing from `ContentProjection::ToolCall`

Owner: PR-084. Last. New digest computed after #1 / #4 / #5.

Existing seven pinned hash fixtures contain no tool-call payload. Adding
`provider_call_id` changes zero existing pinned digests. Landed:
`ContentProjection::ToolCall.provider_call_id` plus
`fixtures/compatibility/public-rust-api/v1/kernel-state/valid--tool-call-hash.json`.
Public-rust-api corpus count is 137.

### #10 — Two batch-resolution implementations are not identical

Owner: PR-082 (not PR-081).

`predicted_settlement_counts` (`records/tools.rs`) and `tool_settlement_shape`
(`apply/shapes.rs`) are not identical. One owner or a documented agreement test.
Together with #1 this resolves F9.

### #11 — Human-path `RawJson` limits after full materialization

Owner: PR-084. Landed.

Human-path `RawJson` decode applies byte, member, and depth limits during
streaming `StrictValue` before canonical materialization. Durable
canonical-CBOR path is out of scope. `raw_json.rs:128-145` stays; it is the
live `Metadata` member-limit path.

### #12 — Kernel telemetry / secret-needle memo

Owner: PR-084. Landed.

Memo: [`artifacts/pr-084/tm-04-secret-needles.md`](artifacts/pr-084/tm-04-secret-needles.md).

### #13a — `InvariantViolation` call-site re-pointing

Owner: PR-084 (not folded into PR-067). Landed where an existing variant
applied without a payload change.

Call-site re-pointing only. `InvariantViolation` is still a unit variant.
Sites that wrap constructor, overflow, or internal-dispatch failures stay
`InvariantViolation`.

### #13b — `InvariantViolation` reshape

**Deferred.** PR-067 principal changes and Phase 10 exit 5 forbid a
`KernelError` variant-shape change. Not landed in Phase 13.

### #14 — `RetryScheduled.prior_error` escapes `ErrorDescriptor` re-validation

HEAD-fixed. Disposition only. Do not re-implement.

### #15 — Apply-path missing-call lookup outcomes

Owner: PR-082.

Apply-path missing-call lookup stays an existing `KernelError`. Do not add a variant.

### #16 — `ToolBatchOpened` record arrays decode without a `BoundedVec`

HEAD-fixed. Disposition only. Do not re-implement.

### #17 — `BudgetRequest.extension_counters` is an unbounded `BTreeMap`

Owner: PR-084. **Landed** (maintainer decided BoundedMap; no ADR).

`BudgetRequest.extension_counters: BTreeMap` → `BoundedMap`. Classified
source-breaking, 1.0.0 pre-publication, no ADR, no major bump (precedent:
`SessionRuntime::existing`). `--write` freeze baselines only if a public
**name** is added or removed; a field-type change is still invisible to B1.

### #18 — Remaining rustdoc drift

Owner: PR-081.

Doc-only. Remaining sites that still claim schema-1-only behavior. Not F23.

### #19 — Residual constructor / `AssigneeHint` input tightening

Owner: PR-083 for residual only.

Most Role/Queue checks already landed at HEAD. Residual work shares #8's
unreachability rule.

### #20 — Transient event bodies are write-only

HEAD-fixed. Disposition only. Accessors exist. Do not re-implement.

### #21 — `request_version`

**No longer needed.** Keep the field. Dropping it is a journal-family DTO removal
that would break replay; a rejecting const is also wrong. The type is re-exported
from `finstack-ai-kernel`, `finstack-ai`, and `finstack-ai-runtime`. Phase 13 will
not remove it. Question closed. Do not drop in 1.0.x.

### #22 — `derived_event_kind` lacks direct tests

HEAD-fixed. Disposition only. Do not re-implement.

### #23 — Remaining rustdoc (not F23)

Owner: PR-081 for remaining sites.

F23 (`state_hash` versions-1–6 rustdoc) is already fixed at HEAD. Remaining #23
rustdoc sites still claim schema-1-only behavior. Do not revert the live
`state_hash` docs.

## Unnumbered PR-081 work recorded by the program

These are not additional finding IDs. They travel with PR-081:

- Remove `buffered_prefix` / `recompute_buffered_prefix` / `note_source_advanced` in
  `records/tools.rs` (private, `#[serde(skip)]`, excluded from `PartialEq`).
- Collapse the three fingerprint/projection twins, including `content/tool.rs`.
- Add `RunEventKind::kind_name()` as an additive inherent method (named review; B1
  does not extract signatures).
- `RecordDraft::validate_run_lineage` and
  `OperationSummary::invocation_effect_id` were reserved for PR-084 and are
  now removed there.

## Residual risks

Recorded in `artifacts/pr-080/plan.md`. Summary:

1. `#3` / 256-entry cap: a long real journal would start failing `apply`.
2. `#5` Option A changes `limit_usage.cost` `Some(maximum)` → `None` for already-committed costless completions.
3. `#1` guards are believed behavior-preserving; run the full `matrix.rs` `include!` suite.
4. `#4` must enumerate structural/foreign bypass before relying on `validate_batch_shape`.
5. `#2` tail loop: test three calls, not two.
6. PR-067 overlap on `apply/` and `validate.rs`.
7. B1 still does not extract signatures.
8. If a parallel branch consumes PR-080–084 / pack v0.26, stop and reconcile IDs.
