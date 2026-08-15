# ADR-028: before finalize

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

before_finalize is the final behavior-changing middleware stage; post-run handling is observation only

## Consequences

No behavior-changing stage exists after before_finalize.

## Rejected alternatives

Post-terminal hooks that can change run outcomes.

## Compatibility and schema-change classification

Middleware stage contract; eighth stage requires a new ADR before merge.

## Security classification

- References: SEC-INV-009; TM-17
- Threat Model review trigger: [§18](../../planning/06-finstack-ai-security-threat-model.md#18-review-triggers)
- Control-change disposition: No control change: standalone recording of the accepted planning baseline; no new trust boundary, threat, or control is introduced by creating this ADR file.

## Affected requirements, design, and delivery

- Affected requirements: FR-MW
- Affected Technical Design: Technical Design §17 (before_finalize stage)

- Architecture Specification §25 decision summary: [link](../../planning/02-finstack-ai-architecture-specification.md#25-architecture-decision-summary)
- Implementation Plan logical PR-004 and mapped delivery: PR-009, PR-018, PR-048
- Engineering Standards public-contract and ADR-trigger rules apply where this decision defines a compatibility or architecture gate

## Supersession metadata

None. This ADR supersedes no prior ADR and is not superseded.

## Reconsideration conditions

May change only through a new superseding ADR and reconciliation of every affected primary planning document (ADR register change-control rules).

## Approval and implementation-evidence links

- Approval: accepted in the planning baseline (Architecture Specification §25); recorded as standalone under PR-004
- Implementation evidence: Partial through PR-013 final-boundary semantics and locally integrated PR-018 seven-stage outcome-matrix evidence proving `before_finalize` permits interaction/retry but rejects replacement (`PR-018-E-integration-3fe0314c6434`) at `3fe0314c6434211e1c8f493f24401888f9609050`; PR-048 before_finalize restart and G5-D-durable-beta-a9568bd869b5 close the mapped durability work. Verified
