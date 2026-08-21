# PR-106 execution plan stub

Date: 2026-08-20
Owner: unassigned
Plan baseline: documentation pack v0.28 / PLAN-0.26
Admission: In progress / locally verified; no candidate, merge, or Done claim. PR-112 owns runtime authority.

## Purpose

Close F5 with exactly three public compaction strategy constructors.

## Principal changes

- Privatize fields; expose `sliding_window`, `large_tool_output`, `summarize`.
- Remove leaf `secondary_model_authorized`; PR-112 owns runtime authority.
- Update API/docs/examples/migration while preserving serialization/digest.

## Acceptance mapping

- PR-106-A01: fixtures expose only the three constructors.
- PR-106-A02: equivalent serialization/digest bytes are unchanged.
- PR-106-A03: no configuration boolean grants secondary-model authority.
- PR-106-A04: public-item, conformance, example, and full checks pass.
- PR-106-A05: migration notes link the PR-112 runtime dependency.

## Threat-model review

TM-21 review confirms leaf configuration cannot self-authorize.

## Explicit exclusions

H5 runtime enforcement, serialization changes, extra constructors/shims.

## Dependencies

PR-100–PR-103; independent of PR-104/PR-105 after that barrier.

## Execution envelope

Local implementation present in the worktree. Local verification passed:
Shared suite including `mise run check-public-api`, `check-all`, `test-all` (PR-15-E-local-suite-ceac2584dc19).
No branch, commit, merge, push, external action, candidate, or Done claim.
