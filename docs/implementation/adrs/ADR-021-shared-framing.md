# ADR-021: shared framing

## Status

Accepted

## Date

2026-08-08

## Accountable role

Runtime/security owner

## Decision owners

me@jeickmeier.com (finstack-ai maintainers)

## Context

This decision was accepted in the planning baseline and is recorded here so later
implementation pull requests cannot silently reinterpret it. Canonical summary
text lives in Architecture Specification §25; this standalone record captures
stable decision context, consequences, compatibility classification, security
linkage, and change-control metadata required by PR-004.

## Decision

Remote and process protocols share framing/handshake code but not message vocabularies

## Consequences

Shared framing code is reused; message schemas stay family-specific.

## Rejected alternatives

Sharing full message vocabularies between remote and process protocols.

## Compatibility and schema-change classification

Remote/process framing family; vocabularies version independently.

## Security classification

- References: TM-09
- Threat Model review trigger: [§18](../../planning/06-finstack-ai-security-threat-model.md#18-review-triggers)
- Control-change disposition: No control change: standalone recording of the accepted planning baseline; no new trust boundary, threat, or control is introduced by creating this ADR file.

## Affected requirements, design, and delivery

- Affected requirements: FR-PLG
- Affected Technical Design: Technical Design §28 (shared framing; distinct remote/process vocabularies)

- Architecture Specification §25 decision summary: [link](../../planning/02-finstack-ai-architecture-specification.md#25-architecture-decision-summary)
- Implementation Plan logical PR-004 and mapped delivery: PR-058
- Engineering Standards public-contract and ADR-trigger rules apply where this decision defines a compatibility or architecture gate

### PRD §18 delivery detail

- Product decision: Share bounded framing, envelope, and handshake code between remote and process protocols, but keep their message vocabularies distinct.
- Delivery point: Generic framing in PR-058; process vocabulary later.
- Source: [PRD §18](../../planning/01-finstack-ai-product-requirements.md#18-resolved-foundational-product-decisions)

## Supersession metadata

None. This ADR supersedes no prior ADR and is not superseded.

## Reconsideration conditions

May change only through a new superseding ADR and reconciliation of every affected primary planning document (ADR register change-control rules).

## Approval and implementation-evidence links

- Approval: accepted in the planning baseline (Architecture Specification §25); recorded as standalone under PR-004
- Implementation evidence: Verified through locally integrated PR-058 shared frame/handshake, family-tagged envelopes, and distinct remote/process vocabularies (`PR-058-E-integration-99ac0fb69952`) at `99ac0fb69952c9e6bfe8354e9d1f930ee061b4a0`. Process session commands remain later work; handshake kinds stay family-separate.
