# PR-041 execution plan

Date: 2026-08-14
Owner: me@jeickmeier.com
Branch: `codex/pr-041-snapshots-replay`
Baseline: local `main` at `b460ce2f17413a465486951b0ac592ca0f36dd6f`
Plan baseline: documentation pack v0.20 / PLAN-0.18

## Admission

- Phase 6 entrance is `Passed` (3/3). Do not re-record `PH6-E-entrance-*`.
- PR-039 is `Done` at local merge `64c54e767f53faac240ab92c19a8447264e82ff8`.
- PR-040 is `Done` at local merge `dbd10d35b223288666b2fdc0e13d03f48b5b97c3`.
- ADR-032 is Accepted; this PR is the mapped disposable snapshot delivery. Compact projections stay a later ADR.
- PR-041 is the only active logical PR. PR-042+ stays excluded.
- No Implementation Plan section 6.3 ADR trigger applies.
- Threat Model section 18 is triggered (snapshot semantics; TM-13, also TM-12 / TM-16). Complete the review before merge.

## Acceptance mapping

- PR-041-A01: deleting every snapshot still recovers the same `state_hash` from records alone.
- PR-041-A02: corrupt, mismatched, unknown-version, and forked snapshots are ignored; load still succeeds.
- PR-041-A03: snapshot-plus-tail and full replay produce identical `state_hash` and settlement maps.
- PR-041-A04: a stalled snapshot write cannot block `submit` past `write_timeout`; benches are warning-only.

## Execution envelope

`mode=integrated; target=main; local branch/commit/merge authorized;
external actions=feature-branch push plus hosted Linux CI/security;
hosted PR/merge, npm publish, tag, and G5 inference prohibited`.

## Explicit exclusions

No compaction or prune API. No compact snapshot DTO or new ADR unless
benches only propose one. No `SnapshotWritten` emission. No deferred-effect
reconciliation, interactions, public multi-lane APIs, crash-prefix, migrators,
IndexedDB v1, or `0.0.3`. Runtime and SDK stay protocol-free. No G5 decision.
