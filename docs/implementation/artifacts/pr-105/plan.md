# PR-105 execution plan stub

Date: 2026-08-20
Owner: unassigned
Plan baseline: documentation pack v0.28 / PLAN-0.26
Admission: In progress / locally verified; no candidate, merge, or Done claim.

## Purpose

Close F7 by making middleware construction the only public validation path.

## Principal changes

- Privatize `PolicyInstructionsConfig::validate`.
- Retarget tests through `InstructionsMiddleware::try_new`.
- Update API fixtures and migration notes.

## Acceptance mapping

- PR-105-A01: invalid construction preserves errors/codes.
- PR-105-A02: API checks prove only the approved removal.
- PR-105-A03: public-item, conformance, and full checks pass.

## Threat-model review

No new trigger; construction validation remains mandatory.

## Explicit exclusions

Validation weakening, convenience APIs, and serialization changes.

## Dependencies

PR-100–PR-103; independent of PR-104/PR-106 after that barrier.

## Execution envelope

Local implementation present in the worktree. Local verification passed:
Shared suite including `mise run check-public-api`, `check-all`, `test-all` (PR-15-E-local-suite-ceac2584dc19).
No branch, commit, merge, push, external action, candidate, or Done claim.
