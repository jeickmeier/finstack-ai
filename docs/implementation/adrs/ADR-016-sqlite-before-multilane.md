# ADR-016: sqlite before multilane

## Status

Accepted

## Date

2026-08-08

## Accountable role

Durability/ecosystem lead

## Decision owners

me@jeickmeier.com (finstack-ai maintainers)

## Context

This decision was accepted in the planning baseline and is recorded here so later
implementation pull requests cannot silently reinterpret it. Canonical summary
text lives in Architecture Specification §25; this standalone record captures
stable decision context, consequences, compatibility classification, security
linkage, and change-control metadata required by PR-004.

## Decision

SQLite durability precedes concurrent multi-lane APIs

## Consequences

Public multi-lane APIs wait for SQLite sequence/conflict validation.

## Rejected alternatives

Shipping public concurrent multi-lane APIs before the first SQLite store.

## Compatibility and schema-change classification

Store delivery ordering policy; lane schema exists early, concurrency APIs wait for SQLite evidence.

## Security classification

- References: SEC-INV-008; TM-12, TM-15, TM-19
- Threat Model review trigger: [§18](../../planning/06-finstack-ai-security-threat-model.md#18-review-triggers)
- Control-change disposition: No control change: standalone recording of the accepted planning baseline; no new trust boundary, threat, or control is introduced by creating this ADR file.

## Affected requirements, design, and delivery

- Affected requirements: FR-DUR
- Affected Technical Design: Technical Design §24 (SQLite before multi-lane)

- Architecture Specification §25 decision summary: [link](../../planning/02-finstack-ai-architecture-specification.md#25-architecture-decision-summary)
- Implementation Plan logical PR-004 and mapped delivery: PR-040 before PR-047
- Engineering Standards public-contract and ADR-trigger rules apply where this decision defines a compatibility or architecture gate

### PRD §18 delivery detail

- Product decision: Ship the first SQLite store before public multi-lane concurrency. Lane IDs and immutable parent-linked entries remain in the Phase 1 data model.
- Delivery point: PR-040 precedes PR-047.
- Source: [PRD §18](../../planning/01-finstack-ai-product-requirements.md#18-resolved-foundational-product-decisions)

## Supersession metadata

None. This ADR supersedes no prior ADR and is not superseded.

## Reconsideration conditions

May change only through a new superseding ADR and reconciliation of every affected primary planning document (ADR register change-control rules).

## Approval and implementation-evidence links

- Approval: accepted in the planning baseline (Architecture Specification §25); recorded as standalone under PR-004
- Implementation evidence: Partial after PR-040 local merge
  `dbd10d35b223288666b2fdc0e13d03f48b5b97c3` and PR-047 candidate
  `14af719a078f1cdf52382e89e0a44837ab207237` (A04 SQLite parallel lane
  appenders; PR-047-E-lanes-14af719a078f). PR-048 sqlite crash-prefix/ops
  and G5-D-durable-beta-a9568bd869b5 close the mapped order. Verified.
