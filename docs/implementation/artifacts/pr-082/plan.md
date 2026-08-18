# PR-082 execution plan

Date: 2026-08-18
Owner: me@jeickmeier.com
Plan baseline: documentation pack v0.26 / PLAN-0.24

This file is the execution contract for PR-082 (decide/apply/cost).
Closed PR-001–PR-081 envelopes are not reused. PR-082 is the only
active logical PR once admitted. This file does not admit
PR-083–PR-084.

## Purpose

Stop decide-limit from terminally failing a healthy run on a
rejectable payload, make `TimerFired` / reconciliation apply-safe,
preflight the four maps `validate()` already caps, and land
cost-accounting plus apply-lookup and batch-resolution ownership.

## Principal changes

- Land #1 and #2 together.
- Land #3 in one commit across `preflight_batch` +
  `preflight_decision` + `decide_timer_fired`.
- Land #4 before #5. #1 before #5. #5 lands Option A (TDD 0.20):
  remaining headroom, no fabricated `usage.cost`, both reserve
  twins deleted. Assign-max is not Option A.
- Land #10 and #15.

## Acceptance mapping

Seven Implementation Plan bullets map 1:1 to A01–A07.

- PR-082-A01: Projection guards plus the narrow hoist reject a
  previously terminal-on-bad-projection payload without mutation.
- PR-082-A02: `TimerFired` after `RunSuspended` is rejected;
  reconciliation includes a three-call interleaving.
- PR-082-A03: Preflight bounds `timer_firings`,
  `child_preparations`, `budget_reservations`, and
  `budget_charges`.
- PR-082-A04: #4 regression pins structural/foreign non-charge. #5
  matches TDD 0.20 Option A (no fabricated observed charge).
- PR-082-A05: Apply-path missing-call lookup (#15) returns an
  existing `KernelError`.
- PR-082-A06: The two batch-resolution implementations (#10) share
  one owner or a documented agreement test.
- PR-082-A07: For #1, #2, and #3, `replay(&batches)` equals live
  state and `state_hash()` matches.

## Explicit exclusions

Validator tightening (PR-083). Hash-projection /
`provider_call_id` fixture (PR-084). `InvariantViolation` reshape.
New `KernelError` variants. #13b `InvariantViolation` reshape.

## Compatibility class

Kernel behavior. Replay and `state_hash` proofs required. No new
`KernelError` variant.

## Dependencies

PR-080. #5 is decided Option A / TDD 0.20. Independent of PR-081
except merge conflicts. Prefer PR-067 at a candidate before
touching `apply/` overlap.

## Suggested authorization sentence

```
Run PR-082; mode=integrated; target=main; local branch/commit/merge
authorized; external actions=none; stop before any gate crossing.
```

Do not infer admission from this planning file.
