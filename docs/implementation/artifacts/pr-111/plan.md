# PR-111 execution plan stub

Date: 2026-08-20
Owner: unassigned
Plan baseline: documentation pack v0.28 / PLAN-0.26
Admission: In progress / locally verified; TM-04/TM-21 security review still required before merge. No candidate or Done claim.

## Purpose

Close H6 by rejecting `Replace` plus `CompactContext` aggregates.

## Principal changes

- Fail folding as `middleware_stage_unlandable`.
- Correct settlement documentation.
- Add fold/settlement/runtime integration coverage.

## Acceptance mapping

- PR-111-A01: combined outcomes fail before settlement.
- PR-111-A02: either outcome alone remains landable.
- PR-111-A03: focused, conformance, TM-04/TM-21, and full checks pass.

## Threat-model review

TM-04/TM-21 security review required before merge.

## Explicit exclusions

Leaf merging, compactor wrapping, sequential rebasing, and a new composition
model/stage/outcome.

## Dependencies

PR-107, PR-108, PR-109, and PR-110.

## Execution envelope

Local implementation present in the worktree. Local verification passed:
`cargo test -p finstack-ai-runtime --locked --lib -- exec::middleware_driver` (27); `cargo test -p finstack-ai-runtime --locked --features native-tokio --lib -- exec::stage_settlement` (61); shared suite (PR-15-E-local-suite-ceac2584dc19). TM-04/TM-21 review: Pending (PR-111-E-tm-pending-f9ab3166f606).
No branch, commit, merge, push, external action, candidate, or Done claim.
