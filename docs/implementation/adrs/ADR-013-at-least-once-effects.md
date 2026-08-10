# ADR-013: at least once effects

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

Exactly-once external side effects are not claimed; stable idempotency keys are provided

## Consequences

Applications must handle duplicates via idempotency keys and reconciliation.

## Rejected alternatives

Claiming exactly-once external side effects without reconciliation.

## Compatibility and schema-change classification

Effect/journal semantics; at-least-once plus idempotency/reconciliation only.

## Security classification

- References: SEC-INV-003, SEC-INV-004, SEC-INV-008; TM-10, TM-14
- Threat Model review trigger: [§18](../../planning/06-finstack-ai-security-threat-model.md#18-review-triggers)
- Control-change disposition: No control change: standalone recording of the accepted planning baseline; no new trust boundary, threat, or control is introduced by creating this ADR file.

## Affected requirements, design, and delivery

- Affected requirements: FR-DUR
- Affected Technical Design: Technical Design §12–§13 (at-least-once effects)

- Architecture Specification §25 decision summary: [link](../../planning/02-finstack-ai-architecture-specification.md#25-architecture-decision-summary)
- Implementation Plan logical PR-004 and mapped delivery: PR-006–PR-008, PR-014, PR-043, PR-046–PR-048
- Engineering Standards public-contract and ADR-trigger rules apply where this decision defines a compatibility or architecture gate

## Supersession metadata

None. This ADR supersedes no prior ADR and is not superseded.

## Reconsideration conditions

May change only through a new superseding ADR and reconciliation of every affected primary planning document (ADR register change-control rules).

## Approval and implementation-evidence links

- Approval: accepted in the planning baseline (Architecture Specification §25); recorded as standalone under PR-004
- Implementation evidence: Partial: PR-014 candidate commit
  `ff9178f2244c8d8fd0c62cbba18b2b4dfbee2a29` provides frozen append retries,
  replay recovery, stable effect/completion identities, equal duplicate
  idempotency, and durable conflicting-command rejection
  (PR-014-E-runtime-f7d0f0ecede0; PR-014-E-security-a8a835efcf19). Concrete
  effect drivers and later persistent reconciliation remain mapped work.
