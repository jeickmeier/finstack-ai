# PR-104 execution plan stub

Date: 2026-08-20
Owner: unassigned
Plan baseline: documentation pack v0.28 / PLAN-0.26
Admission: In progress / locally verified; no candidate, merge, or Done claim.

## Purpose

Close F1 under the approved tool-policy Rust API reduction.

## Principal changes

- Privatize four rule structs/inspection accessors; remove duplicate `Default`.
- Export retained builders/errors explicitly.
- Update API fixtures and migration notes without changing serialized identity.

## Acceptance mapping

- PR-104-A01: fixtures show only approved removals.
- PR-104-A02: serialization and digest bytes are unchanged.
- PR-104-A03: public-item and middleware conformance pass.
- PR-104-A04: migration notes identify replacement construction paths.

## Threat-model review

No new trigger; enforcement behavior remains unchanged.

## Explicit exclusions

Serialization/digest change, wrappers, and policy-semantic changes.

## Dependencies

PR-100, PR-101, PR-102, and PR-103.

## Execution envelope

Local implementation present in the worktree. Local verification passed:
Shared suite including `mise run check-public-api`, `check-all`, `test-all` (PR-15-E-local-suite-ceac2584dc19).
No branch, commit, merge, push, external action, candidate, or Done claim.
