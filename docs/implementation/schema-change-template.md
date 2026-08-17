# Schema / contract change classification template

Copy this template into the pull request (or attach it) whenever a change
touches a compatibility-controlled contract family. Complete every field; use
`N/A` only with a one-line reason.

## Change identity

- Family / version:
- Kind / artifact path(s):
- Owner:
- Reviewer:
- Logical PR:

## Classification

- Compatibility class: `additive` / `ignorable-optional` / `breaking` / `policy-only` / `documentation-only`
- Unknown-field impact:
- Affects durable replay? `yes` / `no`
- Affects authorization, idempotency, ordering, or recovery? `yes` / `no`
- Cross-binding impact: Rust / Python / JavaScript / WIT / none

## Required companion updates

- Migration or compatibility plan:
- Fixture updates (paths):
- Changelog entry: `yes` / `no` (link)
- ADR trigger (Implementation Plan §6.3)? `yes` / `no` (ADR id or rationale)
- Threat Model §18 trigger? `yes` / `no`
- Threat Model disposition: updated controls / no control change (reason)

## Rollback

- Rollback or forward-fix strategy:
- Can old readers reject safely? `yes` / `no`

## Checklist

- [ ] `schemas/schema-families.toml` owner/promise still accurate
- [ ] Fixtures under `fixtures/compatibility/<family>/` updated in this change
- [ ] Per-family Rust fixture coupling still holds (`crates/finstack-ai-test/tests/public_rust_api.rs`, `crates/finstack-ai-test/tests/journal_v1.rs`, `crates/finstack-ai-protocol/tests/compat_fixtures.rs`)
- [ ] PR template API / schema / performance / security impact sections completed
