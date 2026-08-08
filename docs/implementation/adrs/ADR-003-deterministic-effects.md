# ADR-003: deterministic effects

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

Kernel decisions are deterministic and external work is represented as effects

## Consequences

Time/randomness enter through explicit inputs; effects are the only external work representation.

## Rejected alternatives

Allowing non-deterministic kernel decisions or embedding concrete I/O in the reducer.

## Compatibility and schema-change classification

Kernel semantic contract; effect envelopes and fixtures must remain deterministic.

## Security classification

- References: SEC-INV-001, SEC-INV-002; TM-01, TM-02, TM-14
- Threat Model review trigger: [§18](../../planning/06-finstack-ai-security-threat-model.md#18-review-triggers)
- Control-change disposition: No control change: standalone recording of the accepted planning baseline; no new trust boundary, threat, or control is introduced by creating this ADR file.

## Affected requirements, design, and delivery

- Affected requirements: FR-KRN, FR-DUR
- Affected Technical Design: Technical Design §5–§13 (deterministic effects and commit inputs)

- Architecture Specification §25 decision summary: [link](../../planning/02-finstack-ai-architecture-specification.md#25-architecture-decision-summary)
- Implementation Plan logical PR-004 and mapped delivery: PR-002, PR-008–PR-010; G1
- Engineering Standards public-contract and ADR-trigger rules apply where this decision defines a compatibility or architecture gate

## Supersession metadata

None. This ADR supersedes no prior ADR and is not superseded.

## Reconsideration conditions

May change only through a new superseding ADR and reconciliation of every affected primary planning document (ADR register change-control rules).

## Approval and implementation-evidence links

- Approval: accepted in the planning baseline (Architecture Specification §25); recorded as standalone under PR-004
- Implementation evidence: Missing until mapped delivery work completes and evidence is verified
