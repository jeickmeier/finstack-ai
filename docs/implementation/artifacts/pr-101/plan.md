# PR-101 execution plan stub

Date: 2026-08-20
Owner: unassigned
Plan baseline: documentation pack v0.28 / PLAN-0.26
Admission: In progress / locally verified; no candidate, merge, or Done claim.

## Purpose

Close F4-tool-policy as a test-only fixture simplification.

## Principal changes

- Share UUID and `MiddlewareContext` builders within tool-policy tests.
- Preserve production code, serialized configuration, and digest bytes.

## Acceptance mapping

- PR-101-A01: shared builders are used and focused tests pass.
- PR-101-A02: full checks pass with no production behavior change.

## Threat-model review

No trigger; test-only refactor.

## Explicit exclusions

Workspace helper crate, F1 public changes, and production behavior changes.

## Dependencies

PR-099. Independent of PR-100, PR-102, and PR-103.

## Execution envelope

Local implementation present in the worktree. Local verification passed:
Shared suite `mise run check-public-api`, `check-all`, `test-all`, `check-wasm`, `build-wasm --release` (PR-15-E-local-suite-ceac2584dc19).
No branch, commit, merge, push, external action, candidate, or Done claim.
