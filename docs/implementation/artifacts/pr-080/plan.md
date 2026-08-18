# PR-080 execution plan

Date: 2026-08-18
Owner: me@jeickmeier.com
Plan baseline: documentation pack v0.26 / PLAN-0.24

This file is the execution contract for PR-080. Closed PR-001–PR-079
envelopes are not reused. PR-080 is the only active logical PR.
This file authorizes Phase 13 sequencing; it does not admit kernel
code (PR-081–PR-084).

## Purpose

Authorize 1.0.x kernel remediation as Phase 13 and record the
planning-pack amendment. This slice is authorization, review
check-in, and evidence-chain repair, not kernel behavior.

## Principal changes

- Add Implementation Plan §17D with PR-080–PR-084.
- Update §5.1 critical-path graph and §22 traceability.
- Bump Plan 0.23→0.24 and pack v0.25→v0.26. TDD stays 0.19.
- Repair PLAN-0.20–0.22 above PLAN-0.23; append PLAN-0.24 Current.
- Check in `kernel-remediation-review.md` and link both reviews.
- Write this envelope and `pr-081` through `pr-084`.
- Do not amend NFR-PORT-*. Do not re-land B1.

## Acceptance mapping

Six Implementation Plan bullets map 1:1 to A01–A06.

- PR-080-A01: Implementation Plan 0.24 contains §17D with
  PR-080–PR-084, each with Purpose, Principal changes, Acceptance
  evidence, Dependencies, Explicitly excluded, and Traceability.
- PR-080-A02: Pack README is v0.26 and lists Implementation Plan
  0.24. Technical Design remains 0.19.
- PR-080-A03: Evidence register keeps the PLAN-0.23 row and digest,
  inserts recovered PLAN-0.20–0.22 rows, appends PLAN-0.24 as
  Current, and updates the line-21 inventory prose.
- PR-080-A04: Envelope stubs exist for PR-080–PR-084.
  This file records the three open maintainer questions and residual
  risks.
- PR-080-A05: The 23-finding review is checked in. Registers lists
  that file and `kernel-conformance-review.md`. HEAD dispositions
  are recorded on both reviews.
- PR-080-A06: `#21` has a keep-the-field row on
  `public-api-change-backlog.md` (question closed; Phase 13 will
  not remove it). The compatibility matrix classifies decided
  Phase 13 surfaces including #5 and #17. NFR-PORT-* is unchanged.
  B1 is not re-landed. No gate decision ID is manufactured.

## Explicit exclusions

Kernel behavior (PR-081–PR-084). TDD §22.6.2 Option A wording.
`BudgetRequest` → `BoundedMap`. `request_version` removal.
`InvariantViolation` reshape. B1 re-implementation. Phase 11
provider code. Phase 12 production-driver code. NFR-PORT-*
amendment.

## Compatibility class

Docs / tooling only. No new public API in this PR.

## Dependencies

Phase 9 / G8. Phase 10 / PR-067 may remain in progress. Pack v0.25
B1 freeze gate is already landed.

PR-067 overlap: still `In progress` on `main`, 9/9 local, no
candidate. It already owns capacity-before-apply and excludes
`InvariantViolation` reshape. #3 and #13a touch `reducer/apply/`
and `state/validate.rs`. Prefer closing PR-067 to a candidate
before PR-082, or fold #13a into PR-067 only if that does not force
`(revised)` acceptance IDs.

## Open questions (answered 2026-08-18)

**#5 — decided Option A (landed).** On a costless completion under
`AllowWithinReservedMaximum`: test remaining headroom
(`accrued >= maximum` → `control_failure_decision`); write nothing
to `usage.cost`; delete both reserve twins. TDD 0.20. Exact
observed equality does not terminate.

**#17 — decided BoundedMap (landed).** Matrix/backlog row only
(precedent: `SessionRuntime::existing`, source-breaking, 1.0.0
pre-publication, no ADR, no major bump).
`BudgetRequest.extension_counters: BTreeMap` → `BoundedMap`.

**#21 — no longer needed (keep the field).** Dropping
`request_version` is a journal-family DTO removal that would break
replay; a rejecting const is also wrong. Keep the field. Phase 13
will not remove it. Question closed. Do not drop in 1.0.x.

## Residual risks

1. `#3` / 256-entry cap: current goldens do not exceed it; a long
   real journal would start failing `apply`.
2. `#5` Option A changes `limit_usage.cost` `Some(maximum)` →
   `None` for any already-committed costless completion. Fixtures
   appear unused; confirm against any real journal.
3. `#1` guards are believed behavior-preserving; run the full
   `matrix.rs` `include!` suite.
4. `#4` must enumerate structural/foreign bypass before relying on
   `validate_batch_shape`.
5. `#2` tail loop: test three calls, not two.
6. PR-067 overlap on `apply/` and `validate.rs`.
7. B1 still does not extract signatures: `kind_name()`, #17 field
   type, and inherent-method removals stay named review.
8. If a parallel branch consumes PR-080–084 / pack v0.26, stop and
   reconcile IDs.
9. Companion artifact 404: this program's in-repo authority is
   `docs/implementation/kernel-remediation-review.md`.
10. PLAN-0.21 exec-line staleness is recorded, not corrected.
11. Retired mise names (`kernel`, `docs-links`, `conformance`,
    `fuzz-smoke`, `check-public-items`, `ci`) must not be cited.
12. `KernelInput` is 15 variants, not 16. Five are already in
    `equal_committed_redelivery`.

## Suggested successor sentence

When this authorization slice is the admitted baseline:

```
Run PR-081; mode=integrated; target=main; local branch/commit/merge
authorized; external actions=none; stop before any gate crossing.
```

Do not infer PR-082–PR-084 admission from this file.
