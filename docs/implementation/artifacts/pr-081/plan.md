# PR-081 execution plan

Date: 2026-08-18
Owner: me@jeickmeier.com
Plan baseline: documentation pack v0.26 / PLAN-0.24

This file is the execution contract for PR-081 (inert cleanup).
Closed PR-001–PR-080 envelopes are not reused. PR-081 is the only
active logical PR once admitted. This file does not admit
PR-082–PR-084.

## Purpose

Remove proven-dead private bookkeeping, fix remaining rustdoc
drift, collapse fingerprint/projection twins, and add
`RunEventKind::kind_name()` so later slices start from a clean
crate.

## Principal changes

- Remove `buffered_prefix`, `recompute_buffered_prefix`, and
  `note_source_advanced` in `records/tools.rs`.
- Fix remaining #18 and #23 rustdoc (not F23).
- Collapse the three fingerprint/projection twins.
- Add `RunEventKind::kind_name()`.
- Leave `raw_json.rs:128-145` and finding #10 alone. Do not remove
  the two `pub fn`s (PR-084).

## Acceptance mapping

Five Implementation Plan bullets map 1:1 to A01–A05.

- PR-081-A01: `buffered_prefix`, `recompute_buffered_prefix`, and
  `note_source_advanced` are gone; the field remains out of serde,
  `PartialEq`, and `state_hash`.
- PR-081-A02: Remaining #18 and #23 rustdoc sites no longer claim
  schema-1-only behavior.
- PR-081-A03: Fingerprint/projection twin duplication at the three
  named sites is collapsed to one owner.
- PR-081-A04: `RunEventKind::kind_name()` returns the section
  20.2.1 kind string and is pinned by a unit test.
- PR-081-A05: The `raw_json.rs` member-limit path at lines 128–145
  is unchanged. Finding #10 is not unified in this PR.

## Explicit exclusions

Decide/apply/cost behavior (PR-082). Validator tightening
(PR-083). Wire-visible type changes and `pub fn` removals
(PR-084). Finding #10 unification. `raw_json.rs` 128–145 deletion.

## Compatibility class

Docs / inert private cleanup plus one additive inherent method.
`kind_name()` is named review; B1 does not extract signatures.

## Dependencies

PR-080. May proceed in parallel with the start of PR-082.

## Suggested authorization sentence

```
Run PR-081; mode=integrated; target=main; local branch/commit/merge
authorized; external actions=none; stop before any gate crossing.
```

Do not infer admission from this planning file.
