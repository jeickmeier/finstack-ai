# PR-109 execution plan stub

Date: 2026-08-20
Owner: unassigned
Plan baseline: documentation pack v0.28 / PLAN-0.26
Admission: In progress / locally verified; no candidate, merge, or Done claim.

## Purpose

Close H1 by binding document-ingest descriptor identity to limits.

## Principal changes

- Hash canonical JSON for a private versioned three-limit shape.
- Do not serialize public `DocumentLimits` solely for identity.
- Record the intentional descriptor-identity change.

## Acceptance mapping

- PR-109-A01: differing/equivalent limits produce differing/equivalent digests.
- PR-109-A02: canonical bytes/version are pinned.
- PR-109-A03: focused, binding, compatibility, and full checks pass.

## Threat-model review

No new trigger; identity remains stable and secret-free.

## Explicit exclusions

Public `DocumentLimits` serialization, PR-100 cache work, and limits policy.

## Dependencies

PR-104–PR-106; independent of PR-107/PR-108/PR-110.

## Execution envelope

Local implementation present in the worktree. Local verification passed:
`cargo test -p finstack-ai-middleware-document-ingest --locked --lib` (27); shared suite (PR-15-E-local-suite-ceac2584dc19).
No branch, commit, merge, push, external action, candidate, or Done claim.
