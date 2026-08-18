# PR-083 execution plan

Date: 2026-08-18
Owner: me@jeickmeier.com
Plan baseline: documentation pack v0.26 / PLAN-0.24

This file is the execution contract for PR-083 (validators).
Closed PR-001–PR-082 envelopes are not reused. PR-083 is the only
active logical PR once admitted. This file does not admit PR-084.

## Purpose

Reject previously accepted unreachable shapes after PR-082
reordering, without tightening any shape a committed reducer path
can still produce.

## Principal changes

- Land #6 after #1.
- Close remaining #7 `KernelState::validate` gaps, or prove they
  are already enforced.
- Disposition residual #8/#19 that is not the landed
  `AssigneeHint` Role/Queue checks.
- Every new rejection carries an unproducible-by-reducer fixture;
  failures defer.

## Acceptance mapping

Five Implementation Plan bullets map 1:1 to A01–A05.

- PR-083-A01: #6 terminal/validate tightening lands after #1;
  existing precedence tests still pass.
- PR-083-A02: Remaining #7 `KernelState::validate` gaps are closed
  or proven already enforced.
- PR-083-A03: Residual #8/#19 work is closed or dispositioned as
  already landed.
- PR-083-A04: Every new rejection has an unproducible-shape
  fixture; failures defer.
- PR-083-A05: `state_hash_oracles` minimal v2–v6, `successful.rs`
  v1 orphan `ToolResult`, and `accepted_v1_after_prepare` still
  pass.

## Explicit exclusions

Findings #14, #16, #20, and #22 (HEAD). Most of #8/#19
`AssigneeHint` (HEAD). Wire-visible type changes (PR-084). Any
item that fails its unreachability proof.

## Disposition (PR-083)

- **#6 landed.** `KernelState::validate` now rejects a terminal
  phase without the matching `terminal` payload at every state
  version. `apply_record` still writes phase and payload together.
- **#7 landed.** `Cancelling`/`Cancelled` require `cancellation`;
  `Suspended` requires `suspension`. Already-enforced: collection
  ceilings, version feature gates, `pending_interaction` vs phase,
  `accepted_at` pairing at v3+, tool-state at v2+, structured
  output at v4+, composition at v5+.
- **#7 deferred** (unproducible-by-reducer proof failed or the
  shape is still producible): `pending_model_effect` vs phase
  (deadline/limit `RunFailed` can leave a pending model effect);
  `current_turn` vs phase; `accepted` vs `session_id`/`lane_id` at
  v1–v4 (historical v1 snapshots); `last_interaction_terminal`
  cross-checks; `suspension` present outside
  `Suspended`/`Cancelling`/`Cancelled` (deadline-on-suspended can
  yield `Failed` with leftover `suspension`); leftover
  `terminal_candidate` after terminal commit.
- **#8/#19.** Landed Role/Queue `AssigneeHint` deserialize and
  `InteractionRequest::try_new` stay as HEAD. Residual
  `InteractionKind::Custom` empty/NUL decode now calls
  `validated_label`; `InteractionRequest::try_new` already
  rejected that shape.

## Compatibility class

Fail-closed validator tightening. Journal-breaking unless the
rejected shape is proven unproducible.

## Dependencies

PR-082.

## Suggested authorization sentence

```
Run PR-083; mode=integrated; target=main; local branch/commit/merge
authorized; external actions=none; stop before any gate crossing.
```

Do not infer admission from this planning file.
