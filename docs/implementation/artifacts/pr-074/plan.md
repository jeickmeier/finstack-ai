# PR-074 execution plan

Date: 2026-08-17
Owner: me@jeickmeier.com
Plan baseline: documentation pack v0.24 / PLAN-0.22

This file is the execution contract for PR-074. Closed PR-001–PR-073
envelopes are not reused. PR-074 is the only active logical PR.
This file authorizes Phase 12 sequencing; it does not admit E-group
code (PR-075–PR-079).

## Purpose

Authorize 1.0 production-driver work as Phase 12 and record the
planning-pack amendment. Waves 1–4 hygiene (A/B/C + TM-04) is already
in the worktree and must not be reverted. This slice is authorization
and envelope stubs, not production-driver code.

## Principal changes

- Add Implementation Plan §17C with PR-074–PR-079.
- Update §5.1 critical-path graph and §22 traceability.
- Bump Plan 0.21→0.22 and pack 0.23→0.24.
- Add additive / new-package rows to `1.0-compatibility-matrix.md`.
- Write this envelope and `pr-075` through `pr-079`.
- Do not amend NFR-PORT-*.

## Acceptance mapping

Five Implementation Plan bullets map 1:1 to A01–A05.

- PR-074-A01: Implementation Plan 0.22 contains §17C with
  PR-074–PR-079, each with Purpose, Principal changes, Acceptance
  evidence, Dependencies, Explicitly excluded, and Traceability.
- PR-074-A02: Pack README is v0.24 and lists Implementation Plan 0.22.
- PR-074-A03: Compatibility matrix has additive / new-package rows
  for Lane verbs, `finstack-ai-workflow-local`, Python handles, and
  the child-run API.
- PR-074-A04: Envelope stubs exist for PR-074–PR-079.
- PR-074-A05: NFR-PORT-* text is unchanged.

## Explicit exclusions

E1/E3/E4 production code. Phase 11 provider migration. NFR-PORT-*
amendment. D1–D18 governance transaction. `RequestCompactionModel`
landing. Temporal engine integration. Registry publication. Reverting
Waves 1–4.

## Compatibility class

Docs / tooling only. No new public API in this PR.

## Dependencies

Waves 1–4 hygiene in the worktree; B1 freeze-gate upgrade; Phase 9 /
G8. Phase 10 / PR-067 may remain in progress.

## Suggested successor sentence

When this authorization slice is the admitted baseline:

```
Run PR-075; mode=integrated; target=main; local branch/commit/merge
authorized; external actions=none; stop before any gate crossing.
```

Do not infer PR-075–PR-079 admission from this file.
