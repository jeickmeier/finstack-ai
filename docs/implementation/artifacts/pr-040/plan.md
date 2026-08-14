# PR-040 execution plan

Date: 2026-08-14
Owner: me@jeickmeier.com
Branch: `codex/pr-040-sqlite-journalstore`
Baseline: local `main` at `3edf5d8f84aeaa2d8d2af83e5acfc2cffe012818`
Plan baseline: documentation pack v0.20 / PLAN-0.18

## Admission

- Phase 6 entrance is `Passed` (3/3). Do not re-record `PH6-E-entrance-*`.
- PR-039 is `Done` at local merge `64c54e767f53faac240ab92c19a8447264e82ff8`.
- ADR-016 is Accepted; this PR is the mapped SQLite store. Public multi-lane APIs stay PR-047.
- PR-040 is the only active logical PR. PR-041+ stays excluded.
- No Implementation Plan section 6.3 ADR trigger applies.
- Threat Model section 18 is triggered (native persistence; TM-12, also TM-04, TM-10, TM-16, TM-18). Complete the review before merge.

## Acceptance mapping

- PR-040-A01: committed batches are all-or-nothing under injected mid-transaction rollback.
- PR-040-A02: durable-mode commits survive the process-kill and simulated power-loss harnesses separately; relaxed mode never advertises NFR-REL-001.
- PR-040-A03: same-instance second writer gets `Conflict`; second connection/process gets `sqlite_busy`.
- PR-040-A04: crate-local Criterion restore-time and storage-growth benches (warning-only).
- PR-040-A05: v1 `user_version` is one-way with backup guidance; unknown versions fail closed.

## Execution envelope

`mode=integrated; target=main; local branch/commit/merge authorized;
external actions=feature-branch push plus hosted Linux CI/security;
hosted PR/merge, npm publish, tag, and G5 inference prohibited`.

## Explicit exclusions

No PostgreSQL or distributed locking. No snapshot acceleration (PR-041).
No full crash-prefix matrix, migrator, IndexedDB v1, or `0.0.3` (PR-048).
No public multi-lane APIs (PR-047). No default Agent/Python/WASM SQLite
wiring. No rusqlite in kernel, runtime, SDK, or wasm-host. No prune API.
No G5 decision.
