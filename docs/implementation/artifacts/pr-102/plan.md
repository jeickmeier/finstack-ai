# PR-102 execution plan stub

Date: 2026-08-20
Owner: unassigned
Plan baseline: documentation pack v0.28 / PLAN-0.26
Admission: In progress / locally verified; no candidate, merge, or Done claim.

## Purpose

Close F3 through one private redaction-constructor assembly path.

## Principal changes

- Route `try_with_config` and `try_wrapping` through shared assembly.
- Preserve three public constructors, validation, ordering, masks, and digest.

## Acceptance mapping

- PR-102-A01: constructor identity properties are pinned.
- PR-102-A02: redaction behavior/public constructors remain unchanged.
- PR-102-A03: focused and full checks pass.

## Threat-model review

No new trigger; preserve TM-04 fail-closed construction.

## Explicit exclusions

H2, constructor removal, and redaction-policy changes.

## Dependencies

PR-099. Independent of PR-100, PR-101, and PR-103.

## Execution envelope

Local implementation present in the worktree. Local verification passed:
`cargo test -p finstack-ai-middleware-redaction --locked --lib` (42); shared suite (PR-15-E-local-suite-ceac2584dc19).
No branch, commit, merge, push, external action, candidate, or Done claim.
