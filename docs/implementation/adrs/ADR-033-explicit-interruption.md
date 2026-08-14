# ADR-033: explicit interruption

## Status

Accepted

## Date

2026-08-08

## Accountable role

Core/runtime lead

## Decision owners

me@jeickmeier.com (finstack-ai maintainers)

## Context

This decision was accepted in the planning baseline and is recorded here so later
implementation pull requests cannot silently reinterpret it. Canonical summary
text lives in Architecture Specification §25; this standalone record captures
stable decision context, consequences, compatibility classification, security
linkage, and change-control metadata required by PR-004.

## Decision

MVP interruption uses retry/suspend/explicit uncertainty while the Model port reserves optional reconciliation implemented with durability

## Consequences

Interrupted work is explicit; provider reconcile is optional and durability-gated.

## Rejected alternatives

Silent provider cancellation without retry, suspend, or explicit uncertainty.

## Compatibility and schema-change classification

Model port interruption contract; reconciliation reserved and implemented with durability.

## Security classification

- References: SEC-INV-003, SEC-INV-004, SEC-INV-008; TM-10, TM-14
- Threat Model review trigger: [§18](../../planning/06-finstack-ai-security-threat-model.md#18-review-triggers)
- Control-change disposition: No control change: standalone recording of the accepted planning baseline; no new trust boundary, threat, or control is introduced by creating this ADR file.

## Affected requirements, design, and delivery

- Affected requirements: FR-MDL, FR-DUR
- Affected Technical Design: Technical Design §14, §22 (explicit interruption)

- Architecture Specification §25 decision summary: [link](../../planning/02-finstack-ai-architecture-specification.md#25-architecture-decision-summary)
- Implementation Plan logical PR-004 and mapped delivery: PR-042
- Engineering Standards public-contract and ADR-trigger rules apply where this decision defines a compatibility or architecture gate
- Reconsideration source: [Technical Design §37](../../planning/03-finstack-ai-technical-design.md#37-technical-decision-status)

## Supersession metadata

None. This ADR supersedes no prior ADR and is not superseded.

## Reconsideration conditions

Exact Technical Design §37 gate: PR-042 provider evidence determines per-provider support, not the port shape.

Additionally requires a superseding ADR and primary-document reconciliation.

## Approval and implementation-evidence links

- Approval: accepted in the planning baseline (Architecture Specification §25); recorded as standalone under PR-004
- Implementation evidence: Verified at PR-042 local merge `4d627711632c771d733b689e1325b2d9679ee317` (A01–A05; TM-10/TM-14 review)
