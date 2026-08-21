# PR-100 execution plan stub

Date: 2026-08-20
Owner: unassigned
Plan baseline: documentation pack v0.28 / PLAN-0.26
Admission: In progress / locally verified; no candidate, merge, or Done claim.

## Purpose

Close F2, F4-ingest, and F8 through behavior-preserving document-ingest
internal simplification.

## Principal changes

- Share one private bounded FIFO map across parse cache and attachment index.
- Inline one-use canonicalization; move test-only dependency.
- Share crate-local test/benchmark fixtures.

## Acceptance mapping

- PR-100-A01: FIFO/reinsertion/eviction/poison parity.
- PR-100-A02: public `AttachmentIndex` and must-strip behavior unchanged.
- PR-100-A03: focused and full repository checks.
- PR-100-A04: release WASM/check and document-attachment Python coverage.

## Threat-model review

No new trigger; preserve TM-04/TM-16 behavior.

## Explicit exclusions

H1, F9/H3, public API removal, and a workspace helper crate.

## Dependencies

PR-099. Independent of PR-101–PR-103.

## Execution envelope

Local implementation present in the worktree. Local verification passed:
`cargo test -p finstack-ai-middleware-document-ingest --locked --lib` (27); shared suite `mise run check-public-api`, `check-all`, `test-all`, `check-wasm`, `build-wasm --release` (PR-15-E-local-suite-ceac2584dc19).
No branch, commit, merge, push, external action, candidate, or Done claim.
