# ADR-026: run lineage

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

Every run persists explicit root/parent/effect lineage

## Consequences

Cancellation, budgets, and audit can correlate nested/delegated runs.

## Rejected alternatives

Implicit parentage without durable root/parent/effect lineage.

## Compatibility and schema-change classification

Journal/lineage schema family; relation fields are durable and auditable.

## Security classification

- References: SEC-INV-008; TM-12, TM-15, TM-19
- Threat Model review trigger: [§18](../../planning/06-finstack-ai-security-threat-model.md#18-review-triggers)
- Control-change disposition: No control change: standalone recording of the accepted planning baseline; no new trust boundary, threat, or control is introduced by creating this ADR file.

## Affected requirements, design, and delivery

- Affected requirements: FR-DUR, FR-KRN
- Affected Technical Design: Technical Design §12 (run lineage)

- Architecture Specification §25 decision summary: [link](../../planning/02-finstack-ai-architecture-specification.md#25-architecture-decision-summary)
- Implementation Plan logical PR-004 and mapped delivery: PR-006, PR-008, PR-046–PR-048
- Engineering Standards public-contract and ADR-trigger rules apply where this decision defines a compatibility or architecture gate

## Supersession metadata

None. This ADR supersedes no prior ADR and is not superseded.

## Reconsideration conditions

May change only through a new superseding ADR and reconciliation of every affected primary planning document (ADR register change-control rules).

## Approval and implementation-evidence links

- Approval: accepted in the planning baseline (Architecture Specification §25); recorded as standalone under PR-004
- Implementation evidence: Partial: PR-013 lineage attenuation at
  `aba26764a448f6b2691bfac62a0f36f865f61ed6`
  (PR-013-E-kernel-3b7e91c5a2d4; PR-013-E-security-7d2a5f9c1e84). PR-046
  candidate `dc16907e4e8d2c8a303788dbb9fc04febb1a0572` persists and restores
  the parent-effect → child-UUIDv7 mapping and `RunRelation` inspect for
  CompatibleLane and IsolatedChildSession (PR-046-E-tree-dc16907e4e8d;
  PR-046-E-security-dc16907e4e8d). PR-047 candidate
  `14af719a078f1cdf52382e89e0a44837ab207237` adds lineage-aware cancel /
  deadline fan-out; detach-preauthorized children stay up
  (PR-047-E-lanes-14af719a078f; PR-047-E-security-14af719a078f).
  Crash-prefix remains PR-048. Do not mark Implemented.
