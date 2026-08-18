# PR-084 execution plan

Date: 2026-08-18
Owner: me@jeickmeier.com
Plan baseline: documentation pack v0.26 / PLAN-0.24

This file is the execution contract for PR-084 (wire and public
types). Closed PR-001–PR-083 envelopes are not reused. PR-084 is
the only active logical PR once admitted. This file does not admit
later work.

## Purpose

Land the remaining hash, decoder, telemetry, and classified
public-type changes after decide/apply semantics are stable, so
new pinned digests are computed against the final contract.

## Principal changes

- Add `provider_call_id` to `ContentProjection::ToolCall` (#9) and
  a new tool-call hash fixture after #1/#4/#5.
- Apply human-path `RawJson` limits before full materialization
  (#11).
- Record a TM-04-style memo (#12).
- Fold #13a into PR-067 if possible; never land #13b.
- Land #17 (`BudgetRequest.extension_counters` → `BoundedMap`;
  no ADR; no major bump). Remove or hide the two `pub fn`s. Keep
  `request_version` (#21 closed; no longer needed).

## Acceptance mapping

Six Implementation Plan bullets map 1:1 to A01–A06.

- PR-084-A01: `ContentProjection::ToolCall` includes
  `provider_call_id`; a new tool-call hash fixture is added; the
  existing seven pinned digests are unchanged.
- PR-084-A02: Human-path `RawJson` decode applies byte, item, and
  depth limits before full materialization.
- PR-084-A03: A TM-04-style memo records the kernel
  telemetry/secret-needle review.
- PR-084-A04: #13a is folded into PR-067 or landed here; #13b is
  not landed.
- PR-084-A05: #17 lands `BoundedMap` (source-breaking, 1.0.0
  pre-publication, no ADR, no major bump); a matrix row is
  recorded; freeze baselines are `--write` updated only if a
  public name is added or removed.
- PR-084-A06: The two `pub fn`s are removed or hidden with
  matrix/backlog rows. `#21` is keep-the-field / no longer needed.
  The public-rust-api corpus count is updated if fixtures are
  added.

## Explicit exclusions

`#13b` `InvariantViolation` reshape. `#21` `request_version`
removal (keep the field; question closed). New `KernelError`
variants. Journal `RecordBody`, WIT, or remote-protocol meaning
change. Inventing a major version bump.

## Compatibility class

Wire-visible and public-type change. Source-breaking 1.0.0
pre-publication where classified. No major bump. B1 sees added
and removed names only.

## Dependencies

PR-082. #9's pinned digest must be computed after #1/#4/#5. #17
is decided BoundedMap (no ADR). #21 is keep-the-field.

## Suggested authorization sentence

```
Run PR-084; mode=integrated; target=main; local branch/commit/merge
authorized; external actions=none; stop before any gate crossing.
```

Do not infer admission from this planning file.
