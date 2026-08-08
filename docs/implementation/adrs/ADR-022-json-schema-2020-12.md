# ADR-022: json schema 2020 12

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

JSON Schema 2020-12 is the schema source of truth with conformant native/binding validators

## Consequences

Native and binding validators conform to the same draft and fixtures.

## Rejected alternatives

Provider-specific or binding-local schema dialects without a shared 2020-12 source of truth.

## Compatibility and schema-change classification

JSON Schema family source-of-truth policy; validators must share fixtures.

## Security classification

- References: TM-02, TM-16
- Threat Model review trigger: [§18](../../planning/06-finstack-ai-security-threat-model.md#18-review-triggers)
- Control-change disposition: No control change: standalone recording of the accepted planning baseline; no new trust boundary, threat, or control is introduced by creating this ADR file.

## Affected requirements, design, and delivery

- Affected requirements: FR-TLS, FR-SPEC
- Affected Technical Design: Technical Design §15, §28 (JSON Schema 2020-12)

- Architecture Specification §25 decision summary: [link](../../planning/02-finstack-ai-architecture-specification.md#25-architecture-decision-summary)
- Implementation Plan logical PR-004 and mapped delivery: Decide before PR-012; PR-031 and binding peers
- Engineering Standards public-contract and ADR-trigger rules apply where this decision defines a compatibility or architecture gate

### PRD §18 delivery detail

- Product decision: Use JSON Schema draft 2020-12 as the source of truth, a default precompiled Rust validator, and optional binding-native validators behind shared fixtures.
- Delivery point: ADR before PR-012; adapters in PR-031 and peers.
- Source: [PRD §18](../../planning/01-finstack-ai-product-requirements.md#18-resolved-foundational-product-decisions)

## Supersession metadata

None. This ADR supersedes no prior ADR and is not superseded.

## Reconsideration conditions

May change only through a new superseding ADR and reconciliation of every affected primary planning document (ADR register change-control rules).

## Approval and implementation-evidence links

- Approval: accepted in the planning baseline (Architecture Specification §25); recorded as standalone under PR-004
- Implementation evidence: Missing until mapped delivery work completes and evidence is verified
