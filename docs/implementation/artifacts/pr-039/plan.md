# PR-039 execution plan

Date: 2026-08-14
Owner: me@jeickmeier.com
Branch: `codex/pr-039-journalstore-v1`
Baseline: local `main` at `15aa94f54336336d6897b8efbf4371df0d21070c`
Plan baseline: documentation pack v0.20 / PLAN-0.18

## Admission

- G2 is `Passed` via `G2-D-native-runtime-7c2e9a4d1b65`. PR-014 is `Done`.
  Phase 6 entrance criterion 1 (commit loop stable) is recorded as passed.
- The journal schema candidate is ADR-015 plus the PR-008
  `RecordDraft` / `RecordEnvelope` freeze and the TDD §12.2 remaining
  variant names. Codec bytes and variant field materialization remain
  this PR.
- Python and JS consume state through public handles (PR-028 / PR-035).
- PR-039 is the only active logical PR. PR-040+ stays excluded.
- No Implementation Plan section 6.3 ADR trigger applies.
- Threat Model section 18 is triggered (journal parser and checksum
  semantics; TM-12). Complete the review before merge.

## Acceptance mapping

- PR-039-A01: canonical-CBOR bytes match across supported native targets.
- PR-039-A02: binary fixtures plus lossless JSON projection, including
  canonical decimal-string `micros`.
- PR-039-A03: map-order, integer, float, negative-zero, and rejection
  fixtures.
- PR-039-A04: exact-limit and one-over-limit decode fixtures.
- PR-039-A05: payload digest and envelope checksum known-answers for
  every activated `RecordBody` family.
- PR-039-A06: tampered journals fail verification before apply.
- PR-039-A07: memory store uses the protocol codec; runtime/SDK stay
  protocol-free.
- PR-039-A08: `fixtures/compatibility/journal/v1/` historical runner.
- PR-039-A09: post-commit ambiguous-ack retry returns the original
  receipt once; concurrent new batches still conflict.

## Execution envelope

`mode=integrated; target=main; local branch/commit/merge authorized;
external actions=feature-branch push plus hosted Linux CI/security;
hosted PR/merge, npm publish, tag, and G5 inference prohibited`.

## Explicit exclusions

No SQLite, snapshot acceleration, crash-prefix durability,
IndexedDB v1, remote framing, WIT, or G5 decision.
