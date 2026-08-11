# ADR-009: observer middleware separation

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

Observers are immutable and separate from behavior-changing middleware

## Consequences

Telemetry and audit consumers cannot change terminal state.

## Rejected alternatives

Allowing observers to mutate execution or collapse into middleware.

## Compatibility and schema-change classification

Runtime event and middleware contracts; observers remain non-authoritative.

## Security classification

- References: SEC-INV-009; TM-17
- Threat Model review trigger: [§18](../../planning/06-finstack-ai-security-threat-model.md#18-review-triggers)
- Control-change disposition: No control change: standalone recording of the accepted planning baseline; no new trust boundary, threat, or control is introduced by creating this ADR file.

## Affected requirements, design, and delivery

- Affected requirements: FR-OBS, FR-MW
- Affected Technical Design: Technical Design §17, §19 (observer vs middleware separation)

- Architecture Specification §25 decision summary: [link](../../planning/02-finstack-ai-architecture-specification.md#25-architecture-decision-summary)
- Implementation Plan logical PR-004 and mapped delivery: PR-017, PR-018, PR-020, PR-057
- Engineering Standards public-contract and ADR-trigger rules apply where this decision defines a compatibility or architecture gate

## Supersession metadata

None. This ADR supersedes no prior ADR and is not superseded.

## Reconsideration conditions

May change only through a new superseding ADR and reconciliation of every affected primary planning document (ADR register change-control rules).

## Approval and implementation-evidence links

- Approval: accepted in the planning baseline (Architecture Specification §25); recorded as standalone under PR-004
- Implementation evidence: Partial through locally integrated PR-017 bounded observer routing plus locally integrated PR-018 read-only `Observer`, separate typed `Middleware`, and TM-17 evidence (`PR-018-E-integration-3fe0314c6434`) at `3fe0314c6434211e1c8f493f24401888f9609050`; PR-020 and PR-057 remain
