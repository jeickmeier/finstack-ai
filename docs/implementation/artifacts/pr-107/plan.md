# PR-107 execution plan stub

Date: 2026-08-20
Owner: unassigned
Plan baseline: documentation pack v0.28 / PLAN-0.26
Admission: In progress / locally verified; TM-04 security review still required before merge. No candidate or Done claim.

## Purpose

Close H2 so redaction failure construction never fails open.

## Principal changes

- Return `Result<StageOutcome, MiddlewareError>` from failure paths.
- Convert descriptor-construction failure to a stable middleware error.

## Acceptance mapping

- PR-107-A01: oversized/undecodable cases never return `Continue`.
- PR-107-A02: successful/wrapped redaction remains green.
- PR-107-A03: focused, TM-04 adversarial, and full checks pass.

## Threat-model review

TM-04 security review required before merge.

## Explicit exclusions

PR-102 constructor refactor, policy expansion, and secret-bearing errors.

## Dependencies

PR-104, PR-105, and PR-106.

## Execution envelope

Local implementation present in the worktree. Local verification passed:
`cargo test -p finstack-ai-middleware-redaction --locked --lib` (42); shared suite (PR-15-E-local-suite-ceac2584dc19). TM-04 review: Pending (PR-107-E-tm04-pending-32609bbe854d).
No branch, commit, merge, push, external action, candidate, or Done claim.
