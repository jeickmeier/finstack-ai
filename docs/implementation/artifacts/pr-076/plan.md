# PR-076 execution plan

Date: 2026-08-17
Owner: me@jeickmeier.com
Plan baseline: documentation pack v0.24 / PLAN-0.22

This file is the execution contract for PR-076 (E3). Closed
PR-001–PR-075 envelopes are not reused. PR-076 is the only active
logical PR once admitted. This file does not admit PR-077–PR-079.

## Purpose

Restore `finstack-ai-workflow-local` as the shipped driver of
`WorkflowSession` and add adapter-owned durable cron with run-once
catch-up.

## Principal changes

- Restore `extensions/workflow/finstack-ai-workflow-local/` as the
  production driver of `WorkflowSession`.
- Promote the relocated `local_workflow` suite into that crate.
- Persist schedule state in an adapter-owned durable table in the
  same `JournalStore` / sqlite file (tenant-scoped), loaded on
  `attach`. Document that this is adapter state, not a kernel record.
- Cron expression + next-fire instant computed against
  `WorkflowSession::clock()` (`ExternalClock`), never wall time.
- On restart, for each schedule whose next-fire is in the past, fire
  once, then recompute the next future tick. No backfill storm.
- Do not add a `RecordBody` variant.

## Acceptance mapping

Four Implementation Plan bullets map 1:1 to A01–A04.

- PR-076-A01: Existing `local_workflow` A01/A02/A04/TM-19 still pass
  through the restored crate.
- PR-076-A02: New `restart.rs` cases: schedule survives restart; one
  catch-up fire; no second catch-up; tenant isolation via
  `WorkflowSession::tenant_scope()`.
- PR-076-A03: Cron advances only through `ExternalClock` (no
  wall-clock flake).
- PR-076-A04: Temporal remains a non-engine shim; the PR-059
  "reference integration" claim is true of the local adapter.

## Explicit exclusions

`RecordBody` / journal-freeze change. Temporal engine integration.
Kernel clock. Phase 11. E1/E4 code. NFR-PORT-* amendment.

## Compatibility class

New package plus adapter trigger API. Freeze-gate baseline must be
updated in the same change for any new public name.

## Dependencies

PR-074. Independent of PR-075.

## Suggested authorization sentence

```
Run PR-076; mode=integrated; target=main; local branch/commit/merge
authorized; external actions=none; stop before any gate crossing.
```

Do not infer admission from this planning file.
