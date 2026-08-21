# PR-112 execution plan stub

Date: 2026-08-20
Owner: unassigned
Plan baseline: documentation pack v0.28 / PLAN-0.26
Admission: In progress / locally verified; TM-21/SEC-INV-013 security review still required before merge. No candidate or Done claim.

## Purpose

Close H5 with durable exact secondary-model authorization.

## Principal changes

- Add optional, default-deny `CompactionAuthorization` to run security.
- Enforce exact model/policy and sensitivity before commit/dispatch/resume.
- Attenuate children; remove fabricated resume authorization.
- Update durable/public/binding/recovery/security fixtures where exposed.

## Acceptance mapping

- PR-112-A01: missing/mismatched locks fail before commit/dispatch.
- PR-112-A02: authorized requests preserve commit-before-effect.
- PR-112-A03: recovery reuses and cannot broaden/fabricate the lock.
- PR-112-A04: child attenuation and historical default-deny decoding pass.
- PR-112-A05: affected native/WASM/binding/conformance/recovery/security pass.

## Threat-model review

TM-21/SEC-INV-013 security review required before merge. Stop for ADR/change
control if implementation changes journal compatibility policy, needs an
incompatible migration, or changes commit-before-effect ordering.

## Explicit exclusions

New port/stage/record kind, recursive compaction, broader routing policy,
compatibility-policy change, and fabricated resume authority.

## Dependencies

PR-111; PR-106 owns removal of leaf self-authorization.

## Execution envelope

Local implementation present in the worktree. Local verification passed:
`cargo test -p finstack-ai-runtime --locked --features native-tokio --lib -- exec::compaction_driver` (5); `cargo test -p finstack-ai-test --locked --test crash_prefix` (15); `cargo test -p finstack-ai-test --locked --lib -- conformance::ports::tests` (1); shared suite (PR-15-E-local-suite-ceac2584dc19). TM-21/SEC-INV-013 review: Pending (PR-112-E-tm21-pending-513283a61fa5).
No branch, commit, merge, push, external action, candidate, or Done claim.
