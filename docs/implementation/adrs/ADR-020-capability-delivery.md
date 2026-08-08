# ADR-020: capability delivery

## Status

Accepted

## Date

2026-08-08

## Accountable role

Ecosystem lead

## Decision owners

me@jeickmeier.com (finstack-ai maintainers)

## Context

This decision was accepted in the planning baseline and is recorded here so later
implementation pull requests cannot silently reinterpret it. Canonical summary
text lives in Architecture Specification §25; this standalone record captures
stable decision context, consequences, compatibility classification, security
linkage, and change-control metadata required by PR-004.

## Decision

Capability mechanics ship in MVP; model-activated catalog UX is required by public preview

## Consequences

MVP includes activation mechanics; model-activated catalog UX is required before public preview.

## Rejected alternatives

Deferring all capability mechanics past MVP, or shipping model-activated UX without durable activation foundations.

## Compatibility and schema-change classification

Capability schema and binding UX; mechanics before catalog UX.

## Security classification

- References: SEC-INV-001; TM-01
- Threat Model review trigger: [§18](../../planning/06-finstack-ai-security-threat-model.md#18-review-triggers)
- Control-change disposition: No control change: standalone recording of the accepted planning baseline; no new trust boundary, threat, or control is introduced by creating this ADR file.

## Affected requirements, design, and delivery

- Affected requirements: FR-CAP
- Affected Technical Design: Technical Design §10 (capability delivery)

- Architecture Specification §25 decision summary: [link](../../planning/02-finstack-ai-architecture-specification.md#25-architecture-decision-summary)
- Implementation Plan logical PR-004 and mapped delivery: PR-012, PR-022, PR-032, PR-038, PR-048
- Engineering Standards public-contract and ADR-trigger rules apply where this decision defines a compatibility or architecture gate

### PRD §18 delivery detail

- Product decision: Put durable activation mechanics plus always/application modes in MVP; complete model-activated catalog UX after MVP and before 0.1.0.
- Delivery point: PR-012/PR-022 foundations; PR-032/PR-038 binding UX; PR-048 durability gate.
- Source: [PRD §18](../../planning/01-finstack-ai-product-requirements.md#18-resolved-foundational-product-decisions)

## Supersession metadata

None. This ADR supersedes no prior ADR and is not superseded.

## Reconsideration conditions

May change only through a new superseding ADR and reconciliation of every affected primary planning document (ADR register change-control rules).

## Approval and implementation-evidence links

- Approval: accepted in the planning baseline (Architecture Specification §25); recorded as standalone under PR-004
- Implementation evidence: Missing until mapped delivery work completes and evidence is verified
