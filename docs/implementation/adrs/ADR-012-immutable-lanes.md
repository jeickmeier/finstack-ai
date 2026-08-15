# ADR-012: immutable lanes

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

Sessions use immutable entries and lane identifiers from the initial data model

## Consequences

Session data model includes lanes from Phase 1 without promising public multi-lane concurrency yet.

## Rejected alternatives

Linear histories without lane identifiers, forcing a later journal migration.

## Compatibility and schema-change classification

Journal/session schema family; lane identifiers are part of the initial data model.

## Security classification

- References: SEC-INV-008; TM-12, TM-15, TM-19
- Threat Model review trigger: [§18](../../planning/06-finstack-ai-security-threat-model.md#18-review-triggers)
- Control-change disposition: No control change: standalone recording of the accepted planning baseline; no new trust boundary, threat, or control is introduced by creating this ADR file.

## Affected requirements, design, and delivery

- Affected requirements: FR-DUR, FR-KRN
- Affected Technical Design: Technical Design §7, §12, §24 (immutable lanes)

- Architecture Specification §25 decision summary: [link](../../planning/02-finstack-ai-architecture-specification.md#25-architecture-decision-summary)
- Implementation Plan logical PR-004 and mapped delivery: PR-006–PR-008, PR-014, PR-046–PR-048
- Engineering Standards public-contract and ADR-trigger rules apply where this decision defines a compatibility or architecture gate

## Supersession metadata

None. This ADR supersedes no prior ADR and is not superseded.

## Reconsideration conditions

May change only through a new superseding ADR and reconciliation of every affected primary planning document (ADR register change-control rules).

## Approval and implementation-evidence links

- Approval: accepted in the planning baseline (Architecture Specification §25); recorded as standalone under PR-004
- Implementation evidence: Partial: PR-014 merge
  `399f3a7d9d987268f4d79ab90f31b93f854084f8` preserves lane identity in direct
  session loads, replay, and authenticated locator validation
  (PR-014-E-runtime-f7d0f0ecede0; PR-014-E-security-a8a835efcf19;
  PR-014-E-integration-e7ec699722ab). PR-046 candidate
  `dc16907e4e8d2c8a303788dbb9fc04febb1a0572` adds the immutable conversation
  tree, mandatory `main` lane bootstrap, and sibling `ConversationEntry` /
  `LaneMoved` persist/recover (PR-046-E-tree-dc16907e4e8d;
  PR-046-E-security-dc16907e4e8d). PR-047 candidate
  `14af719a078f1cdf52382e89e0a44837ab207237` adds public concurrent
  multi-lane Session/Lane APIs and in-process `(session_id, lane_id)`
  guards (PR-047-E-lanes-14af719a078f; PR-047-E-security-14af719a078f).
  Crash-prefix and lane restart pass at PR-048 local merge
  `b0641b0be338918c1951339657ce8c04c4ccff59`. Verified by
  G5-D-durable-beta-a9568bd869b5.
