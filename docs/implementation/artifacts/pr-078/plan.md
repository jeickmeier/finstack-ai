# PR-078 execution plan

Date: 2026-08-17
Owner: me@jeickmeier.com
Plan baseline: documentation pack v0.24 / PLAN-0.22

This file is the execution contract for PR-078 (E4b+E4c). Closed
PR-001–PR-077 envelopes are not reused. PR-078 is the only active
logical PR once admitted. This file does not admit PR-079.

## Purpose

Expose Rust lane verbs on `PyLane` / `Session` and wire the Python
binding to `finstack-ai-store-sqlite`.

## Principal changes

- Python `cancel`, `append_text`, and `Session.lane_by_id` as thin
  `#[pymethods]` (may ship slightly ahead of `run`/`suspend`/`resume`).
- Python `run` / `suspend` / `resume` wrappers after PR-077.
- Add `finstack-ai-store-sqlite` to the binding manifest; expose
  `SqliteDurability::{Durable, Relaxed}`.
- Add the migration and settlement-idempotency fixtures called by
  the PR-048 plan.
- Extend `test_durable_restart.py` to respawn once E3/E4a `resume`
  exists.

## Acceptance mapping

Four Implementation Plan bullets map 1:1 to A01–A04.

- PR-078-A01: `PyLane` exposes `cancel`, `append_text`, `run`,
  `suspend`, and `resume`; `Session` exposes `lane_by_id`.
- PR-078-A02: `SqliteDurability` is importable from the public
  Python package.
- PR-078-A03: Migration and settlement-idempotency fixtures pass
  through the Python binding.
- PR-078-A04: `test_durable_restart.py` respawns a run after
  `resume` (not inspect-only).

## Explicit exclusions

Child-run / `complete_external` Python routing (PR-079). WASM lane
verbs. Temporal. NFR-PORT-* amendment. Wheel-matrix restoration.

## Compatibility class

Additive Python. Freeze-gate / public-item baseline must be updated
in the same change for new exports.

## Dependencies

PR-077. `cancel` / `append_text` / `lane_by_id` may be written first
in this PR.

## Suggested authorization sentence

```
Run PR-078; mode=integrated; target=main; local branch/commit/merge
authorized; external actions=none; stop before any gate crossing.
```

Do not infer admission from this planning file.
