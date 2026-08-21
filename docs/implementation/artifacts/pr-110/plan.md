# PR-110 execution plan stub

Date: 2026-08-20
Owner: unassigned
Plan baseline: documentation pack v0.28 / PLAN-0.26
Admission: In progress / locally verified; TM-04 security review still required before merge. No candidate or Done claim.

## Purpose

Close F9/H3 with sibling panic lints and fallible message/note construction.

## Principal changes

- Add unsafe/unwrap/expect/panic lint gates.
- Propagate construction failures as middleware errors.
- Preserve fail-soft parser/store/index notes and must-strip behavior.

## Acceptance mapping

- PR-110-A01: production passes lint gates without ignores.
- PR-110-A02: must-strip regressions cover every failure class.
- PR-110-A03: focused, binding, TM-04, and full checks pass.

## Threat-model review

TM-04 security review required before merge.

## Explicit exclusions

Block-type changes, hard-failing parser/store/index errors, and lint ignores.

## Dependencies

PR-104–PR-106; independent of PR-107–PR-109.

## Execution envelope

Local implementation present in the worktree. Local verification passed:
`cargo test -p finstack-ai-middleware-document-ingest --locked --lib` (27); shared suite (PR-15-E-local-suite-ceac2584dc19). TM-04 review: Pending (PR-110-E-tm04-pending-0c0ce340fabe).
No branch, commit, merge, push, external action, candidate, or Done claim.
