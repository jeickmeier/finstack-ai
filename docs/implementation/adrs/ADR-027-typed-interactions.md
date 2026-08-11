# ADR-027: typed interactions

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

Approval is a standard profile of generalized typed interactions

## Consequences

Human/external input uses one interaction mechanism with typed profiles.

## Rejected alternatives

A separate durable approval mechanism outside typed interactions.

## Compatibility and schema-change classification

Interaction schema family; approval is a profile, not a separate mechanism.

## Security classification

- References: SEC-INV-003, SEC-INV-004; TM-11
- Threat Model review trigger: [§18](../../planning/06-finstack-ai-security-threat-model.md#18-review-triggers)
- Control-change disposition: No control change: standalone recording of the accepted planning baseline; no new trust boundary, threat, or control is introduced by creating this ADR file.

## Affected requirements, design, and delivery

- Affected requirements: FR-DUR, FR-MW
- Affected Technical Design: Technical Design §12, §17 (typed interactions)

- Architecture Specification §25 decision summary: [link](../../planning/02-finstack-ai-architecture-specification.md#25-architecture-decision-summary)
- Implementation Plan logical PR-004 and mapped delivery: PR-008, PR-018, PR-044, PR-048
- Engineering Standards public-contract and ADR-trigger rules apply where this decision defines a compatibility or architecture gate

## Supersession metadata

None. This ADR supersedes no prior ADR and is not superseded.

## Reconsideration conditions

May change only through a new superseding ADR and reconciliation of every affected primary planning document (ADR register change-control rules).

## Approval and implementation-evidence links

- Approval: accepted in the planning baseline (Architecture Specification §25); recorded as standalone under PR-004
- Implementation evidence: Partial through PR-013 typed DTO semantics and locally integrated PR-018 `RequestInteraction` outcome/approval-profile evidence (`PR-018-E-integration-3fe0314c6434`) at `3fe0314c6434211e1c8f493f24401888f9609050`; PR-044/PR-048 routing and lifecycle remain
