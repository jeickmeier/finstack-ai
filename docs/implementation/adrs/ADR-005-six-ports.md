# ADR-005: six ports

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

Six primary extension ports are sufficient for the first major version

## Consequences

Ordinary integrations implement one of the six ports; new port kinds need an ADR.

## Rejected alternatives

Adding ad-hoc ports for every integration concern in v1.

## Compatibility and schema-change classification

Public extension ABI boundary; adding a seventh port requires a new ADR before merge.

## Security classification

- References: TM-04, TM-05, TM-06
- Threat Model review trigger: [§18](../../planning/06-finstack-ai-security-threat-model.md#18-review-triggers)
- Control-change disposition: No control change: standalone recording of the accepted planning baseline; no new trust boundary, threat, or control is introduced by creating this ADR file.

## Affected requirements, design, and delivery

- Affected requirements: FR-EXT, FR-MDL, FR-TLS, FR-CTX, FR-MW, FR-DUR, FR-OBS
- Affected Technical Design: Technical Design §8–§19; Architecture Specification §6 (six primary ports)

- Architecture Specification §25 decision summary: [link](../../planning/02-finstack-ai-architecture-specification.md#25-architecture-decision-summary)
- Implementation Plan logical PR-004 and mapped delivery: PR-002, PR-015–PR-018, PR-021, PR-026
- Engineering Standards public-contract and ADR-trigger rules apply where this decision defines a compatibility or architecture gate

## Supersession metadata

None. This ADR supersedes no prior ADR and is not superseded.

## Reconsideration conditions

May change only through a new superseding ADR and reconciliation of every affected primary planning document (ADR register change-control rules).

## Approval and implementation-evidence links

- Approval: accepted in the planning baseline (Architecture Specification §25); recorded as standalone under PR-004
- Implementation evidence: Missing until mapped delivery work completes and evidence is verified
