# ADR-024: governance and license

## Status

Accepted

## Date

2026-08-08

## Accountable role

Quality/release owner

## Decision owners

me@jeickmeier.com (finstack-ai maintainers)

## Context

This decision was accepted in the planning baseline and is recorded here so later
implementation pull requests cannot silently reinterpret it. Canonical summary
text lives in Architecture Specification §25; this standalone record captures
stable decision context, consequences, compatibility classification, security
linkage, and change-control metadata required by PR-004.

## Decision

The project uses MIT OR Apache-2.0, DCO, named maintainers, and ADR/RFC governance

## Consequences

Contribution, licensing, and ADR/RFC process are explicit before external contribution.

## Rejected alternatives

CLA-only contribution licensing or unnamed maintainer authority.

## Compatibility and schema-change classification

Governance/license policy; not a runtime schema family.

## Security classification

- References: TM-18
- Threat Model review trigger: [§18](../../planning/06-finstack-ai-security-threat-model.md#18-review-triggers)
- Control-change disposition: No control change: standalone recording of the accepted planning baseline; no new trust boundary, threat, or control is introduced by creating this ADR file.

## Affected requirements, design, and delivery

- Affected requirements: NFR-COMP, NFR-DX
- Affected Technical Design: Technical Design governance/delivery surfaces; Engineering Standards license/ownership

- Architecture Specification §25 decision summary: [link](../../planning/02-finstack-ai-architecture-specification.md#25-architecture-decision-summary)
- Implementation Plan logical PR-004 and mapped delivery: PR-001, PR-004; G0
- Engineering Standards public-contract and ADR-trigger rules apply where this decision defines a compatibility or architecture gate

### PRD §18 delivery detail

- Product decision: Dual-license under MIT OR Apache-2.0, use DCO sign-off, and govern through a named maintainer group plus the existing ADR/RFC process.
- Delivery point: License/governance files in PR-001; ADR in PR-004.
- Source: [PRD §18](../../planning/01-finstack-ai-product-requirements.md#18-resolved-foundational-product-decisions)

## Supersession metadata

None. This ADR supersedes no prior ADR and is not superseded.

## Reconsideration conditions

May change only through a new superseding ADR and reconciliation of every affected primary planning document (ADR register change-control rules).

## Approval and implementation-evidence links

- Approval: accepted in the planning baseline (Architecture Specification §25); recorded as standalone under PR-004
- Implementation evidence: Partial: license/governance files at `73bfe88c8dbc92c4e4c6eba1a5a7814240c2e862` (PR-001-E-ownership-review-b14626f70259; PR-001-E-security-md-f70391db8ac2). Standalone ADR text completed in PR-004; full Verified evidence awaits remaining mapped delivery.
